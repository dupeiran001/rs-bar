//! CapsLock indicator widget. Subscribes to `hub::capslock` (a `bool`) and
//! shows an icon only while CapsLock is on; hides itself otherwise.
//!
//! Mirrors the canonical `cpu_usage` pattern: cached SVG `gdk::Texture`,
//! displayed-value coalescing in `update`. The on/off transition is driven
//! by `set_visible`, and the icon carries the `capslock-active` CSS class
//! while shown so themes can recolour it.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule_icon};

const ICON_NAME: &str = "capslock-symbolic";

pub struct CapsLock {
    /// Last-seen state, kept for the displayed-value coalescing check.
    on: bool,
    grouped: bool,
    /// Held so `update` can toggle visibility on each transition.
    icon: gtk::Image,
}

#[derive(Debug)]
pub enum CapsLockMsg {
    Update(bool),
}

#[relm4::component(pub)]
impl SimpleComponent for CapsLock {
    type Init = WidgetInit;
    type Input = CapsLockMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            set_halign: gtk::Align::Center,
            set_visible: false,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_NAME),
                set_pixel_size: config::ICON_SIZE() as i32,
                add_css_class: "capslock-active",
                set_halign: gtk::Align::Center,
                set_hexpand: true,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = CapsLock {
            on: false,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
        };

        capsule_icon(&root, model.grouped);

        let mut rx = hub::capslock::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Forward the initial value too, so visibility is correct on the
            // first sample rather than waiting for a change.
            s.input(CapsLockMsg::Update(*rx.borrow_and_update()));
            while rx.changed().await.is_ok() {
                let v = *rx.borrow_and_update();
                s.input(CapsLockMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CapsLockMsg::Update(on) => {
                if on == self.on {
                    return;
                }
                self.on = on;
                if let Some(parent) = self.icon.parent() {
                    parent.set_visible(on);
                }
            }
        }
    }
}

impl NamedWidget for CapsLock {
    const NAME: &'static str = "capslock";
}
