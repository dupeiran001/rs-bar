//! Platform power-draw widget (RAPL `psys` / `platform` or macsmc Total
//! System Power). Subscribes to `hub::power_draw` and reads `psys_w`. When
//! the source field is `None` (psys not detected on this hardware) the
//! widget hides itself entirely; otherwise it renders an icon plus a
//! `X.XW` value, with a CSS color band selected by current watts.
//!
//! Mirrors the pattern documented in `cpu_usage.rs`. Coalescing key is the
//! displayed value: watts quantised to one decimal place.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_NAME: &str = "psys-symbolic";

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "psys-draw-crit",
    "psys-draw-warn",
    "psys-draw-norm",
    "psys-draw-dim",
];

/// Quantise watts to the precision of the rendered string (`X.XW`, one
/// decimal place). Used for the displayed-value coalescing check so that
/// repeated samples that round to the same string don't trigger paints.
fn quantise_watts(w: f64) -> i32 {
    (w * 10.0).round() as i32
}

pub struct PsysDraw {
    /// Last quantised watts, kept for the coalescing check. `None` until
    /// the first reading arrives or while the source is unavailable.
    last_q: Option<i32>,
    grouped: bool,
    /// Root box, held so `update` can toggle visibility when the source
    /// transitions between absent and present.
    root: gtk::Box,
    /// Held so `update` can re-style the icon on color-band changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum PsysDrawMsg {
    /// `Some(watts)` from the hub when psys is available; `None` when the
    /// source has not been detected on this hardware (widget hides).
    Update(Option<f64>),
}

#[relm4::component(pub)]
impl SimpleComponent for PsysDraw {
    type Init = WidgetInit;
    type Input = PsysDrawMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            // Hidden until the first `Some` reading arrives. If psys is
            // unavailable on this hardware the box stays hidden forever.
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
        let model = PsysDraw {
            last_q: None,
            grouped: init.grouped,
            root: root.clone(),
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<PowerDrawSample> into
        // component messages, forwarding only the psys field.
        let mut rx = hub::power_draw::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().psys_w;
                s.input(PsysDrawMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            PsysDrawMsg::Update(None) => {
                // Source unavailable: hide and reset coalescing state so a
                // later `Some` will repaint correctly.
                if self.root.is_visible() {
                    self.root.set_visible(false);
                }
                self.last_q = None;
            }
            PsysDrawMsg::Update(Some(watts)) => {
                let q = quantise_watts(watts);
                // Coalescing optimisation: skip GTK property writes when the
                // displayed value (one-decimal watts) is unchanged.
                if self.last_q == Some(q) {
                    return;
                }
                self.last_q = Some(q);

                if !self.root.is_visible() {
                    self.root.set_visible(true);
                }

                self.label.set_label(&format!("{:.1}W", watts));

                // Color band by absolute watts. Thresholds chosen so a
                // typical idle laptop sits in `dim`, light load in `norm`,
                // sustained load in `warn`, and a heavy desktop in `crit`.
                let class = if watts >= 60.0 {
                    "psys-draw-crit"
                } else if watts >= 30.0 {
                    "psys-draw-warn"
                } else if watts >= 5.0 {
                    "psys-draw-norm"
                } else {
                    "psys-draw-dim"
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
        }
    }
}

impl NamedWidget for PsysDraw {
    const NAME: &'static str = "psys-draw";
}
