//! CPU usage widget. Subscribes to `hub::cpu_usage` and renders an icon + %.
//!
//! # Canonical relm4 widget pattern
//!
//! This widget is the canonical example for the relm4 widget pattern.
//! New widgets should mirror its structure:
//!
//! 1. **Module-level cached resources** — SVG textures via
//!    `OnceLock<gdk::Texture>` so the icon is parsed once and shared across
//!    every bar instance. Implemented here as `cached_texture()`.
//! 2. **Model fields** — held GTK widgets (`gtk::Image`, `gtk::Label`, …)
//!    needed in `update`, plus `grouped: bool` and the displayed value.
//! 3. **Message enum** — at minimum an `Update(T)` variant matching the hub's
//!    value type.
//! 4. `#[relm4::component(pub)]` + `view!` macro for declarative layout.
//! 5. **`init`** — build view via `view_output!()`, apply
//!    `capsule(&root, grouped)`, then subscribe to the hub via
//!    `relm4::spawn_local` + `rx.borrow_and_update()` and forward updates as
//!    component messages.
//! 6. **`update`** — short-circuit when the *displayed* value hasn't changed
//!    (the coalescing optimisation), then mutate the held GTK widgets
//!    directly. Use `set_exclusive_class` from `widgets::mod` to swap between
//!    mutually-exclusive CSS classes (e.g. color bands).
//! 7. **`impl NamedWidget`** with `const NAME` so the framework can refer to
//!    the widget.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

/// Symbolic icon name registered in the GTK IconTheme. The SVG is loaded
/// from `assets/icons/cpu-usage-symbolic.svg`, which uses `fill="currentColor"`
/// so the GTK style cascade can recolor it at render time.
const ICON_NAME: &str = "cpu-usage-symbolic";

/// CSS classes for color bands. `set_exclusive_class` strips the others
/// before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &[
    "cpu-usage-crit",
    "cpu-usage-warn",
    "cpu-usage-norm",
    "cpu-usage-dim",
];

pub struct CpuUsage {
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
pub enum CpuUsageMsg {
    Update(f32),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuUsage {
    type Init = WidgetInit;
    type Input = CpuUsageMsg;
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
        let model = CpuUsage {
            usage: 0.0,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        crate::subscribe_into_msg!(hub::cpu_usage::subscribe(), sender, CpuUsageMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuUsageMsg::Update(usage) => {
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

                // Color band — same thresholds as rs-bar.
                let class = if usage >= 80.0 {
                    "cpu-usage-crit"
                } else if usage >= 60.0 {
                    "cpu-usage-warn"
                } else if usage >= 25.0 {
                    "cpu-usage-norm"
                } else {
                    "cpu-usage-dim"
                };
                set_exclusive_class(&self.label, class, COLOR_CLASSES);
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
        }
    }
}

impl NamedWidget for CpuUsage {
    const NAME: &'static str = "cpu-usage";
}
