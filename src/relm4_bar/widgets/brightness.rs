//! Brightness widget. Subscribes to `hub::brightness` and renders an icon +
//! percent label. Click → opens a popover with a horizontal slider.
//! Scroll-wheel over the bar widget bumps brightness up/down.
//!
//! Follows the canonical pattern from `cpu_usage.rs` for the bar-line view
//! and the popover pattern from `volume.rs` for the slider — without mute or
//! sink dropdown, since brightness has only a single level. The slider's
//! `value-changed` signal is gated behind a `RefCell<bool>` flag so calls
//! that come from our own updates don't loop back into command invocations.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::subscribe_into_msg;

use super::popover::BarPopover;
use super::util::SuppressGuard;
use super::{NamedWidget, WidgetInit, capsule, capsule_interactive, set_exclusive_class};

const ICON_HIGH: &str = "brightness-high-symbolic";
const ICON_LOW: &str = "brightness-low-symbolic";

/// CSS classes toggled on the icon (and the matching label) so themes can
/// style the two states differently.
const COLOR_CLASSES: &[&str] = &["brightness-low", "brightness-high"];

/// Map a percent value to the icon name + CSS class.
fn icon_for(percent: u32) -> (&'static str, &'static str) {
    if percent < 50 {
        (ICON_LOW, "brightness-low")
    } else {
        (ICON_HIGH, "brightness-high")
    }
}

pub struct Brightness {
    grouped: bool,
    /// Last-applied percent, kept for the displayed-value coalescing check.
    percent: u32,
    /// Held so `update` can swap the paintable + class on changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the percent text.
    label: gtk::Label,
    /// Popover slider, mutated on every state update.
    popover_slider: gtk::Scale,
    /// Set to true while we're applying an external state update so the
    /// slider's value-changed signal doesn't push our own change back
    /// through the hub command API.
    suppress_signals: Rc<RefCell<bool>>,
}

#[derive(Debug)]
pub enum BrightnessMsg {
    Update(u32),
}

#[relm4::component(pub)]
impl SimpleComponent for Brightness {
    type Init = WidgetInit;
    type Input = BrightnessMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_LOW),
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
        let widgets = view_output!();

        // ── Popover content ────────────────────────────────────────────
        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        popover_box.set_margin_top(8);
        popover_box.set_margin_bottom(8);
        popover_box.set_margin_start(8);
        popover_box.set_margin_end(8);

        let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        slider.set_width_request(220);
        slider.set_hexpand(true);
        slider.set_draw_value(true);
        slider.set_value_pos(gtk::PositionType::Right);
        popover_box.append(&slider);

        let popover = BarPopover::builder(&root, "brightness-popover").build(&popover_box);

        // ── Model ──────────────────────────────────────────────────────
        let model = Brightness {
            grouped: init.grouped,
            // Seed with a sentinel that's guaranteed to differ from the
            // hub's first value so the initial update always applies.
            percent: u32::MAX,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
            popover_slider: slider.clone(),
            suppress_signals: Rc::new(RefCell::new(false)),
        };

        capsule(&root, model.grouped);
        capsule_interactive(&root, model.grouped);
        root.set_cursor_from_name(Some("pointer"));

        // ── Signal wiring ──────────────────────────────────────────────
        // Slider drag → step the brightness toward the target. We don't have
        // a "set absolute percent" command, only up/down, so we issue the
        // appropriate one-shot bump and let the next hub poll catch up. The
        // popover_slider value gets corrected back to the real value on the
        // following Update message, which is fine.
        {
            let suppress = model.suppress_signals.clone();
            let last_target: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));
            slider.connect_value_changed(move |s| {
                if *suppress.borrow() {
                    *last_target.borrow_mut() = s.value();
                    return;
                }
                let new_v = s.value();
                let old_v = *last_target.borrow();
                if (new_v - old_v).abs() < f64::EPSILON {
                    return;
                }
                if new_v > old_v {
                    hub::brightness::brightness_up();
                } else {
                    hub::brightness::brightness_down();
                }
                *last_target.borrow_mut() = new_v;
            });
        }

        // Click on the bar widget → popup the popover.
        popover.attach_click(&root);

        // Scroll-wheel over the bar widget → bump up/down. Each scroll tick
        // issues exactly one BRIGHTNESS_UP/DOWN_CMD() invocation; the
        // configured command itself defines the step size.
        {
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::VERTICAL,
            );
            scroll.connect_scroll(move |_, _dx, dy| {
                if dy < 0.0 {
                    hub::brightness::brightness_up();
                } else if dy > 0.0 {
                    hub::brightness::brightness_down();
                }
                glib::Propagation::Stop
            });
            root.add_controller(scroll);
        }

        // ── Subscription ────────────────────────────────────────────────
        subscribe_into_msg!(hub::brightness::subscribe(), sender, BrightnessMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BrightnessMsg::Update(new) => {
                let new = new.min(100);
                let icon_changed = {
                    let (name_old, _) = icon_for(self.percent.min(100));
                    let (name_new, _) = icon_for(new);
                    !std::ptr::eq(name_old, name_new)
                };
                let pct_changed = self.percent != new;
                if !icon_changed && !pct_changed {
                    return;
                }

                // Suppress signal handlers while we mutate the slider so it
                // doesn't bounce a fake "user moved the slider" change back
                // through the hub command API.
                let _suppress = SuppressGuard::new(&self.suppress_signals);

                if icon_changed || pct_changed {
                    let (name, class) = icon_for(new);
                    self.icon.set_icon_name(Some(name));
                    set_exclusive_class(&self.icon, class, COLOR_CLASSES);
                    set_exclusive_class(&self.label, class, COLOR_CLASSES);
                    self.label.set_label(&format!("{:>3}%", new));
                }

                if pct_changed {
                    let target = new as f64;
                    if (self.popover_slider.value() - target).abs() > f64::EPSILON {
                        self.popover_slider.set_value(target);
                    }
                }

                self.percent = new;
            }
        }
    }
}

impl NamedWidget for Brightness {
    const NAME: &'static str = "brightness";
}
