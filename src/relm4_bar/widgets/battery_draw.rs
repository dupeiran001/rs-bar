//! Battery power-draw widget. Subscribes to `hub::power_draw` and renders an
//! icon + the current battery watts.
//!
//! The hub publishes a coalesced `PowerDrawSample` whose `battery_w` field is
//! `None` when no battery was detected at first-subscriber time. In that case
//! this widget hides itself (the root box is set invisible). When a value is
//! present, it renders `X.XW` next to a battery icon, mirroring the rs-bar
//! gpui_bar `BatteryDraw` widget.
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_NAME: &str = "battery-symbolic";

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "battery-draw-crit",
    "battery-draw-warn",
    "battery-draw-norm",
    "battery-draw-dim",
];

/// Quantise a float watts value to the precision of its display
/// (one decimal place: `X.X`). Used to skip redundant paints when the
/// displayed string would be identical.
fn quantise_watts(w: f64) -> i32 {
    (w * 10.0).round() as i32
}

pub struct BatteryDraw {
    /// Last-seen displayed watts, kept for the coalescing check. `None`
    /// before the first reading or when no battery is present.
    watts: Option<f64>,
    grouped: bool,
    /// Held so `update` can toggle visibility when the hub starts/stops
    /// publishing a battery value.
    root: gtk::Box,
    /// Held so `update` can re-style the icon when the color band changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum BatteryDrawMsg {
    Update(Option<f64>),
}

#[relm4::component(pub)]
impl SimpleComponent for BatteryDraw {
    type Init = WidgetInit;
    type Input = BatteryDrawMsg;
    type Output = ();

    view! {
        #[name = "root_box"]
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            set_visible: false,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_NAME),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "0.0W",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = BatteryDraw {
            watts: None,
            grouped: init.grouped,
            root: widgets.root_box.clone(),
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        crate::subscribe_into_msg!(
            hub::power_draw::subscribe(),
            sender,
            |sample: hub::power_draw::PowerDrawSample| { BatteryDrawMsg::Update(sample.battery_w) }
        );

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BatteryDrawMsg::Update(watts) => {
                // Coalescing: skip GTK writes when the displayed value is
                // unchanged. Quantise to one decimal place to match the
                // `{:.1}W` render precision.
                let same = match (self.watts, watts) {
                    (Some(a), Some(b)) => quantise_watts(a) == quantise_watts(b),
                    (None, None) => true,
                    _ => false,
                };
                if same {
                    return;
                }
                self.watts = watts;

                match watts {
                    None => {
                        // Battery hardware not detected (or disappeared) —
                        // hide the whole row so it occupies no space.
                        self.root.set_visible(false);
                    }
                    Some(w) => {
                        self.root.set_visible(true);
                        self.label.set_label(&format!("{:.1}W", w));

                        // Color band on the magnitude of discharge/charge.
                        // Higher watts → more attention-grabbing color.
                        let class = if w >= 30.0 {
                            "battery-draw-crit"
                        } else if w >= 15.0 {
                            "battery-draw-warn"
                        } else if w >= 5.0 {
                            "battery-draw-norm"
                        } else {
                            "battery-draw-dim"
                        };
                        set_exclusive_class(&self.label, class, COLOR_CLASSES);
                        set_exclusive_class(&self.icon, class, COLOR_CLASSES);
                    }
                }
            }
        }
    }
}

impl NamedWidget for BatteryDraw {
    const NAME: &'static str = "battery-draw";
}
