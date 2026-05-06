//! GPU busy-percent widget. Subscribes to `hub::gpu_busy` and renders a
//! vendor-specific icon plus the current busy percentage.
//!
//! Mirrors the canonical `cpu_usage` pattern (see that file for full notes):
//! cached `gdk::Texture`s parsed once per vendor, displayed-value coalescing
//! in `update`, and CSS color bands applied via `set_exclusive_class`.
//!
//! The chosen icon depends on `sample.vendor` (AMD / Intel / NVIDIA / generic).
//! Vendor changes are essentially never observed at runtime, but we still
//! handle them by swapping the icon's paintable from one of four module-level
//! `OnceLock<gdk::Texture>` slots — never reloading from disk on a hot path.
//!
//! When no readable GPU was detected (`busy_pct == None` with the default
//! `Unknown` vendor before any successful read), the widget hides itself with
//! `set_visible(false)`. This matches rs-bar's behaviour of rendering an
//! empty capsule (effectively invisible) when the source is absent.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::gpu_busy::{GpuBusySample, GpuVendor};

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one.
const COLOR_CLASSES: &[&str] = &[
    "gpu-busy-crit",
    "gpu-busy-warn",
    "gpu-busy-norm",
    "gpu-busy-dim",
];

/// Map a vendor to its symbolic icon name (registered via the IconTheme
/// search path; SVGs use `fill="currentColor"` for live recoloring).
fn vendor_icon_name(vendor: GpuVendor) -> &'static str {
    match vendor {
        GpuVendor::Amd => "amd-radeon-symbolic",
        GpuVendor::Intel => "intel-arc-gpu-symbolic",
        GpuVendor::Nvidia => "nvidia-gpu-symbolic",
        GpuVendor::Unknown => "gpu-busy-symbolic",
    }
}

pub struct GpuBusy {
    /// Last displayed percentage (`None` until the first successful read).
    pct: Option<u32>,
    /// Last seen vendor — used to skip re-setting the paintable when only the
    /// percentage changes.
    vendor: GpuVendor,
    grouped: bool,
    icon: gtk::Image,
    label: gtk::Label,
}

#[derive(Debug)]
pub enum GpuBusyMsg {
    Update(GpuBusySample),
}

#[relm4::component(pub)]
impl SimpleComponent for GpuBusy {
    type Init = WidgetInit;
    type Input = GpuBusyMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            set_visible: false,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(vendor_icon_name(GpuVendor::Unknown)),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "--%",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = GpuBusy {
            pct: None,
            vendor: GpuVendor::Unknown,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        crate::subscribe_into_msg!(hub::gpu_busy::subscribe(), sender, GpuBusyMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            GpuBusyMsg::Update(sample) => {
                let GpuBusySample { busy_pct, vendor } = sample;

                // Hide the widget entirely when no GPU was detected — matches
                // rs-bar's empty-capsule behaviour for an absent source.
                let parent = self.icon.parent();
                if busy_pct.is_none() && vendor == GpuVendor::Unknown {
                    if let Some(p) = parent.as_ref() {
                        p.set_visible(false);
                    }
                    return;
                }

                // Coalesce on (displayed_pct, vendor). The pct is already an
                // integer in 0..=100 from the hub.
                if busy_pct == self.pct && vendor == self.vendor {
                    return;
                }

                if vendor != self.vendor {
                    self.icon.set_icon_name(Some(vendor_icon_name(vendor)));
                    self.vendor = vendor;
                }
                self.pct = busy_pct;

                let text = match busy_pct {
                    Some(p) => format!("{p:>2}%"),
                    None => "--%".to_string(),
                };
                self.label.set_label(&text);

                // Color band — same thresholds as cpu_usage.
                let class = match busy_pct {
                    Some(p) if p >= 80 => "gpu-busy-crit",
                    Some(p) if p >= 60 => "gpu-busy-warn",
                    Some(p) if p >= 25 => "gpu-busy-norm",
                    _ => "gpu-busy-dim",
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);

                if let Some(p) = parent.as_ref() {
                    p.set_visible(true);
                }
            }
        }
    }
}

impl NamedWidget for GpuBusy {
    const NAME: &'static str = "gpu-busy";
}
