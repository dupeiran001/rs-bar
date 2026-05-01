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
    _halves_size_group: gtk::SizeGroup, // keeps the equal-width constraint live
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

        // Three-column structural row: [left_half] [center_zone] [right_half].
        // The center column takes its natural width (e.g. Notch = 196px) — it's
        // a real layout slot, not an overlay, so nothing else can occupy that
        // space (matches the macbook hardware notch). The two halves are
        // forced to equal width via SizeGroup; combined with hexpand on both
        // and overflow=Hidden, GTK splits (bar_width − center_width) evenly
        // between them, putting the center widget at exact bar_width/2.
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_hexpand(true);

        // Left half: left_zone + center_left_zone. center_left_zone gets
        // hexpand so its content (halign=End) is pushed up against the inner
        // edge of the half, right next to the center column.
        let left_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let left_zone = build_zone(&init.layout.left, gtk::Align::Start);
        let center_left_zone = build_zone(&init.layout.center_left, gtk::Align::End);
        center_left_zone.set_hexpand(true);
        left_half.append(&left_zone);
        left_half.append(&center_left_zone);

        // Right half: center_right_zone (inner edge) + right_zone (outer edge).
        let right_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let center_right_zone = build_zone(&init.layout.center_right, gtk::Align::Start);
        let right_zone = build_zone(&init.layout.right, gtk::Align::End);
        right_zone.set_hexpand(true);
        right_half.append(&center_right_zone);
        right_half.append(&right_zone);

        // Wrap each half in an Overlay so we can paint a fade gradient on
        // its inner edge. The half itself gets overflow=Hidden + min-width=0
        // so it shrinks rather than pushing the center off-axis. When half
        // content fits inside its allocation the gradient (transparent → bg)
        // sits over empty bar bg and is invisible — it only becomes visible
        // when content reaches the inner edge and gets faded out.
        let left_with_fade = build_half_with_fade(&left_half, "bar-fade-left", gtk::Align::End);
        let right_with_fade = build_half_with_fade(&right_half, "bar-fade-right", gtk::Align::Start);

        // Center column: real, structural, never faded.
        let center_zone = build_zone(&init.layout.center, gtk::Align::Center);

        row.append(&left_with_fade);
        row.append(&center_zone);
        row.append(&right_with_fade);

        // Force equal half widths. GTK distributes shrinkage / growth evenly
        // between hexpand children with equal natural widths, so the center
        // column lands at exact bar_width/2 regardless of side content.
        let halves_size_group = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
        halves_size_group.add_widget(&left_with_fade);
        halves_size_group.add_widget(&right_with_fade);

        // Outer overlay for the 1px top/bottom borders.
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

        let model = Bar {
            _layout: init.layout,
            _halves_size_group: halves_size_group,
        };
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

/// Wrap one of the side halves in an Overlay carrying a fade gradient pinned
/// to its inner edge. The half itself is set to overflow-hidden + min-width 0
/// so it can shrink without pushing the centre column off-axis.
///
/// The fade starts opacity 0 and only flips on (`.bar-fade-active`) when this
/// half's **minimum** width exceeds its allocated width — i.e. when something
/// rigid (a fixed-width capsule) can no longer be shrunk and `overflow=Hidden`
/// is genuinely clipping pixels. Comparing against `natural` would trip every
/// time a `WindowTitle` label ellipsized, even though the label is happily
/// shrinking to its allocation and nothing is being clipped, which is exactly
/// the false positive that lit the fade up next to the centre column. The
/// check runs on each frame tick — one `measure()` and a width compare.
fn build_half_with_fade(half: &gtk::Box, fade_class: &str, fade_align: gtk::Align) -> gtk::Overlay {
    half.set_overflow(gtk::Overflow::Hidden);
    half.set_size_request(0, -1);
    half.set_hexpand(true);

    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_child(Some(half));

    let fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fade.add_css_class(fade_class);
    fade.set_halign(fade_align);
    fade.set_valign(gtk::Align::Fill);
    fade.set_can_target(false);
    overlay.add_overlay(&fade);

    let half_w = half.clone();
    let fade_w = fade.clone();
    half.add_tick_callback(move |_w, _clock| {
        let allocated = half_w.width();
        let (min_w, _, _, _) = half_w.measure(gtk::Orientation::Horizontal, -1);
        let truncated = allocated > 0 && min_w > allocated;
        let active = fade_w.has_css_class("bar-fade-active");
        if truncated && !active {
            fade_w.add_css_class("bar-fade-active");
        } else if !truncated && active {
            fade_w.remove_css_class("bar-fade-active");
        }
        glib::ControlFlow::Continue
    });

    overlay
}
