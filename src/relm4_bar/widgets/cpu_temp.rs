//! CPU package/die temperature widget. Subscribes to `hub::cpu_temp` and
//! renders a thermometer icon plus a `°C` reading.
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`:
//! cached SVG texture via `OnceLock`, model holds the GTK widgets, watch
//! receiver is bridged into component messages on the GTK main context, and
//! `update` short-circuits when the displayed value is unchanged.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_NAME: &str = "thermometer-symbolic";

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &["cpu-temp-hot", "cpu-temp-warm", "cpu-temp-cool"];

pub struct CpuTemp {
    /// Last-seen temperature in degrees C, kept for the displayed-value
    /// coalescing check in `update`.
    temp: f32,
    grouped: bool,
    /// Held so `update` can re-style the icon when the color band changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum CpuTempMsg {
    Update(f32),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuTemp {
    type Init = WidgetInit;
    type Input = CpuTempMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_NAME),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "0°C",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = CpuTemp {
            temp: 0.0,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        crate::subscribe_into_msg!(hub::cpu_temp::subscribe(), sender, CpuTempMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuTempMsg::Update(temp) => {
                // Coalescing optimisation: skip the GTK property writes when
                // the *displayed* value (rounded integer °C) is unchanged.
                let new_t = temp.round() as i32;
                let old_t = self.temp.round() as i32;
                if new_t == old_t {
                    return;
                }
                self.temp = temp;
                self.label.set_label(&format!("{}°C", new_t));

                // Color band — thresholds match rs-bar (88/75/62 °C). The
                // <62 °C tier maps to `cpu-temp-cool` (default fg in rs-bar);
                // the 62-74 °C and 75-87 °C tiers both map to `cpu-temp-warm`
                // since the spec defines three classes.
                let class = if new_t >= 88 {
                    "cpu-temp-hot"
                } else if new_t >= 75 {
                    "cpu-temp-warm"
                } else {
                    "cpu-temp-cool"
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
        }
    }
}

impl NamedWidget for CpuTemp {
    const NAME: &'static str = "cpu-temp";
}
