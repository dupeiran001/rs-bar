//! Memory usage widget. Subscribes to `hub::memory` and renders an icon + %.
//!
//! Mirrors the rs-bar gpui_bar memory widget: displays used percentage as
//! `NN%` (right-aligned to 2 chars) with a color band determined by the
//! current usage. See `cpu_usage.rs` for the canonical relm4 widget pattern
//! this file follows.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/memory.svg");

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "memory-crit",
    "memory-warn",
    "memory-norm",
    "memory-dim",
];

/// Parse the SVG icon once and reuse the resulting `gdk::Texture` across
/// every bar instance. The path is hard-coded with `concat!(env!(…))`, so a
/// missing icon is a build-time problem and `expect` here is acceptable.
fn cached_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(ICON_PATH).expect("icon load"))
}

pub struct Memory {
    /// Last-seen usage as a float, kept for the displayed-value coalescing
    /// check in `update`.
    usage: f32,
    grouped: bool,
    /// Held so `update` can re-style the icon when the color band changes.
    icon: gtk::Image,
    /// Held so `update` can rewrite the label text and re-style it.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum MemoryMsg {
    Update(f32),
}

#[relm4::component(pub)]
impl SimpleComponent for Memory {
    type Init = WidgetInit;
    type Input = MemoryMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_paintable: Some(cached_texture()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: " 0%",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = Memory {
            usage: 0.0,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<f32> into component messages.
        // `relm4::spawn_local` runs on the GTK main context, so passing the
        // ComponentSender across the await is safe.
        let mut rx = hub::memory::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = *rx.borrow_and_update();
                s.input(MemoryMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            MemoryMsg::Update(usage) => {
                // Coalescing optimisation: skip the GTK property writes when
                // the *displayed* value is unchanged. The float value only
                // affects the color band, whose thresholds are integer, so
                // a no-op rounded pct means the rendered output is identical.
                let new_pct = usage.round() as u32;
                let old_pct = self.usage.round() as u32;
                if new_pct == old_pct {
                    return;
                }
                self.usage = usage;
                self.label.set_label(&format!("{:>2}%", new_pct));

                // Color band — same thresholds as rs-bar gpui_bar memory.
                let class = if usage >= 90.0 {
                    "memory-crit"
                } else if usage >= 75.0 {
                    "memory-warn"
                } else {
                    "memory-norm"
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
        }
    }
}

impl NamedWidget for Memory {
    const NAME: &'static str = "memory";
}
