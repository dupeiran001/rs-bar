//! Volume widget. Subscribes to `hub::volume` and renders an icon + percent
//! label. Click → opens a popover with two sections (output / input), each
//! with a slider, mute toggle, and device dropdown. Layout and slider styling
//! follow the Noctalia shell aesthetic — minimal trough, small accent
//! handle, generous breathing room between sections.
//!
//! The bar-line view follows the canonical pattern from `cpu_usage.rs`:
//! cached SVG textures via `OnceLock`, model holds the GTK widgets, watch
//! receiver bridged into component messages on the GTK main context, and
//! `update` short-circuits when nothing visible changed.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::volume::{DeviceInfo, VolumeState};

use super::{NamedWidget, WidgetInit, capsule, capsule_interactive, set_exclusive_class};

const ICON_HIGH: &str = "volume-high-symbolic";
const ICON_LOW: &str = "volume-low-symbolic";
const ICON_MUTE: &str = "mute-symbolic";
const ICON_UNMUTE: &str = "unmute-symbolic";
const ICON_MIC_ON: &str = "mic-on-symbolic";
const ICON_MIC_OFF: &str = "mic-off-symbolic";

/// CSS classes for color states. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "volume-muted",
    "volume-low",
    "volume-mid",
    "volume-high",
];

fn ensure_css() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let css = "\
            .volume-muted { color: @rs_fg_dark; }\n\
            .volume-low   { color: @rs_fg_dark; }\n\
            .volume-mid   { color: @rs_fg; }\n\
            .volume-high  { color: @rs_fg; }\n\
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

fn icon_for_output(percent: u32, muted: bool) -> (&'static str, &'static str) {
    if muted {
        (ICON_MUTE, "volume-muted")
    } else if percent == 0 {
        (ICON_UNMUTE, "volume-muted")
    } else if percent < 33 {
        (ICON_LOW, "volume-low")
    } else if percent < 66 {
        (ICON_LOW, "volume-mid")
    } else {
        (ICON_HIGH, "volume-high")
    }
}

fn icon_for_mic(muted: bool) -> &'static str {
    if muted { ICON_MIC_OFF } else { ICON_MIC_ON }
}

/// Holds all the popover-section widgets for one audio direction (output or
/// input). Same shape — only the icon and command-API targets differ.
struct DeviceSection {
    icon: gtk::Image,
    slider: gtk::Scale,
    mute: gtk::ToggleButton,
    pct: gtk::Label,
    dropdown: gtk::DropDown,
    dropdown_model: gtk::StringList,
    dropdown_names: Rc<RefCell<Vec<String>>>,
}

pub struct Volume {
    grouped: bool,
    state: VolumeState,
    /// Bar-line icon + label.
    bar_icon: gtk::Image,
    bar_label: gtk::Label,
    /// Popover sections.
    out_section: DeviceSection,
    in_section: DeviceSection,
    /// Set to true while we're applying an external state update so the
    /// signal handlers know to skip pushing the change back through the hub.
    suppress: Rc<RefCell<bool>>,
}

#[derive(Debug)]
pub enum VolumeMsg {
    Update(VolumeState),
}

#[relm4::component(pub)]
impl SimpleComponent for Volume {
    type Init = WidgetInit;
    type Input = VolumeMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_UNMUTE),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "  0%",
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

        // ── Popover scaffolding ────────────────────────────────────────
        let popover = gtk::Popover::builder().autohide(true).build();
        popover.add_css_class("volume-popover");
        popover.set_parent(&root);

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 14);
        popover_box.add_css_class("noctalia-section-box");

        let suppress = Rc::new(RefCell::new(false));

        let out_section = build_section(
            &popover_box,
            "Output",
            ICON_HIGH,
            "volume-output-section",
            suppress.clone(),
            VolumeChannel::Output,
        );

        // Visual divider between output and input.
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("noctalia-divider");
        divider.set_height_request(1);
        popover_box.append(&divider);

        let in_section = build_section(
            &popover_box,
            "Input",
            ICON_MIC_ON,
            "volume-input-section",
            suppress.clone(),
            VolumeChannel::Input,
        );

        popover.set_child(Some(&popover_box));

        // ── Model ──────────────────────────────────────────────────────
        let model = Volume {
            grouped: init.grouped,
            state: VolumeState::default(),
            bar_icon: widgets.icon.clone(),
            bar_label: widgets.label.clone(),
            out_section,
            in_section,
            suppress,
        };

        capsule(&root, model.grouped);
        capsule_interactive(&root, model.grouped);

        // ── Bar-widget interactions ────────────────────────────────────
        // Click → popup. Scroll → ±5% on output volume.
        {
            let popover = popover.clone();
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_PRIMARY);
            click.connect_pressed(move |_, _, _, _| popover.popup());
            root.add_controller(click);
        }
        {
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL,
            );
            scroll.connect_scroll(move |_, _dx, dy| {
                let cur_pct = {
                    let mut rx = hub::volume::subscribe();
                    rx.borrow_and_update().percent
                };
                let new_pct = if dy < 0.0 {
                    (cur_pct + 5).min(100)
                } else {
                    cur_pct.saturating_sub(5)
                };
                hub::volume::set_volume(new_pct);
                glib::Propagation::Stop
            });
            root.add_controller(scroll);
        }

        // ── Hub subscription ───────────────────────────────────────────
        let mut rx = hub::volume::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            let initial = rx.borrow_and_update().clone();
            s.input(VolumeMsg::Update(initial));
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().clone();
                s.input(VolumeMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            VolumeMsg::Update(new) => {
                *self.suppress.borrow_mut() = true;

                // ── Bar line (output side only) ─────────────────────────
                let (name, class) = icon_for_output(new.percent, new.muted);
                self.bar_icon.set_icon_name(Some(name));
                set_exclusive_class(&self.bar_icon, class, COLOR_CLASSES);
                set_exclusive_class(&self.bar_label, class, COLOR_CLASSES);
                self.bar_label
                    .set_label(&format!("{:>3}%", new.percent.min(999)));

                // ── Output section ──────────────────────────────────────
                apply_to_section(
                    &self.out_section,
                    new.percent,
                    new.muted,
                    icon_for_output(new.percent, new.muted).0,
                    &new.default_sink,
                    &new.sinks,
                );

                // ── Input section ───────────────────────────────────────
                apply_to_section(
                    &self.in_section,
                    new.mic_percent,
                    new.mic_muted,
                    icon_for_mic(new.mic_muted),
                    &new.default_source,
                    &new.sources,
                );

                self.state = new;
                *self.suppress.borrow_mut() = false;
            }
        }
    }
}

/// Which side of the audio plumbing a `DeviceSection` controls.
#[derive(Clone, Copy)]
enum VolumeChannel {
    Output,
    Input,
}

impl VolumeChannel {
    fn set_volume(self, pct: u32) {
        match self {
            VolumeChannel::Output => hub::volume::set_volume(pct),
            VolumeChannel::Input => hub::volume::set_mic_volume(pct),
        }
    }
    fn toggle_mute(self) {
        match self {
            VolumeChannel::Output => hub::volume::toggle_mute(),
            VolumeChannel::Input => hub::volume::toggle_mic_mute(),
        }
    }
    fn set_default(self, name: &str) {
        match self {
            VolumeChannel::Output => hub::volume::set_default_sink(name),
            VolumeChannel::Input => hub::volume::set_default_source(name),
        }
    }
}

/// Build one device section (header + slider row + dropdown), append it to
/// `parent`, wire up its signal handlers, and return the held widgets.
fn build_section(
    parent: &gtk::Box,
    title: &str,
    initial_icon: &str,
    css_class: &str,
    suppress: Rc<RefCell<bool>>,
    channel: VolumeChannel,
) -> DeviceSection {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class(css_class);
    section.add_css_class("noctalia-section");

    // Header: small label like "OUTPUT" — Noctalia uses uppercase tracking.
    let header = gtk::Label::new(Some(&title.to_uppercase()));
    header.set_xalign(0.0);
    header.add_css_class("noctalia-section-header");
    section.append(&header);

    // Row: icon, slider, percent label, mute button.
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let icon = gtk::Image::builder()
        .icon_name(initial_icon)
        .pixel_size(16)
        .build();
    row.append(&icon);

    let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    slider.set_width_request(220);
    slider.set_hexpand(true);
    slider.set_draw_value(false);
    slider.add_css_class("noctalia-slider");
    row.append(&slider);

    let pct = gtk::Label::new(Some("  0%"));
    pct.add_css_class("noctalia-pct");
    pct.set_width_chars(4);
    pct.set_xalign(1.0);
    row.append(&pct);

    let mute = gtk::ToggleButton::new();
    mute.set_child(Some(&gtk::Image::from_icon_name(initial_icon)));
    mute.add_css_class("noctalia-mute");
    mute.set_tooltip_text(Some("Toggle mute"));
    row.append(&mute);

    section.append(&row);

    // Dropdown for the device list.
    let dropdown_model = gtk::StringList::new(&[]);
    let dropdown = gtk::DropDown::builder()
        .model(&dropdown_model)
        .build();
    dropdown.add_css_class("noctalia-dropdown");
    section.append(&dropdown);

    parent.append(&section);

    let dropdown_names: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    // Wire signals.
    {
        let suppress = suppress.clone();
        let pct_label = pct.clone();
        slider.connect_value_changed(move |s| {
            let v = s.value().round() as u32;
            // Always update the inline pct text so dragging feels live.
            pct_label.set_label(&format!("{:>3}%", v.min(999)));
            if *suppress.borrow() {
                return;
            }
            channel.set_volume(v);
        });
    }
    {
        let suppress = suppress.clone();
        mute.connect_toggled(move |_| {
            if *suppress.borrow() {
                return;
            }
            channel.toggle_mute();
        });
    }
    {
        let suppress = suppress.clone();
        let names = dropdown_names.clone();
        dropdown.connect_selected_notify(move |dd| {
            if *suppress.borrow() {
                return;
            }
            let idx = dd.selected() as usize;
            let name = names.borrow().get(idx).cloned();
            if let Some(name) = name {
                channel.set_default(&name);
            }
        });
    }

    DeviceSection {
        icon,
        slider,
        mute,
        pct,
        dropdown,
        dropdown_model,
        dropdown_names,
    }
}

/// Apply a `(percent, muted, icon_name, default_name, devices)` snapshot to
/// a section's widgets without firing the signal handlers (caller must hold
/// `suppress = true`).
fn apply_to_section(
    s: &DeviceSection,
    percent: u32,
    muted: bool,
    icon_name: &str,
    default_name: &str,
    devices: &[DeviceInfo],
) {
    s.icon.set_icon_name(Some(icon_name));
    if let Some(child) = s.mute.child() {
        if let Ok(image) = child.downcast::<gtk::Image>() {
            image.set_icon_name(Some(icon_name));
        }
    }

    let target = percent.min(100) as f64;
    if (s.slider.value() - target).abs() > f64::EPSILON {
        s.slider.set_value(target);
    }
    s.pct.set_label(&format!("{:>3}%", percent.min(999)));

    if s.mute.is_active() != muted {
        s.mute.set_active(muted);
    }

    // Rebuild dropdown only if the device list changed.
    let need_rebuild = {
        let names = s.dropdown_names.borrow();
        names.len() != devices.len()
            || names.iter().zip(devices.iter()).any(|(n, d)| n != &d.name)
    };
    if need_rebuild {
        let mut names_mut = s.dropdown_names.borrow_mut();
        let old_len = names_mut.len() as u32;
        let descriptions: Vec<&str> = devices.iter().map(|d| d.description.as_str()).collect();
        s.dropdown_model.splice(0, old_len, &descriptions);
        names_mut.clear();
        names_mut.extend(devices.iter().map(|d| d.name.clone()));
    }

    if let Some(idx) = s
        .dropdown_names
        .borrow()
        .iter()
        .position(|n| n == default_name)
    {
        if s.dropdown.selected() != idx as u32 {
            s.dropdown.set_selected(idx as u32);
        }
    }
}

impl NamedWidget for Volume {
    const NAME: &'static str = "volume";
}
