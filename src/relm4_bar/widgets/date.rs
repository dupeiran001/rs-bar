//! Date widget. Self-contained: no hub. Uses `glib::timeout_add_local` to wake
//! once per second and only mutates the GTK label when the formatted string
//! actually changes (matching rs-bar's coalescing behaviour).
//!
//! `#![allow(dead_code)]` because Date isn't enabled in any current config
//! profile (matches rs-bar). Drop the attribute once a profile uses it.
#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::Duration;

use chrono::Local;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;

use super::{NamedWidget, WidgetInit, capsule};

const ICON_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/calendar.svg");

/// Parse the SVG icon once and reuse the resulting `gdk::Texture` across
/// every bar instance.
fn cached_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(ICON_PATH).expect("icon load"))
}

fn format_date() -> String { Local::now().format("%m-%d").to_string() }

pub struct Date {
    /// Last-seen formatted string; updates short-circuit when unchanged.
    date: String,
    grouped: bool,
    /// Held so `update` can rewrite the label text on day rollover.
    label: gtk::Label,
}

#[derive(Debug)]
pub enum DateMsg {
    Tick,
}

#[relm4::component(pub)]
impl SimpleComponent for Date {
    type Init = WidgetInit;
    type Input = DateMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            gtk::Image {
                set_paintable: Some(cached_texture()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: &format_date(),
                add_css_class: "date-label",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = Date {
            date: format_date(),
            grouped: init.grouped,
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        // 1 Hz wake — matches rs-bar. The `update` handler short-circuits when
        // the formatted value hasn't changed, so the GTK label is rewritten at
        // most once per day.
        let s = sender.clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            s.input(DateMsg::Tick);
            glib::ControlFlow::Continue
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            DateMsg::Tick => {
                let new = format_date();
                if new == self.date {
                    return;
                }
                self.date = new;
                self.label.set_label(&self.date);
            }
        }
    }
}

impl NamedWidget for Date {
    const NAME: &'static str = "date";
}
