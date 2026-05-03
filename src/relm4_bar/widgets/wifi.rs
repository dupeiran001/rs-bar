//! Wi-Fi widget. Renders one of five icons (off / weak / fair / good /
//! excellent) keyed by signal strength, optionally followed by a truncated
//! SSID label.
//!
//! Click → opens a popover containing the network list reported by the hub
//! (visible + known SSIDs), with a refresh button that triggers an nmcli
//! rescan and per-row click-to-connect. The currently-connected SSID gets a
//! `wifi-row-connected` class for distinct styling.
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`:
//! cached SVG textures via `OnceLock`, model holds the GTK widgets needed
//! for in-place updates, watch receiver bridged into component messages on
//! the GTK main context, and `update` short-circuits when the displayed
//! value is unchanged.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::wifi::{KnownNetwork, WifiState};
use crate::subscribe_into_msg;

use super::popover::{self, BarPopover};
use super::{NamedWidget, WidgetInit, capsule, capsule_interactive, set_exclusive_class};

// ── symbolic icon names (one per signal band) ───────────────────────────

const WIFI_OFF: &str = "wifi-off-symbolic";
const WIFI_WEAK: &str = "wifi-weak-symbolic";
const WIFI_FAIR: &str = "wifi-fair-symbolic";
const WIFI_GOOD: &str = "wifi-good-symbolic";
const WIFI_EXCELLENT: &str = "wifi-excellent-symbolic";

/// CSS classes for the four signal/state colour bands. `set_exclusive_class`
/// strips the others before adding the chosen one, so stale classes can't
/// accumulate.
const COLOR_CLASSES: &[&str] = &["wifi-disconnected", "wifi-low", "wifi-mid", "wifi-high"];

/// Maximum visible characters of the SSID in the bar (popover always shows
/// the full name). Mirrors rs-bar's compact bar layout.
const SSID_BAR_MAX: usize = 16;

/// Choose the bar icon for the current state. Same thresholds as rs-bar's
/// GPUI version: 80/60/40/20 percent (else off).
fn icon_for(state: &WifiState) -> &'static str {
    match state.connected.as_ref() {
        _ if !state.enabled => WIFI_OFF,
        None => WIFI_OFF,
        Some(c) if c.signal >= 80 => WIFI_EXCELLENT,
        Some(c) if c.signal >= 60 => WIFI_GOOD,
        Some(c) if c.signal >= 40 => WIFI_FAIR,
        Some(c) if c.signal >= 20 => WIFI_WEAK,
        Some(_) => WIFI_OFF,
    }
}

/// Map state to a CSS colour band for the icon and SSID label.
fn class_for(state: &WifiState) -> &'static str {
    match state.connected.as_ref() {
        _ if !state.enabled => "wifi-disconnected",
        None => "wifi-disconnected",
        Some(c) if c.signal >= 60 => "wifi-high",
        Some(c) if c.signal >= 20 => "wifi-mid",
        Some(_) => "wifi-low",
    }
}

/// Truncate a SSID to `SSID_BAR_MAX` characters, appending `…` when it had to
/// be shortened.
fn truncate_ssid(s: &str) -> String {
    if s.chars().count() <= SSID_BAR_MAX {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(SSID_BAR_MAX - 1).collect();
        out.push('…');
        out
    }
}

/// Same icon helper, just for the popover row indicator.
fn icon_for_signal(signal: i32) -> &'static str {
    if signal >= 80 {
        WIFI_EXCELLENT
    } else if signal >= 60 {
        WIFI_GOOD
    } else if signal >= 40 {
        WIFI_FAIR
    } else if signal >= 20 {
        WIFI_WEAK
    } else {
        WIFI_OFF
    }
}

/// Shallow render-equality: do `a` and `b` produce the same on-screen output
/// for the bar (icon, ssid label, class)? Used to coalesce hub publishes.
fn bar_render_eq(a: &WifiState, b: &WifiState) -> bool {
    icon_for(a) == icon_for(b)
        && class_for(a) == class_for(b)
        && a.connected.as_ref().map(|c| c.ssid.as_str())
            == b.connected.as_ref().map(|c| c.ssid.as_str())
}

// ── component ───────────────────────────────────────────────────────────

pub struct Wifi {
    /// Last-rendered hub state, for the displayed-value coalescing check.
    state: WifiState,
    grouped: bool,
    /// Held so `update` can swap the paintable + colour class.
    icon: gtk::Image,
    /// Held so `update` can rewrite the SSID label.
    ssid_label: gtk::Label,
    /// Popover root and its scrolled list container — `update` rebuilds the
    /// list contents whenever the hub publishes new data so the popover
    /// reflects the latest scan even when already open.
    popover: gtk::Popover,
    list_box: gtk::ListBox,
    /// Cached row-handler installer. The popover rows are recreated on each
    /// hub update; this closure (held as a fresh sender clone) is what those
    /// rows call when clicked.
    sender: Rc<RefCell<Option<ComponentSender<Self>>>>,
}

#[derive(Debug)]
pub enum WifiMsg {
    Update(WifiState),
    Connect(String),
    Disconnect,
    Refresh,
}

#[relm4::component(pub)]
impl SimpleComponent for Wifi {
    type Init = WidgetInit;
    type Input = WifiMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(WIFI_OFF),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "ssid_label"]
            gtk::Label {
                set_label: "",
                set_visible: false,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();

        // ── popover content ──
        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        popover_box.set_margin_top(8);
        popover_box.set_margin_bottom(8);
        popover_box.set_margin_start(8);
        popover_box.set_margin_end(8);

        // Header row: title + Refresh button.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Wi-Fi Networks"));
        title.add_css_class("wifi-popover-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        header.append(&title);

        let refresh_btn = gtk::Button::with_label("Refresh");
        refresh_btn.add_css_class("wifi-refresh");
        {
            let s = sender.clone();
            refresh_btn.connect_clicked(move |_| s.input(WifiMsg::Refresh));
        }
        header.append(&refresh_btn);
        popover_box.append(&header);

        // Disconnect button (visible only when connected; toggled in `update`).
        let disconnect_btn = gtk::Button::with_label("Disconnect");
        disconnect_btn.add_css_class("wifi-disconnect");
        disconnect_btn.set_visible(false);
        {
            let s = sender.clone();
            disconnect_btn.connect_clicked(move |_| s.input(WifiMsg::Disconnect));
        }
        popover_box.append(&disconnect_btn);

        // Scrolled network list.
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_min_content_height(240);
        scroll.set_min_content_width(280);
        scroll.set_propagate_natural_height(true);
        let list_box = gtk::ListBox::new();
        list_box.add_css_class("wifi-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        scroll.set_child(Some(&list_box));
        popover_box.append(&scroll);

        let bar_popover = BarPopover::builder(&root, "wifi-popover").build(&popover_box);
        bar_popover.attach_click(&root);
        let popover = bar_popover.popover.clone();
        root.set_cursor_from_name(Some("pointer"));

        // Disconnect button visibility tracking via field on the model below;
        // we keep a clone here so `update` can toggle it.
        // Stash it on the popover via an attached object so it doesn't
        // require another model field.
        unsafe {
            popover.set_data::<gtk::Button>("disconnect-btn", disconnect_btn);
        }

        let model = Wifi {
            state: WifiState::default(),
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            ssid_label: widgets.ssid_label.clone(),
            popover: popover.clone(),
            list_box: list_box.clone(),
            sender: Rc::new(RefCell::new(Some(sender.clone()))),
        };

        capsule(&root, model.grouped);
        capsule_interactive(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<WifiState> into messages.
        subscribe_into_msg!(hub::wifi::subscribe(), sender, WifiMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WifiMsg::Update(new) => {
                let bar_changed = !bar_render_eq(&self.state, &new);

                if bar_changed {
                    let icon_name = icon_for(&new);
                    self.icon.set_icon_name(Some(icon_name));

                    let class = class_for(&new);
                    set_exclusive_class(&self.icon, class, COLOR_CLASSES);
                    set_exclusive_class(&self.ssid_label, class, COLOR_CLASSES);

                    if let Some(c) = new.connected.as_ref() {
                        if c.ssid.is_empty() {
                            self.ssid_label.set_visible(false);
                        } else {
                            self.ssid_label.set_label(&truncate_ssid(&c.ssid));
                            self.ssid_label.set_visible(true);
                        }
                    } else {
                        self.ssid_label.set_visible(false);
                    }
                }

                // Rebuild the popover network list. Cheap to rebuild; the
                // list typically holds <30 rows. We do this even when
                // `bar_changed` is false because the list itself may have
                // changed (new scan results) without affecting the bar.
                self.rebuild_popover_list(&new);

                self.state = new;
            }
            WifiMsg::Connect(ssid) => {
                hub::wifi::connect(&ssid);
                popover::popdown(&self.popover);
            }
            WifiMsg::Disconnect => {
                hub::wifi::disconnect();
                popover::popdown(&self.popover);
            }
            WifiMsg::Refresh => {
                hub::wifi::refresh();
            }
        }
    }
}

impl Wifi {
    /// Replace the popover list contents with one row per network. The
    /// currently-connected SSID gets `wifi-row-connected`; others get
    /// `wifi-row`. Each row connects a `GestureClick` that fires
    /// `WifiMsg::Connect(ssid)`.
    fn rebuild_popover_list(&self, state: &WifiState) {
        // Remove all existing rows.
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        // Toggle the disconnect button.
        if let Some(btn) = unsafe {
            self.popover
                .data::<gtk::Button>("disconnect-btn")
                .map(|p| p.as_ref().clone())
        } {
            btn.set_visible(state.connected.is_some());
        }

        let connected_ssid = state.connected.as_ref().map(|c| c.ssid.clone());

        for net in &state.networks {
            let row = make_row(net, connected_ssid.as_deref(), self.sender.clone());
            self.list_box.append(&row);
        }

        // Empty state.
        if state.networks.is_empty() {
            let empty = gtk::Label::new(Some(if state.enabled {
                "No networks found"
            } else {
                "Wi-Fi disabled"
            }));
            empty.add_css_class("wifi-empty");
            empty.set_margin_top(12);
            empty.set_margin_bottom(12);
            self.list_box.append(&empty);
        }
    }
}

/// Build one row widget for a `KnownNetwork`. Layout: `[icon][SSID … bold if
/// known][lock if secured][SIGNAL%]`. Click fires `WifiMsg::Connect(ssid)`.
fn make_row(
    net: &KnownNetwork,
    connected_ssid: Option<&str>,
    sender: Rc<RefCell<Option<ComponentSender<Wifi>>>>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("wifi-row");

    let is_connected = connected_ssid == Some(net.ssid.as_str());
    if is_connected {
        row.add_css_class("wifi-row-connected");
    }

    // Signal icon.
    let icon = gtk::Image::from_icon_name(icon_for_signal(net.signal));
    icon.set_pixel_size(config::ICON_SIZE() as i32);
    row.append(&icon);

    // SSID label (bold if known/saved).
    let ssid_label = gtk::Label::new(Some(&net.ssid));
    ssid_label.set_xalign(0.0);
    ssid_label.set_hexpand(true);
    ssid_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    ssid_label.set_max_width_chars(20);
    if net.known {
        ssid_label.add_css_class("wifi-row-known");
    }
    row.append(&ssid_label);

    // Lock indicator for secured networks. Plain text glyph keeps us off the
    // hook for shipping yet another SVG; the user CSS file can replace this
    // via `.wifi-row-secured { … }`.
    if net.secured {
        let lock = gtk::Label::new(Some("\u{1F512}"));
        lock.add_css_class("wifi-row-secured");
        row.append(&lock);
    }

    // Right-aligned signal percentage.
    let signal = gtk::Label::new(Some(&format!("{}%", net.signal)));
    signal.add_css_class("wifi-row-signal");
    row.append(&signal);

    // Click → connect (only when not already connected to this SSID).
    if !is_connected {
        row.set_cursor_from_name(Some("pointer"));
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let ssid = net.ssid.clone();
        click.connect_pressed(move |_, _, _, _| {
            if let Some(s) = sender.borrow().as_ref() {
                s.input(WifiMsg::Connect(ssid.clone()));
            }
        });
        row.add_controller(click);
    }

    row
}

impl NamedWidget for Wifi {
    const NAME: &'static str = "wifi";
}
