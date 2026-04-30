//! Battery widget. Subscribes to `hub::battery` and renders an icon + percent.
//!
//! When `BatteryState::present` is false (no battery detected, e.g. desktop)
//! the widget hides itself by setting the root invisible — it still exists in
//! the bar's layout but takes up no space, so the bar's right zone shifts
//! cleanly. Otherwise it renders a battery icon (charging variant when AC is
//! plugged in) plus the integer capacity percentage.
//!
//! Click → opens a `gtk::Popover` showing the status, estimated time
//! remaining, charge cycles, and health percentage. Mirrors the popover
//! pattern from `clock.rs`.
//!
//! CSS color bands (mutually exclusive on icon and label):
//!   * `battery-charging` — currently charging
//!   * `battery-crit`     — ≤ 10% and not charging
//!   * `battery-low`      — ≤ 20% and not charging
//!   * `battery-norm`     — discharging, > 20%
//!   * `battery-full`     — full or not-charging at high capacity
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::battery::{BatteryState, BatteryStatus};

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_BATTERY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/battery.svg");
const ICON_CHARGING: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/icons/battery-charging.svg"
);

/// CSS classes for battery color bands. `set_exclusive_class` strips the
/// others before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "battery-charging",
    "battery-crit",
    "battery-low",
    "battery-norm",
    "battery-full",
];

/// Cache the discharging/charging textures so the icon swap on plug-in is
/// just a `set_paintable` call rather than a re-decode of the SVG.
fn cached_texture(charging: bool) -> &'static gdk::Texture {
    static T_BAT: OnceLock<gdk::Texture> = OnceLock::new();
    static T_CHG: OnceLock<gdk::Texture> = OnceLock::new();
    if charging {
        T_CHG.get_or_init(|| gdk::Texture::from_filename(ICON_CHARGING).expect("icon load"))
    } else {
        T_BAT.get_or_init(|| gdk::Texture::from_filename(ICON_BATTERY).expect("icon load"))
    }
}

/// Install a process-wide CssProvider once that defines the battery color
/// bands. Mounted at `STYLE_PROVIDER_PRIORITY_APPLICATION + 1` so it sits just
/// above the global theme provider but below user overrides.
fn ensure_css() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let css = "\
            .battery-charging { color: @rs_green; }\n\
            .battery-full     { color: @rs_green; }\n\
            .battery-norm     { color: @rs_fg; }\n\
            .battery-low      { color: @rs_orange; }\n\
            .battery-crit     { color: @rs_red; }\n\
            .battery-popover-status { font-weight: 700; }\n\
        ";
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
    });
}

/// Pick the color class for a given state. Charging always wins so that an
/// almost-empty plugged-in battery still displays as healthy/charging rather
/// than as a critical warning.
fn class_for(state: &BatteryState) -> &'static str {
    if matches!(state.status, BatteryStatus::Charging) {
        "battery-charging"
    } else if matches!(state.status, BatteryStatus::Full) {
        "battery-full"
    } else if state.capacity_pct <= 10 {
        "battery-crit"
    } else if state.capacity_pct <= 20 {
        "battery-low"
    } else if matches!(state.status, BatteryStatus::NotCharging) && state.capacity_pct >= 90 {
        "battery-full"
    } else {
        "battery-norm"
    }
}

fn fmt_time_remaining(minutes: Option<u32>) -> String {
    match minutes {
        Some(m) if m > 0 => {
            let h = m / 60;
            let mm = m % 60;
            if h > 0 {
                format!("{}h {}m", h, mm)
            } else {
                format!("{}m", mm)
            }
        }
        _ => "—".to_string(),
    }
}

fn fmt_status(status: BatteryStatus) -> &'static str {
    match status {
        BatteryStatus::Charging => "Charging",
        BatteryStatus::Discharging => "Discharging",
        BatteryStatus::Full => "Full",
        BatteryStatus::NotCharging => "Not charging",
        BatteryStatus::Unknown => "Unknown",
    }
}

pub struct Battery {
    /// Last-seen state, kept for the displayed-value coalescing check.
    last: BatteryState,
    grouped: bool,
    /// Root box; held so `update` can toggle visibility when the battery
    /// becomes (un)present.
    root: gtk::Box,
    /// Held so `update` can swap textures and re-style the icon.
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
    /// Popover labels — held for refresh on each state change.
    popover_status: gtk::Label,
    popover_time: gtk::Label,
    popover_cycles: gtk::Label,
    popover_health: gtk::Label,
}

#[derive(Debug)]
pub enum BatteryMsg {
    Update(BatteryState),
}

#[relm4::component(pub)]
impl SimpleComponent for Battery {
    type Init = WidgetInit;
    type Input = BatteryMsg;
    type Output = ();

    view! {
        #[name = "root_box"]
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            // Hidden by default until the first hub publish tells us whether
            // a battery is present. Without this, desktops would briefly show
            // an empty capsule on startup.
            set_visible: false,
            #[name = "icon"]
            gtk::Image {
                set_paintable: Some(cached_texture(false)),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "0%",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        ensure_css();
        let widgets = view_output!();

        // ── Popover with status / time / cycles / health ──────────────
        let popover = gtk::Popover::builder().autohide(true).build();
        popover.add_css_class("battery-popover");
        popover.set_parent(&root);

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let popover_status = gtk::Label::new(Some("—"));
        popover_status.add_css_class("battery-popover-status");
        popover_status.set_xalign(0.0);
        popover_box.append(&popover_status);

        let popover_time = gtk::Label::new(Some("Time remaining: —"));
        popover_time.set_xalign(0.0);
        popover_box.append(&popover_time);

        let popover_cycles = gtk::Label::new(Some("Cycles: —"));
        popover_cycles.set_xalign(0.0);
        popover_box.append(&popover_cycles);

        let popover_health = gtk::Label::new(Some("Health: —"));
        popover_health.set_xalign(0.0);
        popover_box.append(&popover_health);

        popover.set_child(Some(&popover_box));

        // Click → open popover.
        let click = gtk::GestureClick::new();
        let popover_for_click = popover.clone();
        click.connect_pressed(move |_, _, _, _| popover_for_click.popup());
        root.add_controller(click);

        let model = Battery {
            last: BatteryState::default(),
            grouped: init.grouped,
            root: widgets.root_box.clone(),
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
            popover_status,
            popover_time,
            popover_cycles,
            popover_health,
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<BatteryState> into messages.
        let mut rx = hub::battery::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Send the initial value so the visibility/state is set on first
            // tick rather than after the first hub publish.
            let initial = rx.borrow_and_update().clone();
            s.input(BatteryMsg::Update(initial));
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().clone();
                s.input(BatteryMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BatteryMsg::Update(state) => {
                // Coalescing: if nothing observable changed, skip every GTK
                // write. `BatteryState: PartialEq` makes this a single check.
                if state == self.last {
                    return;
                }

                // Visibility — hide the entire root when no battery is present.
                if !state.present {
                    self.root.set_visible(false);
                    self.last = state;
                    return;
                }
                self.root.set_visible(true);

                // Bar-line: icon (charging variant when applicable) + percent.
                let charging = matches!(state.status, BatteryStatus::Charging);
                let was_charging = matches!(self.last.status, BatteryStatus::Charging);
                if charging != was_charging || !self.last.present {
                    self.icon.set_paintable(Some(cached_texture(charging)));
                }
                if state.capacity_pct != self.last.capacity_pct || !self.last.present {
                    self.label.set_label(&format!("{}%", state.capacity_pct));
                }
                let class = class_for(&state);
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);

                // Popover labels — cheap to write, refreshed on every change.
                self.popover_status.set_label(&format!(
                    "{} — {}%",
                    fmt_status(state.status),
                    state.capacity_pct
                ));
                self.popover_time.set_label(&format!(
                    "Time remaining: {}",
                    fmt_time_remaining(state.time_remaining_minutes)
                ));
                self.popover_cycles.set_label(&format!(
                    "Cycles: {}",
                    state
                        .cycles
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "—".into())
                ));
                self.popover_health.set_label(&format!(
                    "Health: {}",
                    state
                        .health_pct
                        .map(|h| format!("{}%", h))
                        .unwrap_or_else(|| "—".into())
                ));

                self.last = state;
            }
        }
    }
}

impl NamedWidget for Battery {
    const NAME: &'static str = "battery";
}
