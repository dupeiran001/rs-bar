//! CPU frequency widget. Subscribes to `hub::cpu_freq` and renders an icon +
//! the current frequency. Hybrid CPUs show separate P/E values divided by a
//! vertical separator; uniform CPUs show a single `X.XX GHz` value.
//!
//! A small bar-graph sparkline is drawn to the LEFT of the icon, tracing the
//! recent average core frequency over the last `HISTORY_SIZE` seconds. Filled
//! vertical bars are right-aligned (newest on the right) so an empty history
//! grows in from the right edge — same look as the GPUI bar.
//!
//! Mirrors the pattern documented in `cpu_usage.rs`.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::cpu_freq::FreqDisplay;

use super::{NamedWidget, WidgetInit, capsule};

const ICON_NAME: &str = "cpu-freq-symbolic";

/// Number of historical samples to plot (one per second). Matches rs-bar's
/// `HISTORY_SIZE` so the sparkline shape mirrors the GPUI version.
const HISTORY_SIZE: usize = 24;
/// Sparkline visual dimensions in px.
const SPARK_W: i32 = 28;
const SPARK_H: i32 = 14;

pub struct CpuFreq {
    /// Last-displayed reading, kept for the displayed-value coalescing check.
    display: FreqDisplay,
    grouped: bool,
    /// Held so `update` can swap between Single and Split layouts.
    label: gtk::Label,
    p_label: gtk::Label,
    sep: gtk::Separator,
    e_label: gtk::Label,
    /// Sparkline DrawingArea. Held so `update` can call `queue_draw()` after
    /// pushing a new sample into the shared `samples` buffer.
    sparkline: gtk::DrawingArea,
    /// Shared sparkline sample buffer. The model pushes new readings via
    /// `borrow_mut()`; the draw callback (which holds its own clone of the
    /// `Rc`) reads via `borrow()`.
    samples: Rc<RefCell<VecDeque<f32>>>,
    /// Per-CPU min/max scaling frequency, used as the sparkline's vertical
    /// scale. Captured once at init time from sysfs and copied into the draw
    /// closure; kept on the model for completeness/debugging only.
    #[allow(dead_code)]
    min_freq_ghz: f32,
    #[allow(dead_code)]
    max_freq_ghz: f32,
}

#[derive(Debug)]
pub enum CpuFreqMsg {
    Update(FreqDisplay, f32),
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
            #[name = "sparkline"]
            gtk::DrawingArea {
                set_content_width: SPARK_W,
                set_content_height: SPARK_H,
                set_valign: gtk::Align::Center,
            },
            gtk::Image {
                set_icon_name: Some(ICON_NAME),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
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

        let (min_freq_ghz, max_freq_ghz) = hub::cpu_freq::detect_freq_range_ghz();

        // Shared sample buffer between the model and the draw callback.
        let samples: Rc<RefCell<VecDeque<f32>>> =
            Rc::new(RefCell::new(VecDeque::with_capacity(HISTORY_SIZE)));

        // Wire the draw callback. We capture a clone of `samples` plus the
        // min/max so the callback is self-contained — the GTK runtime invokes
        // it whenever the area is invalidated (which we trigger from `update`).
        {
            let samples = samples.clone();
            widgets.sparkline.set_draw_func(move |_da, cr, w, h| {
                draw_sparkline(cr, w, h, &samples.borrow(), min_freq_ghz, max_freq_ghz);
            });
        }

        let model = CpuFreq {
            display: FreqDisplay::Single(String::new()),
            grouped: init.grouped,
            label: widgets.label.clone(),
            p_label: widgets.p_label.clone(),
            sep: widgets.sep.clone(),
            e_label: widgets.e_label.clone(),
            sparkline: widgets.sparkline.clone(),
            samples,
            min_freq_ghz,
            max_freq_ghz,
        };

        capsule(&root, model.grouped);

        // Subscription: bridge the watch::Receiver<FreqReading> into messages.
        let mut rx = hub::cpu_freq::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let reading = rx.borrow_and_update().clone();
                s.input(CpuFreqMsg::Update(reading.display, reading.avg_ghz));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuFreqMsg::Update(display, avg_ghz) => {
                // Push the new sample into the sparkline history (always; the
                // sparkline updates every tick even if the displayed text is
                // unchanged) and request a redraw.
                {
                    let mut s = self.samples.borrow_mut();
                    s.push_back(avg_ghz);
                    while s.len() > HISTORY_SIZE {
                        s.pop_front();
                    }
                }
                self.sparkline.queue_draw();

                // Coalescing: skip GTK label writes when the displayed value
                // is identical to the previously-rendered one.
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

/// Render the sparkline into the cairo context. Filled vertical bars, one per
/// sample, right-aligned (newest on the right). Brighter colour for the most
/// recent bar gives the same fading-in feel as the GPUI version. The decay
/// gradient overlay from rs-bar isn't reproduced here — for v1 the simpler
/// solid bars are already a faithful read of recent frequency history.
///
/// `min_freq_ghz` / `max_freq_ghz` are the per-CPU scaling range; samples are
/// normalised against them so an idle laptop reads as low bars and a loaded
/// CPU pegs to the top.
fn draw_sparkline(
    cr: &cairo::Context,
    w: i32,
    h: i32,
    samples: &VecDeque<f32>,
    min_freq_ghz: f32,
    max_freq_ghz: f32,
) {
    if samples.is_empty() || w <= 0 || h <= 0 {
        return;
    }
    let w_f = w as f64;
    let h_f = h as f64;
    let bar_w = w_f / HISTORY_SIZE as f64;
    let n = samples.len();
    // Right-align: leave empty space on the left if history isn't full yet.
    let x_offset = (HISTORY_SIZE - n) as f64 * bar_w;

    let range = (max_freq_ghz - min_freq_ghz).max(0.1);

    // Nord frost-2 (#88C0D0) accent colour and a dimmer shade. Hardcoded
    // because looking up CSS variables from inside a draw callback is awkward
    // in GTK4 — the user's theme can still override the bar's background, and
    // the decay-from-old behaviour matches the GPUI version visually.
    let accent_dim = (0x6c as f64 / 255.0, 0x9e as f64 / 255.0, 0xb0 as f64 / 255.0);
    let accent = (0x88 as f64 / 255.0, 0xc0 as f64 / 255.0, 0xd0 as f64 / 255.0);

    for (i, &ghz) in samples.iter().enumerate() {
        let norm = ((ghz - min_freq_ghz) / range).clamp(0.0, 1.0) as f64;
        // Reserve a 1px floor so even idle samples are visible.
        let bar_h = (norm * (h_f - 1.0) + 1.0).max(1.0);
        let y = h_f - bar_h;
        let x = x_offset + i as f64 * bar_w;
        let (r, g, b) = if (i + 1) == n { accent } else { accent_dim };
        cr.set_source_rgb(r, g, b);
        cr.rectangle(x, y, bar_w, bar_h);
        let _ = cr.fill();
    }
}

impl NamedWidget for CpuFreq {
    const NAME: &'static str = "cpu-freq";
}
