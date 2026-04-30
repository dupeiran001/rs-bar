//! CPU frequency widget. Subscribes to `hub::cpu_freq` and renders the
//! current frequency. Hybrid CPUs show separate P/E values divided by a
//! vertical separator; uniform CPUs show a single `X.XX GHz` value.
//!
//! The historical sparkline that used to live here was split out into a
//! dedicated `CpuFreqGraph` widget so layouts can place the graph in its
//! own capsule (or grouped with this widget).
//!
//! Mirrors the pattern documented in `cpu_usage.rs`.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;
use crate::relm4_bar::hub::cpu_freq::FreqDisplay;

use super::{NamedWidget, WidgetInit, capsule};

pub struct CpuFreq {
    /// Last-displayed reading, kept for the displayed-value coalescing check.
    display: FreqDisplay,
    grouped: bool,
    /// Held so `update` can swap between Single and Split layouts.
    label: gtk::Label,
    p_label: gtk::Label,
    sep: gtk::Separator,
    e_label: gtk::Label,
}

#[derive(Debug)]
pub enum CpuFreqMsg {
    Update(FreqDisplay),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuFreq {
    type Init = WidgetInit;
    type Input = CpuFreqMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "label"]
            gtk::Label {
                set_label: "0.00 GHz",
                set_visible: true,
            },
            #[name = "p_label"]
            gtk::Label {
                set_label: "P:0.00",
                set_visible: false,
            },
            #[name = "sep"]
            gtk::Separator {
                set_orientation: gtk::Orientation::Vertical,
                set_visible: false,
            },
            #[name = "e_label"]
            gtk::Label {
                set_label: "E:0.00",
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

        let model = CpuFreq {
            display: FreqDisplay::Single(String::new()),
            grouped: init.grouped,
            label: widgets.label.clone(),
            p_label: widgets.p_label.clone(),
            sep: widgets.sep.clone(),
            e_label: widgets.e_label.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<FreqReading> into messages.
        let mut rx = hub::cpu_freq::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Push the initial value so the widget renders immediately, then
            // loop on changes.
            let initial = rx.borrow_and_update().display.clone();
            s.input(CpuFreqMsg::Update(initial));
            while rx.changed().await.is_ok() {
                let display = rx.borrow_and_update().display.clone();
                s.input(CpuFreqMsg::Update(display));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuFreqMsg::Update(display) => {
                if display == self.display {
                    return;
                }
                self.display = display;

                match &self.display {
                    FreqDisplay::Single(s) => {
                        self.label.set_label(s);
                        self.label.set_visible(true);
                        self.p_label.set_visible(false);
                        self.sep.set_visible(false);
                        self.e_label.set_visible(false);
                    }
                    FreqDisplay::Split(p, e) => {
                        self.label.set_visible(false);
                        self.p_label.set_label(p);
                        self.p_label.set_visible(true);
                        self.sep.set_visible(true);
                        self.e_label.set_label(e);
                        self.e_label.set_visible(true);
                    }
                }
            }
        }
    }
}

impl NamedWidget for CpuFreq {
    const NAME: &'static str = "cpu-freq";
}
