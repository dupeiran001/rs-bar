//! Clock widget. Two-line `HH:MM` / `Mon DD` display with a calendar popover.
//!
//! Self-contained: no hub subscription. A 1Hz `glib::timeout_add_local` timer
//! runs while the widget exists. The animated border re-draws once per second,
//! while the text still coalesces its GTK label writes to minute/day changes.
//!
//! Click → opens a `gtk::Popover` containing a seconds-resolution time label,
//! a long-form date label, and a `gtk::Calendar`. A second 1Hz timer runs
//! **only while the popover is visible**, started on `connect_show` and
//! removed on `connect_closed` (via `glib::SourceId::remove`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Local, Timelike};
use gtk::cairo;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;

use super::{NamedWidget, WidgetInit, capsule_interactive, popover};

const BORDER_STROKE: f64 = 1.35;
const HEAD_GLOW_RADIUS: f64 = 4.0;
const HEAD_DOT_RADIUS: f64 = 1.7;
const PROGRESS_SEGMENTS: usize = 180;
const CLOCK_SHELL_PAD: i32 = 5;
const BORDER_INSET: f64 = CLOCK_SHELL_PAD as f64;

fn format_time_at(now: &DateTime<Local>) -> String {
    now.format("%H:%M").to_string()
}

fn format_time_full_at(now: &DateTime<Local>) -> String {
    now.format("%H:%M:%S").to_string()
}

fn format_date_short_at(now: &DateTime<Local>) -> String {
    now.format("%b %-d").to_string()
}

fn format_date_full_at(now: &DateTime<Local>) -> String {
    now.format("%A, %B %e, %Y").to_string()
}

#[derive(Clone, Copy)]
struct ClockPalette {
    unvisited: (f64, f64, f64),
    tail: (f64, f64, f64),
    mid: (f64, f64, f64),
    head: (f64, f64, f64),
}

impl ClockPalette {
    fn current() -> Self {
        let theme = config::THEME();
        Self {
            unvisited: rgb(theme.fg_gutter),
            tail: rgb(theme.blue),
            mid: rgb(theme.accent_dim),
            head: rgb(theme.teal),
        }
    }
}

pub struct Clock {
    /// Last-seen displayed strings, used to coalesce 1Hz wakes into 1/min
    /// label writes.
    time: String,
    date: String,
    second: u32,
    /// Held so `update` can rewrite their labels on minute rollover.
    time_label: gtk::Label,
    date_label: gtk::Label,
    border_layer: gtk::DrawingArea,
    border_second: Rc<Cell<u32>>,
}

#[derive(Debug)]
pub enum ClockMsg {
    /// Bar-line tick: re-check `HH:MM` / `Mon DD` and update if changed.
    Tick,
}

#[relm4::component(pub)]
impl SimpleComponent for Clock {
    type Init = WidgetInit;
    type Input = ClockMsg;
    type Output = ();

    view! {
        gtk::Overlay {
            set_valign: gtk::Align::Center,
            set_halign: gtk::Align::Center,

            #[wrap(Some)]
            set_child = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_valign: gtk::Align::Center,
                set_halign: gtk::Align::Center,
                set_margin_all: CLOCK_SHELL_PAD,
                add_css_class: "clock-pill",
                add_css_class: "clock-content",
                #[name = "time_label"]
                gtk::Label {
                    set_label: &format_time_at(&Local::now()),
                    add_css_class: "clock-time",
                },
                #[name = "date_label"]
                gtk::Label {
                    set_label: &format_date_short_at(&Local::now()),
                    add_css_class: "clock-date",
                },
            },

            add_overlay: border_layer = &gtk::DrawingArea {
                set_halign: gtk::Align::Fill,
                set_valign: gtk::Align::Fill,
                set_hexpand: true,
                set_vexpand: true,
                set_can_target: false,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        assert!(
            !init.grouped,
            "Clock cannot be launched as a grouped widget; use `Clock`, not `group!(Clock)`"
        );

        let widgets = view_output!();
        let now = Local::now();
        let border_second = Rc::new(Cell::new(now.second()));
        let palette = ClockPalette::current();

        {
            let border_second = border_second.clone();
            widgets.border_layer.set_draw_func(move |_da, cr, w, h| {
                draw_clock_border(cr, w, h, border_second.get(), palette);
            });
        }

        let model = Clock {
            time: format_time_at(&now),
            date: format_date_short_at(&now),
            second: now.second(),
            time_label: widgets.time_label.clone(),
            date_label: widgets.date_label.clone(),
            border_layer: widgets.border_layer.clone(),
            border_second,
        };

        capsule_interactive(&root, init.grouped);
        root.add_css_class("clock-shell");

        // Self-spawned 1Hz timer. Sends Tick to the component each second; the
        // `update` handler does the displayed-value coalescing check.
        let s = sender.input_sender().clone();
        glib::timeout_add_local(Duration::from_secs(1), move || {
            if s.send(ClockMsg::Tick).is_ok() {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        // ── Popover with seconds-precision time, long date, and calendar ──
        let popover = gtk::Popover::builder().autohide(true).build();
        popover.add_css_class("clock-popover");
        popover.set_parent(&root);
        popover::install_motion(&popover);

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let popover_time = gtk::Label::new(Some(&format_time_full_at(&now)));
        popover_time.add_css_class("clock-popover-time");
        popover_box.append(&popover_time);

        let popover_date = gtk::Label::new(Some(&format_date_full_at(&now)));
        popover_date.add_css_class("clock-popover-date");
        popover_box.append(&popover_date);

        let calendar = gtk::Calendar::new();
        popover_box.append(&calendar);

        popover::set_liquid_child(&popover, &popover_box);

        // 1Hz timer for the popover seconds label, alive only while shown.
        // SourceId is !Send and !Sync; keep it inside an Rc<RefCell<…>> so the
        // show/closed closures can swap it without cloning the SourceId.
        let popover_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

        {
            let popover_time = popover_time.clone();
            let popover_date = popover_date.clone();
            let popover_source = popover_source.clone();
            popover.connect_show(move |_| {
                // Refresh immediately on open (don't wait a full second).
                let now = Local::now();
                popover_time.set_label(&format_time_full_at(&now));
                popover_date.set_label(&format_date_full_at(&now));

                let pt = popover_time.clone();
                let pd = popover_date.clone();
                let id = glib::timeout_add_local(Duration::from_secs(1), move || {
                    let now = Local::now();
                    pt.set_label(&format_time_full_at(&now));
                    pd.set_label(&format_date_full_at(&now));
                    glib::ControlFlow::Continue
                });
                // Replace any previous source (shouldn't exist, but be safe).
                if let Some(old) = popover_source.borrow_mut().replace(id) {
                    old.remove();
                }
            });
        }

        {
            let popover_source = popover_source.clone();
            popover.connect_closed(move |_| {
                if let Some(id) = popover_source.borrow_mut().take() {
                    id.remove();
                }
            });
        }

        // Click → open popover. Use GestureClick on the root box.
        let click = gtk::GestureClick::new();
        let popover_for_click = popover.clone();
        click.connect_pressed(move |_, _, _, _| popover::toggle(&popover_for_click));
        root.add_controller(click);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            ClockMsg::Tick => {
                let now = Local::now();
                let new_second = now.second();
                if new_second != self.second {
                    self.second = new_second;
                    self.border_second.set(new_second);
                    self.border_layer.queue_draw();
                }

                // Coalescing: only touch GTK when the visible strings change.
                let new_time = format_time_at(&now);
                let new_date = format_date_short_at(&now);
                if new_time != self.time {
                    self.time = new_time;
                    self.time_label.set_label(&self.time);
                }
                if new_date != self.date {
                    self.date = new_date;
                    self.date_label.set_label(&self.date);
                }
            }
        }
    }
}

impl NamedWidget for Clock {
    const NAME: &'static str = "clock";
}

fn draw_clock_border(
    cr: &cairo::Context,
    width: i32,
    height: i32,
    second: u32,
    palette: ClockPalette,
) {
    if width <= 0 || height <= 0 {
        return;
    }

    let Some(geom) = BorderGeometry::new(width as f64, height as f64, BORDER_STROKE) else {
        return;
    };

    cr.set_line_width(BORDER_STROKE);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_join(cairo::LineJoin::Round);

    rounded_rect_path(
        cr,
        geom.left,
        geom.top,
        geom.width,
        geom.height,
        geom.radius,
    );
    let (r, g, b) = palette.unvisited;
    cr.set_source_rgba(r, g, b, 0.9);
    let _ = cr.stroke();

    let progress = second.min(59) as f64 / 60.0;
    if progress > 0.0 {
        let segment_count = ((PROGRESS_SEGMENTS as f64) * progress).ceil() as usize;
        let mut prev = geom.point_at(0.0);

        for idx in 1..=segment_count.max(1) {
            let t = progress * (idx as f64) / (segment_count.max(1) as f64);
            let next = geom.point_at(t);
            let color_t = (t / progress).clamp(0.0, 1.0);
            let (r, g, b) = progress_color(palette, color_t);
            cr.set_source_rgba(r, g, b, 0.98);
            cr.move_to(prev.0, prev.1);
            cr.line_to(next.0, next.1);
            let _ = cr.stroke();
            prev = next;
        }
    }

    let head = geom.point_at(progress);
    let (hr, hg, hb) = palette.head;
    cr.set_source_rgba(hr, hg, hb, 0.18);
    cr.arc(head.0, head.1, HEAD_GLOW_RADIUS, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();

    cr.set_source_rgba(hr, hg, hb, 0.45);
    cr.arc(
        head.0,
        head.1,
        HEAD_GLOW_RADIUS * 0.58,
        0.0,
        std::f64::consts::TAU,
    );
    let _ = cr.fill();

    cr.set_source_rgb(hr, hg, hb);
    cr.arc(head.0, head.1, HEAD_DOT_RADIUS, 0.0, std::f64::consts::TAU);
    let _ = cr.fill();
}

fn progress_color(palette: ClockPalette, t: f64) -> (f64, f64, f64) {
    if t < 0.75 {
        lerp_rgb(palette.tail, palette.mid, t / 0.75)
    } else {
        lerp_rgb(palette.mid, palette.head, (t - 0.75) / 0.25)
    }
}

fn rgb(hex: u32) -> (f64, f64, f64) {
    (
        ((hex >> 16) & 0xff) as f64 / 255.0,
        ((hex >> 8) & 0xff) as f64 / 255.0,
        (hex & 0xff) as f64 / 255.0,
    )
}

fn lerp_rgb(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

struct BorderGeometry {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    radius: f64,
    center_x: f64,
    horiz: f64,
    vert: f64,
    perimeter: f64,
}

impl BorderGeometry {
    fn new(width: f64, height: f64, stroke: f64) -> Option<Self> {
        // Keep the animated head + glow fully inside the drawing area's
        // allocation. GTK clips DrawingArea rendering to its bounds, so if the
        // border path sits right on the edge the glow gets cut off.
        let inset = BORDER_INSET.max(stroke / 2.0);
        let inner_w = width - 2.0 * inset;
        let inner_h = height - 2.0 * inset;
        if inner_w <= 0.0 || inner_h <= 0.0 {
            return None;
        }

        let radius = (inner_w.min(inner_h) / 2.0).max(0.0);
        let horiz = (inner_w - 2.0 * radius).max(0.0);
        let vert = (inner_h - 2.0 * radius).max(0.0);
        let perimeter = 2.0 * (horiz + vert) + 2.0 * std::f64::consts::PI * radius;

        Some(Self {
            left: inset,
            top: inset,
            width: inner_w,
            height: inner_h,
            radius,
            center_x: inset + inner_w / 2.0,
            horiz,
            vert,
            perimeter,
        })
    }

    fn point_at(&self, fraction: f64) -> (f64, f64) {
        let mut distance = fraction.rem_euclid(1.0) * self.perimeter;
        let arc = std::f64::consts::FRAC_PI_2 * self.radius;
        let right = self.left + self.width;
        let bottom = self.top + self.height;

        if distance <= self.horiz / 2.0 {
            return (self.center_x + distance, self.top);
        }
        distance -= self.horiz / 2.0;

        if distance <= arc {
            let angle = -std::f64::consts::FRAC_PI_2 + distance / self.radius.max(f64::EPSILON);
            return point_on_arc(
                right - self.radius,
                self.top + self.radius,
                self.radius,
                angle,
            );
        }
        distance -= arc;

        if distance <= self.vert {
            return (right, self.top + self.radius + distance);
        }
        distance -= self.vert;

        if distance <= arc {
            let angle = distance / self.radius.max(f64::EPSILON);
            return point_on_arc(
                right - self.radius,
                bottom - self.radius,
                self.radius,
                angle,
            );
        }
        distance -= arc;

        if distance <= self.horiz {
            return (right - self.radius - distance, bottom);
        }
        distance -= self.horiz;

        if distance <= arc {
            let angle = std::f64::consts::FRAC_PI_2 + distance / self.radius.max(f64::EPSILON);
            return point_on_arc(
                self.left + self.radius,
                bottom - self.radius,
                self.radius,
                angle,
            );
        }
        distance -= arc;

        if distance <= self.vert {
            return (self.left, bottom - self.radius - distance);
        }
        distance -= self.vert;

        if distance <= arc {
            let angle = std::f64::consts::PI + distance / self.radius.max(f64::EPSILON);
            return point_on_arc(
                self.left + self.radius,
                self.top + self.radius,
                self.radius,
                angle,
            );
        }
        distance -= arc;

        (self.left + self.radius + distance, self.top)
    }
}

fn point_on_arc(cx: f64, cy: f64, radius: f64, angle: f64) -> (f64, f64) {
    (cx + radius * angle.cos(), cy + radius * angle.sin())
}

fn rounded_rect_path(
    cr: &cairo::Context,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let right = left + width;
    let bottom = top + height;

    cr.new_sub_path();
    cr.arc(
        right - radius,
        top + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        right - radius,
        bottom - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        left + radius,
        bottom - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        left + radius,
        top + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    cr.close_path();
}
