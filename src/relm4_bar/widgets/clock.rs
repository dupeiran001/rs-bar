//! Clock widget. Two-line `HH:MM` / `Mon DD` display with a calendar popover.
//!
//! Self-contained: no hub subscription. A 1Hz `glib::timeout_add_local` timer
//! runs while the widget exists; it wakes 60×/min but only emits an `Update`
//! message (and only re-sets GTK labels) when the *displayed* strings change,
//! so visible repaint happens once per minute.
//!
//! Click → opens a `gtk::Popover` containing a seconds-resolution time label,
//! a long-form date label, and a `gtk::Calendar`. A second 1Hz timer runs
//! **only while the popover is visible**, started on `connect_show` and
//! removed on `connect_closed` (via `glib::SourceId::remove`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use chrono::Local;
use gtk::prelude::*;
use relm4::prelude::*;

use super::{NamedWidget, WidgetInit, capsule, capsule_interactive, popover};

fn format_time() -> String {
    Local::now().format("%H:%M").to_string()
}

fn format_time_full() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

fn format_date_short() -> String {
    Local::now().format("%b %-d").to_string()
}

fn format_date_full() -> String {
    Local::now().format("%A, %B %e, %Y").to_string()
}

pub struct Clock {
    grouped: bool,
    /// Last-seen displayed strings, used to coalesce 1Hz wakes into 1/min
    /// label writes.
    time: String,
    date: String,
    /// Held so `update` can rewrite their labels on minute rollover.
    time_label: gtk::Label,
    date_label: gtk::Label,
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
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_valign: gtk::Align::Center,
            set_halign: gtk::Align::Center,
            #[name = "time_label"]
            gtk::Label {
                set_label: &format_time(),
                add_css_class: "clock-time",
            },
            #[name = "date_label"]
            gtk::Label {
                set_label: &format_date_short(),
                add_css_class: "clock-date",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = Clock {
            grouped: init.grouped,
            time: format_time(),
            date: format_date_short(),
            time_label: widgets.time_label.clone(),
            date_label: widgets.date_label.clone(),
        };

        capsule(&root, model.grouped);
        capsule_interactive(&root, model.grouped);

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

        let popover_time = gtk::Label::new(Some(&format_time_full()));
        popover_time.add_css_class("clock-popover-time");
        popover_box.append(&popover_time);

        let popover_date = gtk::Label::new(Some(&format_date_full()));
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
                popover_time.set_label(&format_time_full());
                popover_date.set_label(&format_date_full());

                let pt = popover_time.clone();
                let pd = popover_date.clone();
                let id = glib::timeout_add_local(Duration::from_secs(1), move || {
                    pt.set_label(&format_time_full());
                    pd.set_label(&format_date_full());
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
                // Coalescing: only touch GTK when the visible strings change.
                let new_time = format_time();
                let new_date = format_date_short();
                if new_time == self.time && new_date == self.date {
                    return;
                }
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
