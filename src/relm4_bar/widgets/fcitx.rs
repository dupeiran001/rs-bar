//! Fcitx5 input-method widget. Subscribes to [`hub::fcitx`] and renders a short
//! status label (`中` / `EN` / `?`) coloured by state.
//!
//! Mirrors the GPUI rs-bar widget: bold, fixed-width, one of three CSS
//! classes (`fcitx-chinese`, `fcitx-english`, `fcitx-unknown`) is applied to
//! the label so users can re-theme via the user CSS file without touching Rust.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;
use crate::relm4_bar::hub::fcitx::FcitxIm;

use super::{NamedWidget, WidgetInit, capsule_icon, set_exclusive_class};

/// CSS classes for the three states. `set_exclusive_class` strips the others
/// before adding the chosen one so stale classes can't accumulate.
const STATE_CLASSES: &[&str] = &["fcitx-chinese", "fcitx-english", "fcitx-unknown"];

pub struct Fcitx {
    /// Last-rendered state — used to short-circuit redundant GTK writes.
    im: FcitxIm,
    grouped: bool,
    /// Held so `update` can rewrite the label and re-style it.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum FcitxMsg {
    Update(FcitxIm),
}

#[relm4::component(pub)]
impl SimpleComponent for Fcitx {
    type Init = WidgetInit;
    type Input = FcitxMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            #[name = "label"]
            gtk::Label {
                set_label: "?",
                add_css_class: "fcitx-unknown",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = Fcitx {
            im: FcitxIm::Unknown,
            grouped: init.grouped,
            label: widgets.label.clone(),
        };

        capsule_icon(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<FcitxState> into messages.
        let mut rx = hub::fcitx::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = rx.borrow_and_update().im;
                s.input(FcitxMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            FcitxMsg::Update(im) => {
                if im == self.im {
                    return;
                }
                self.im = im;

                let (text, class) = match im {
                    FcitxIm::Chinese => ("中", "fcitx-chinese"),
                    FcitxIm::English => ("EN", "fcitx-english"),
                    FcitxIm::Unknown => ("?", "fcitx-unknown"),
                };
                self.label.set_label(text);
                set_exclusive_class(&self.label, class, STATE_CLASSES);
            }
        }
    }
}

impl NamedWidget for Fcitx {
    const NAME: &'static str = "fcitx";
}
