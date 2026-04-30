//! CPU package power-draw widget. Subscribes to `hub::power_draw` and
//! renders a vendor-specific CPU icon plus the current package watts
//! (one decimal place, e.g. `12.3W`).
//!
//! Mirrors the canonical `cpu_usage` pattern (see that file for full notes):
//! cached `gdk::Texture` parsed once, displayed-value coalescing in
//! `update`, and CSS color bands applied via `set_exclusive_class`.
//!
//! Vendor detection is performed once at widget init time by reading
//! `/proc/cpuinfo` and the macsmc-battery sysfs marker, matching rs-bar's
//! choice of doing this in the widget rather than the hub (rs-bar's hub
//! publishes only the watts value, leaving icon selection to the widget).
//!
//! When the hub reports `cpu_w == None` (no RAPL package domain and no
//! macsmc Heatpipe sensor was detected), the widget hides itself with
//! `set_visible(false)` — matches rs-bar's empty-capsule behaviour for an
//! absent source.

use std::path::Path;
use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_AMD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/amd-cpu.svg");
const ICON_INTEL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/intel-cpu.svg");
const ICON_APPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/apple-chip.svg");

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "cpu-draw-crit",
    "cpu-draw-warn",
    "cpu-draw-norm",
    "cpu-draw-dim",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum CpuVendor {
    Intel,
    Amd,
    Apple,
    Unknown,
}

/// Detect the CPU vendor from `/proc/cpuinfo` plus the Asahi macsmc-battery
/// marker. Mirrors `detect_cpu_vendor` in rs-bar's `power_draw.rs`.
fn detect_cpu_vendor() -> CpuVendor {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    for line in cpuinfo.lines() {
        if line.starts_with("vendor_id") {
            if line.contains("GenuineIntel") {
                return CpuVendor::Intel;
            } else if line.contains("AuthenticAMD") {
                return CpuVendor::Amd;
            }
        }
    }
    if Path::new("/sys/class/power_supply/macsmc-battery").exists() || cpuinfo.contains("Apple") {
        return CpuVendor::Apple;
    }
    CpuVendor::Unknown
}

/// Cache one `gdk::Texture` per known vendor. The first call parses the
/// SVG; later calls (including across other bar instances) reuse the
/// cached texture. `Unknown` returns `None` so the icon can be hidden.
fn vendor_texture(vendor: CpuVendor) -> Option<&'static gdk::Texture> {
    fn load(path: &str) -> gdk::Texture {
        gdk::Texture::from_filename(path).expect("icon load")
    }
    match vendor {
        CpuVendor::Amd => {
            static T: OnceLock<gdk::Texture> = OnceLock::new();
            Some(T.get_or_init(|| load(ICON_AMD)))
        }
        CpuVendor::Intel => {
            static T: OnceLock<gdk::Texture> = OnceLock::new();
            Some(T.get_or_init(|| load(ICON_INTEL)))
        }
        CpuVendor::Apple => {
            static T: OnceLock<gdk::Texture> = OnceLock::new();
            Some(T.get_or_init(|| load(ICON_APPLE)))
        }
        CpuVendor::Unknown => None,
    }
}

/// Quantise watts to the precision of the displayed string (one decimal
/// place: `X.X`). Used for the displayed-value coalescing check.
fn quantise_watts(w: f64) -> i32 {
    (w * 10.0).round() as i32
}

pub struct CpuDraw {
    /// Last displayed quantised watts (`None` until the first successful
    /// read). Used for the coalescing check in `update`.
    last_q: Option<i32>,
    grouped: bool,
    icon: gtk::Image,
    label: gtk::Label,
}

#[derive(Debug)]
pub enum CpuDrawMsg {
    Update(Option<f64>),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuDraw {
    type Init = WidgetInit;
    type Input = CpuDrawMsg;
    type Output = ();

    view! {
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

        // Vendor never changes at runtime — resolve once and apply.
        let vendor = detect_cpu_vendor();
        if let Some(tex) = vendor_texture(vendor) {
            widgets.icon.set_paintable(Some(tex));
        } else {
            widgets.icon.set_visible(false);
        }

        let model = CpuDraw {
            last_q: None,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        let mut rx = hub::power_draw::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Forward the initial value too, so we can decide visibility on
            // the first sample rather than waiting for the first change.
            s.input(CpuDrawMsg::Update(rx.borrow_and_update().cpu_w));
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().cpu_w;
                s.input(CpuDrawMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuDrawMsg::Update(cpu_w) => {
                // Hide the widget entirely when the source is absent —
                // matches rs-bar's empty-capsule behaviour.
                let parent = self.icon.parent();
                let Some(watts) = cpu_w else {
                    if let Some(p) = parent.as_ref() {
                        p.set_visible(false);
                    }
                    return;
                };

                // Coalesce on the *displayed* quantised value (1 decimal).
                let q = quantise_watts(watts);
                if Some(q) == self.last_q {
                    if let Some(p) = parent.as_ref() {
                        p.set_visible(true);
                    }
                    return;
                }
                self.last_q = Some(q);

                self.label.set_label(&format!("{:.1}W", watts));

                // Color band — rs-bar uses a flat `t.fg` for cpu-draw, so
                // there are no real thresholds. The classes are still
                // exposed for users who want to theme this widget; default
                // to `cpu-draw-norm`.
                let class = "cpu-draw-norm";
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);

                if let Some(p) = parent.as_ref() {
                    p.set_visible(true);
                }
            }
        }
    }
}

impl NamedWidget for CpuDraw {
    const NAME: &'static str = "cpu-draw";
}
