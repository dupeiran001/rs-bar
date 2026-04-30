//! Package-update indicator widget. Subscribes to `hub::pkg_update` and
//! renders one of two SVG icons (uptodate / pending) plus an optional count.
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`,
//! with the small twist that there are two cached SVG textures rather than
//! one — the icon swaps based on whether the count is zero.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_OK_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/pkg-uptodate.svg");
const ICON_PENDING_PATH: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/pkg-updates.svg");

/// CSS classes for the two states. `set_exclusive_class` strips the other
/// before adding the chosen one.
const STATE_CLASSES: &[&str] = &["pkg-update-pending", "pkg-update-ok"];

/// Parse the "all up to date" SVG icon once and reuse the resulting
/// `gdk::Texture` across every bar instance. The path is hard-coded with
/// `concat!(env!(…))`, so a missing icon is a build-time problem and `expect`
/// here is acceptable.
fn cached_uptodate_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(ICON_OK_PATH).expect("icon load"))
}

/// Parse the "updates pending" SVG icon once and reuse the resulting
/// `gdk::Texture` across every bar instance.
fn cached_pending_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(ICON_PENDING_PATH).expect("icon load"))
}

pub struct PkgUpdate {
    /// Last-seen update count, kept for the displayed-value coalescing check
    /// in `update`.
    count: u32,
    /// Tracks whether `count` has ever been written to. The model's initial
    /// `count` of 0 is identical to a real zero result, but the GTK widgets
    /// haven't yet been configured for that state, so the very first update
    /// must always run.
    initialized: bool,
    grouped: bool,
    /// Held so `update` can swap icons and re-style.
    icon: gtk::Image,
    /// Held so `update` can rewrite the count text and toggle visibility.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum PkgUpdateMsg {
    Update(u32),
}

#[relm4::component(pub)]
impl SimpleComponent for PkgUpdate {
    type Init = WidgetInit;
    type Input = PkgUpdateMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_paintable: Some(cached_uptodate_texture()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "",
                set_visible: false,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = PkgUpdate {
            count: 0,
            initialized: false,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<u32> into component messages.
        // `relm4::spawn_local` runs on the GTK main context, so passing the
        // ComponentSender across the await is safe.
        let mut rx = hub::pkg_update::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = *rx.borrow_and_update();
                s.input(PkgUpdateMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            PkgUpdateMsg::Update(count) => {
                // Coalescing optimisation: skip GTK property writes when the
                // displayed value is unchanged. The first message must always
                // run because the initial widget state hasn't been configured
                // by `update` yet.
                if self.initialized && count == self.count {
                    return;
                }
                self.count = count;
                self.initialized = true;

                if count == 0 {
                    self.icon.set_paintable(Some(cached_uptodate_texture()));
                    self.label.set_visible(false);
                    self.label.set_label("");
                    set_exclusive_class(&self.label, "pkg-update-ok", STATE_CLASSES);
                    set_exclusive_class(&self.icon, "pkg-update-ok", STATE_CLASSES);
                } else {
                    self.icon.set_paintable(Some(cached_pending_texture()));
                    self.label.set_label(&count.to_string());
                    self.label.set_visible(true);
                    set_exclusive_class(&self.label, "pkg-update-pending", STATE_CLASSES);
                    set_exclusive_class(&self.icon, "pkg-update-pending", STATE_CLASSES);
                }
            }
        }
    }
}

impl NamedWidget for PkgUpdate {
    const NAME: &'static str = "pkg-update";
}
