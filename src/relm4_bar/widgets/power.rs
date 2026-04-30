//! Power widget. Static icon button that runs `config::POWER_COMMAND()` on
//! click. No subscription, no hub, no popover.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;

use super::{NamedWidget, WidgetInit, capsule};

/// Parse the configured icon path once and reuse the resulting `gdk::Texture`
/// across every bar instance.
fn cached_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(config::POWER_ICON()).expect("power icon load"))
}

#[allow(dead_code)]
pub struct Power {
    grouped: bool,
}

#[derive(Debug)]
pub enum PowerMsg {}

#[relm4::component(pub)]
impl SimpleComponent for Power {
    type Init = WidgetInit;
    type Input = PowerMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            add_css_class: "power-button",
            gtk::Image {
                set_paintable: Some(cached_texture()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = Power {
            grouped: init.grouped,
        };

        capsule(&root, model.grouped);

        // Click → shell out to the configured power command. The command is a
        // `&'static str`, so it's safe to move into the closure.
        let click = gtk::GestureClick::new();
        click.connect_pressed(|_, _, _, _| {
            let cmd = config::POWER_COMMAND();
            std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .spawn()
                .ok();
        });
        root.add_controller(click);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>) {}
}

impl NamedWidget for Power {
    const NAME: &'static str = "power";
}
