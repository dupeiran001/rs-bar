//! Discrete-GPU power draw widget. Subscribes to `hub::power_draw` and
//! renders a vendor-specific icon plus the GPU's instantaneous watts.
//!
//! Mirrors the rs-bar `GpuDraw` widget (gpui_bar/widgets/power_draw.rs):
//! one decimal place ("X.XW"), capsule wrapper when ungrouped, and hidden
//! entirely until the hub publishes a `Some(gpu_w)` reading. Absent hardware
//! stays hidden for the lifetime of the program.
//!
//! GPU vendor is detected once at first construction via a small PCI scan of
//! `/sys/bus/pci/devices` and cached in a `OnceLock`. The detection mirrors
//! `hub::power_draw::detect_gpu` so the icon matches the source the hub picked.
//!
//! Coalescing optimisation: `update` short-circuits when the displayed value
//! (watts quantised to one decimal place) hasn't changed.

use std::path::Path;
use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "gpu-draw-crit",
    "gpu-draw-warn",
    "gpu-draw-norm",
    "gpu-draw-dim",
];

/// Detected discrete GPU vendor, used for icon selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GpuVendor {
    Amd,
    Nvidia,
    Intel,
}

/// One-time vendor detection. Mirrors `hub::power_draw::detect_gpu`'s ranking
/// so the icon matches whichever GPU the hub reports watts for. iGPUs (Intel
/// at PCI bus 00:02.x) are deprioritised the same way.
fn detect_vendor() -> Option<GpuVendor> {
    static V: OnceLock<Option<GpuVendor>> = OnceLock::new();
    *V.get_or_init(|| {
        let mut best: Option<(GpuVendor, u32)> = None;
        if let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") {
            for entry in entries.filter_map(Result::ok) {
                let dev = entry.path();
                let class = std::fs::read_to_string(dev.join("class")).unwrap_or_default();
                if !class.trim().starts_with("0x03") {
                    continue;
                }
                let vendor_id = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
                let bus = entry.file_name().to_str().unwrap_or("").to_string();
                let (vendor, rank): (GpuVendor, u32) = match vendor_id.trim() {
                    "0x10de" => (GpuVendor::Nvidia, 1),
                    "0x1002" => (GpuVendor::Amd, 1),
                    "0x8086" if bus.starts_with("0000:00:02.") => (GpuVendor::Intel, 4),
                    "0x8086" => (GpuVendor::Intel, 2),
                    _ => continue,
                };
                if best.as_ref().is_none_or(|(_, br)| rank < *br) {
                    best = Some((vendor, rank));
                }
            }
        }
        if best.is_none() && Path::new("/proc/driver/nvidia/gpus").exists() {
            return Some(GpuVendor::Nvidia);
        }
        best.map(|(v, _)| v)
    })
}

/// Map a GPU vendor to its symbolic icon name (registered via the IconTheme
/// search path; SVGs use `fill="currentColor"` for live recoloring).
fn vendor_icon_name(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Amd => "amd-radeon-symbolic",
        GpuVendor::Nvidia => "nvidia-gpu-symbolic",
        GpuVendor::Intel => "intel-arc-gpu-symbolic",
    }
}

/// Quantise a float watts value to the precision of its display
/// (one decimal place: `X.X`). Used to skip redundant paints when the
/// displayed string would be identical.
fn quantise_watts(w: f64) -> i32 {
    (w * 10.0).round() as i32
}

pub struct GpuDraw {
    /// Last-seen value used for the displayed-value coalescing check.
    /// `None` until the first sample arrives.
    last_q: Option<i32>,
    grouped: bool,
    /// Held so `update` can swap the icon's texture if vendor were ever
    /// detected late (in practice it's static — but keeping the handle is
    /// the canonical pattern).
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
    /// Held so `update` can show/hide the whole widget when the hub flips
    /// between Some and None.
    root: gtk::Box,
}

#[derive(Debug)]
pub enum GpuDrawMsg {
    Update(Option<f64>),
}

#[relm4::component(pub)]
impl SimpleComponent for GpuDraw {
    type Init = WidgetInit;
    type Input = GpuDrawMsg;
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

        // Pre-set the icon for the detected vendor so it's ready
        // the moment the first reading arrives.
        if let Some(v) = detect_vendor() {
            widgets.icon.set_icon_name(Some(vendor_icon_name(v)));
        }

        let model = GpuDraw {
            last_q: None,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
            root: widgets.root_box.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<PowerDrawSample> into
        // component messages. We forward only the `gpu_w` field; the widget
        // ignores battery / cpu / psys readings.
        let mut rx = hub::power_draw::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().gpu_w;
                s.input(GpuDrawMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            GpuDrawMsg::Update(None) => {
                // Hub reports no GPU power source. Hide the widget; absent
                // hardware stays hidden for the program's lifetime.
                if self.last_q.is_some() || self.root.is_visible() {
                    self.last_q = None;
                    self.root.set_visible(false);
                }
            }
            GpuDrawMsg::Update(Some(watts)) => {
                let q = quantise_watts(watts);
                // Coalescing optimisation: skip GTK property writes when
                // the *displayed* value is unchanged.
                if self.last_q == Some(q) && self.root.is_visible() {
                    return;
                }
                self.last_q = Some(q);

                if !self.root.is_visible() {
                    // Belt-and-braces: re-apply the icon name in case vendor
                    // detection won the race vs first sample.
                    if let Some(v) = detect_vendor() {
                        self.icon.set_icon_name(Some(vendor_icon_name(v)));
                    }
                    self.root.set_visible(true);
                }

                // Format with one decimal place to match rs-bar.
                self.label.set_label(&format!("{:.1}W", watts));

                // Color band — thresholds chosen for discrete GPUs. dim
                // covers idle / desktop, norm light load, warn sustained
                // load, crit near TDP. Theme can target these via
                // `.gpu-draw-<state>` CSS rules.
                let class = if watts >= 200.0 {
                    "gpu-draw-crit"
                } else if watts >= 100.0 {
                    "gpu-draw-warn"
                } else if watts >= 20.0 {
                    "gpu-draw-norm"
                } else {
                    "gpu-draw-dim"
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
        }
    }
}

impl NamedWidget for GpuDraw {
    const NAME: &'static str = "gpu-draw";
}
