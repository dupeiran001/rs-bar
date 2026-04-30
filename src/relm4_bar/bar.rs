//! Bar Component — one per monitor. Owns the layer-shell ApplicationWindow
//! and renders the five zones (left, center_left, center, center_right, right).

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::widgets::Widget;

/// The five-zone layout produced by config::bar().
pub struct BarLayout {
    pub left: Vec<Widget>,
    pub center_left: Vec<Widget>,
    pub center: Vec<Widget>,
    pub center_right: Vec<Widget>,
    pub right: Vec<Widget>,
}

pub struct BarInit {
    pub monitor: gdk::Monitor,
    pub layout: BarLayout,
}

#[allow(dead_code)]
pub struct Bar {
    _layout: BarLayout, // keep widgets alive; window owns their roots
}

#[derive(Debug)]
pub enum BarMsg {}

pub struct BarWidgets {
    #[allow(dead_code)]
    pub window: gtk::ApplicationWindow,
}

impl Component for Bar {
    type Init = BarInit;
    type Input = BarMsg;
    type Output = ();
    type CommandOutput = ();
    type Root = gtk::ApplicationWindow;
    type Widgets = BarWidgets;

    fn init_root() -> Self::Root {
        let app = relm4::main_application();
        let window = gtk::ApplicationWindow::new(&app);
        window.add_css_class("rs-bar");
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_exclusive_zone(config::BAR_HEIGHT() as i32);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace(Some("rs-bar"));
        window.set_default_size(1, config::BAR_HEIGHT() as i32);
        window
    }

    fn init(init: Self::Init, window: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
        window.set_monitor(Some(&init.monitor));

        let row = gtk::CenterBox::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();

        // Left half: left + center_left
        let left_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        left_half.set_hexpand(true);
        let left_zone = build_zone(&init.layout.left, gtk::Align::Start);
        let center_left_zone = build_zone(&init.layout.center_left, gtk::Align::End);
        center_left_zone.set_hexpand(true);
        left_half.append(&left_zone);
        left_half.append(&center_left_zone);

        // Center
        let center_zone = build_zone(&init.layout.center, gtk::Align::Center);

        // Right half: center_right + right
        let right_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        right_half.set_hexpand(true);
        let center_right_zone = build_zone(&init.layout.center_right, gtk::Align::Start);
        let right_zone = build_zone(&init.layout.right, gtk::Align::End);
        right_zone.set_hexpand(true);
        right_half.append(&center_right_zone);
        right_half.append(&right_zone);

        row.set_start_widget(Some(&left_half));
        row.set_center_widget(Some(&center_zone));
        row.set_end_widget(Some(&right_half));

        // Outer overlay for borders
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&row));

        let top_border = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top_border.add_css_class("bar-border-top");
        top_border.set_valign(gtk::Align::Start);
        top_border.set_hexpand(true);
        overlay.add_overlay(&top_border);

        let bottom_border = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bottom_border.add_css_class("bar-border-bottom");
        bottom_border.set_valign(gtk::Align::End);
        bottom_border.set_hexpand(true);
        overlay.add_overlay(&bottom_border);

        window.set_child(Some(&overlay));
        window.present();

        let model = Bar { _layout: init.layout };
        let widgets = BarWidgets { window: window.clone() };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>, _root: &Self::Root) {}
}

fn build_zone(widgets: &[Widget], align: gtk::Align) -> gtk::Box {
    let zone = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    zone.add_css_class("bar-zone");
    zone.set_halign(align);
    zone.set_valign(gtk::Align::Center);
    for w in widgets {
        zone.append(&w.root);
    }
    zone
}
