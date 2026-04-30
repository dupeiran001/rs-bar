//! Volume widget. Subscribes to `hub::volume` and renders an icon + percent
//! label. Click → opens a popover containing a horizontal slider, a mute
//! toggle, and a sink-selection dropdown.
//!
//! The bar-line view follows the canonical pattern from `cpu_usage.rs`:
//! cached SVG textures via `OnceLock`, model holds the GTK widgets, watch
//! receiver bridged into component messages on the GTK main context, and
//! `update` short-circuits when the displayed value is unchanged.
//!
//! The popover is built once in `init` and re-populated from each Update
//! message: the slider is moved to the new percent, the mute toggle is
//! re-checked, and the sink dropdown's `StringList` is rebuilt only when the
//! set of sinks actually changes (a cheap structural compare). The slider's
//! `value-changed` signal is gated behind a `RefCell<bool>` flag so calls
//! that come from our own updates don't loop back into `set_volume`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::volume::{SinkInfo, VolumeState};

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_HIGH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-high.svg");
const ICON_LOW: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-low.svg");
const ICON_MUTE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/mute.svg");
const ICON_UNMUTE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/unmute.svg");

/// CSS classes for color states. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "volume-muted",
    "volume-low",
    "volume-mid",
    "volume-high",
];

fn cached(path: &'static str) -> &'static gdk::Texture {
    // Tiny per-path texture cache. Each ICON_* constant gets its own static
    // through this macro-free helper; we identify them by pointer equality
    // since the &'static strs are unique values produced by `concat!(env!())`.
    static HIGH: OnceLock<gdk::Texture> = OnceLock::new();
    static LOW: OnceLock<gdk::Texture> = OnceLock::new();
    static MUTE: OnceLock<gdk::Texture> = OnceLock::new();
    static UNMUTE: OnceLock<gdk::Texture> = OnceLock::new();

    let slot = if std::ptr::eq(path, ICON_HIGH) {
        &HIGH
    } else if std::ptr::eq(path, ICON_LOW) {
        &LOW
    } else if std::ptr::eq(path, ICON_MUTE) {
        &MUTE
    } else {
        &UNMUTE
    };
    slot.get_or_init(|| gdk::Texture::from_filename(path).expect("icon load"))
}

/// Install a process-wide CssProvider once that defines the color classes.
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

/// Map a `(percent, muted)` pair to the icon path + CSS class.
fn icon_for(percent: u32, muted: bool) -> (&'static str, &'static str) {
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

pub struct Volume {
    grouped: bool,
    /// Last-applied snapshot, kept for the displayed-value coalescing check.
    state: VolumeState,
    /// Held so `update` can swap the paintable + class on volume changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the percent text.
    label: gtk::Label,
    /// Popover widgets. Mutated on every state update.
    popover_slider: gtk::Scale,
    popover_mute: gtk::ToggleButton,
    popover_dropdown: gtk::DropDown,
    /// String list backing the dropdown — rebuilt when sinks change.
    popover_dropdown_model: gtk::StringList,
    /// The `name`s corresponding to each entry in `popover_dropdown_model`,
    /// in the same order. Used to map a dropdown selection back to a
    /// `set_default_sink` call.
    popover_dropdown_names: Rc<RefCell<Vec<String>>>,
    /// Set to true while we're applying an external state update, so the
    /// signal handlers know to skip pushing the change back through the
    /// hub command API.
    suppress_signals: Rc<RefCell<bool>>,
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
                set_paintable: Some(cached(ICON_UNMUTE)),
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

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        popover_box.set_margin_top(8);
        popover_box.set_margin_bottom(8);
        popover_box.set_margin_start(8);
        popover_box.set_margin_end(8);

        // Volume slider (0..=100, integer step).
        let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        slider.set_width_request(220);
        slider.set_hexpand(true);
        slider.set_draw_value(true);
        slider.set_value_pos(gtk::PositionType::Right);
        popover_box.append(&slider);

        // Mute toggle.
        let mute = gtk::ToggleButton::with_label("Mute");
        mute.add_css_class("volume-mute-toggle");
        popover_box.append(&mute);

        // Sink selection dropdown — backed by a StringList we rebuild as the
        // sink set changes.
        let dropdown_model = gtk::StringList::new(&[]);
        let dropdown = gtk::DropDown::builder()
            .model(&dropdown_model)
            .build();
        popover_box.append(&dropdown);

        popover.set_child(Some(&popover_box));

        // ── Model ──────────────────────────────────────────────────────
        let model = Volume {
            grouped: init.grouped,
            // `Default::default()` so the first Update message will always
            // be applied (the seeded state.percent/muted differ from the
            // hub's first publish in the typical case).
            state: VolumeState::default(),
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
            popover_slider: slider.clone(),
            popover_mute: mute.clone(),
            popover_dropdown: dropdown.clone(),
            popover_dropdown_model: dropdown_model.clone(),
            popover_dropdown_names: Rc::new(RefCell::new(Vec::new())),
            suppress_signals: Rc::new(RefCell::new(false)),
        };

        capsule(&root, model.grouped);
        root.set_cursor_from_name(Some("pointer"));

        // ── Signal wiring ──────────────────────────────────────────────
        // Slider → set_volume (suppressed when we're applying an external
        // update to avoid a feedback loop).
        {
            let suppress = model.suppress_signals.clone();
            slider.connect_value_changed(move |s| {
                if *suppress.borrow() {
                    return;
                }
                hub::volume::set_volume(s.value().round() as u32);
            });
        }

        // Mute toggle → toggle_mute. Same suppression flag.
        {
            let suppress = model.suppress_signals.clone();
            mute.connect_toggled(move |_| {
                if *suppress.borrow() {
                    return;
                }
                hub::volume::toggle_mute();
            });
        }

        // Dropdown → set_default_sink. We listen to `selected-item` rather
        // than `selected` so we don't fire when the model is rebuilt to a
        // different size.
        {
            let suppress = model.suppress_signals.clone();
            let names = model.popover_dropdown_names.clone();
            dropdown.connect_selected_notify(move |dd| {
                if *suppress.borrow() {
                    return;
                }
                let idx = dd.selected() as usize;
                let name = names.borrow().get(idx).cloned();
                if let Some(name) = name {
                    hub::volume::set_default_sink(&name);
                }
            });
        }

        // Click on the bar widget → popup the popover.
        {
            let popover = popover.clone();
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_PRIMARY);
            click.connect_pressed(move |_, _, _, _| popover.popup());
            root.add_controller(click);
        }

        // Scroll-wheel over the bar widget → ±5% volume, like rs-bar.
        {
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL,
            );
            scroll.connect_scroll(move |_, _dx, dy| {
                // dy < 0 means scroll up (away from user).
                let cur_pct = {
                    let mut rx = hub::volume::subscribe();
                    let s = rx.borrow_and_update().clone();
                    s.percent
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

        // ── Subscription ────────────────────────────────────────────────
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
                // Coalescing: skip GTK writes when nothing visible changed.
                let icon_changed = {
                    let (path_old, _) = icon_for(self.state.percent, self.state.muted);
                    let (path_new, _) = icon_for(new.percent, new.muted);
                    !std::ptr::eq(path_old, path_new)
                };
                let pct_changed = self.state.percent != new.percent;
                let muted_changed = self.state.muted != new.muted;
                let default_changed = self.state.default_sink != new.default_sink;
                let sinks_changed = !sinks_equal(&self.state.sinks, &new.sinks);

                if !icon_changed
                    && !pct_changed
                    && !muted_changed
                    && !default_changed
                    && !sinks_changed
                {
                    return;
                }

                // Suppress signal handlers while we mutate the popover
                // controls so they don't bounce changes back through the
                // hub command API.
                *self.suppress_signals.borrow_mut() = true;

                if icon_changed || muted_changed || pct_changed {
                    let (path, class) = icon_for(new.percent, new.muted);
                    self.icon.set_paintable(Some(cached(path)));
                    set_exclusive_class(&self.icon, class, COLOR_CLASSES);
                    set_exclusive_class(&self.label, class, COLOR_CLASSES);
                    self.label.set_label(&format!("{:>3}%", new.percent.min(999)));
                }

                if pct_changed {
                    let target = new.percent.min(100) as f64;
                    if (self.popover_slider.value() - target).abs() > f64::EPSILON {
                        self.popover_slider.set_value(target);
                    }
                }

                if muted_changed && self.popover_mute.is_active() != new.muted {
                    self.popover_mute.set_active(new.muted);
                }

                if sinks_changed {
                    rebuild_dropdown_model(
                        &self.popover_dropdown_model,
                        &self.popover_dropdown_names,
                        &new.sinks,
                    );
                }

                if sinks_changed || default_changed {
                    // Move the dropdown selection to the new default sink.
                    if let Some(idx) = self
                        .popover_dropdown_names
                        .borrow()
                        .iter()
                        .position(|n| n == &new.default_sink)
                    {
                        if self.popover_dropdown.selected() != idx as u32 {
                            self.popover_dropdown.set_selected(idx as u32);
                        }
                    }
                }

                self.state = new;
                *self.suppress_signals.borrow_mut() = false;
            }
        }
    }
}

impl NamedWidget for Volume {
    const NAME: &'static str = "volume";
}

/// Cheap structural compare on the sink list. The hub already coalesces
/// identical states, but we re-check on the widget side because the UI
/// rebuild (StringList replacement) is more expensive than the comparison.
fn sinks_equal(a: &[SinkInfo], b: &[SinkInfo]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.name == y.name && x.description == y.description)
}

/// Replace every entry in `model` with descriptions from `sinks`, and
/// mirror the ordering into `names`. Uses `splice(0, old_len, new)` for an
/// atomic "clear and replace" — the dropdown will reset its `selected`
/// index after this, which the caller then re-points at the new default.
fn rebuild_dropdown_model(
    model: &gtk::StringList,
    names: &Rc<RefCell<Vec<String>>>,
    sinks: &[SinkInfo],
) {
    let mut names_mut = names.borrow_mut();
    let old_len = names_mut.len() as u32;
    let descriptions: Vec<&str> = sinks.iter().map(|s| s.description.as_str()).collect();
    model.splice(0, old_len, &descriptions);
    names_mut.clear();
    names_mut.extend(sinks.iter().map(|s| s.name.clone()));
}
