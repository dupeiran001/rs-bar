//! 196px-wide spacer reserving space for the macbook hardware notch.

use gtk::prelude::*;
use relm4::prelude::*;

use super::{NamedWidget, WidgetInit, capsule};

#[allow(dead_code)]
pub struct Notch {
    grouped: bool,
}

#[derive(Debug)]
pub enum NotchMsg {}

#[relm4::component(pub)]
impl SimpleComponent for Notch {
    type Init = WidgetInit;
    type Input = NotchMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_size_request: (196, -1),
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Notch {
            grouped: init.grouped,
        };
        let widgets = view_output!();
        capsule(&root, model.grouped);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>) {}
}

impl NamedWidget for Notch {
    const NAME: &'static str = "notch";
}
