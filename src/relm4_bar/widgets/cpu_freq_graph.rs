//! CPU-frequency sparkline widget. Subscribes to `hub::cpu_freq` and renders
//! a small bar-graph sparkline of the recent average core frequency over the
//! last `HISTORY_SIZE` seconds. Filled vertical bars are right-aligned (newest
//! on the right) so an empty history grows in from the right edge.
//!
//! Split out of `CpuFreq` so the layout can place the graph in its own
//! capsule (or group it with the textual `CpuFreq` widget). Each instance
//! maintains its own ring buffer; samples are pushed on every hub update.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::cairo;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

/// Number of historical samples to plot (one per second). Matches rs-bar's
/// `HISTORY_SIZE` so the sparkline shape mirrors the GPUI version.
const HISTORY_SIZE: usize = 24;
/// Sparkline visual dimensions in px.
const SPARK_W: i32 = 28;
const SPARK_H: i32 = 14;

pub struct CpuFreqGraph {
    /// Sparkline DrawingArea. Held so `update` can call `queue_draw()` after
    /// pushing a new sample into the shared `samples` buffer.
    sparkline: gtk::DrawingArea,
    /// Shared sparkline sample buffer. The model pushes new readings via
    /// `borrow_mut()`; the draw callback (which holds its own clone of the
    /// `Rc`) reads via `borrow()`.
    samples: Rc<RefCell<VecDeque<f32>>>,
}

#[derive(Debug)]
pub enum CpuFreqGraphMsg {
    /// New average-GHz sample.
    Sample(f32),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuFreqGraph {
    type Init = WidgetInit;
    type Input = CpuFreqGraphMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            #[name = "sparkline"]
            gtk::DrawingArea {
                set_content_width: SPARK_W,
                set_content_height: SPARK_H,
                set_valign: gtk::Align::Center,
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

        let samples: Rc<RefCell<VecDeque<f32>>> =
            Rc::new(RefCell::new(VecDeque::with_capacity(HISTORY_SIZE)));

        // Wire the draw callback. Captures `samples` plus the per-CPU min/max
        // so the closure is self-contained — GTK invokes it whenever the area
        // is invalidated (which we trigger from `update`).
        {
            let samples = samples.clone();
            widgets.sparkline.set_draw_func(move |_da, cr, w, h| {
                draw_sparkline(cr, w, h, &samples.borrow(), min_freq_ghz, max_freq_ghz);
            });
        }

        let model = CpuFreqGraph {
            sparkline: widgets.sparkline.clone(),
            samples,
        };

        capsule(&root, init.grouped);

        // Subscription: forward every hub publish as a Sample message. The
        // graph updates every tick even when the textual freq is unchanged.
        let mut rx = hub::cpu_freq::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Send the current value first so the graph renders something
            // immediately on launch, then loop on changes.
            let initial = rx.borrow_and_update().avg_ghz;
            s.input(CpuFreqGraphMsg::Sample(initial));
            while rx.changed().await.is_ok() {
                let avg = rx.borrow_and_update().avg_ghz;
                s.input(CpuFreqGraphMsg::Sample(avg));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuFreqGraphMsg::Sample(avg_ghz) => {
                let mut s = self.samples.borrow_mut();
                s.push_back(avg_ghz);
                while s.len() > HISTORY_SIZE {
                    s.pop_front();
                }
                drop(s);
                self.sparkline.queue_draw();
            }
        }
    }
}

/// Render the sparkline into the cairo context. Filled vertical bars, one per
/// sample, right-aligned (newest on the right). Older samples fade toward
/// fully-transparent on the left edge so the sparkline visually trails off
/// into the past — same behaviour as the GPUI version.
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
    let x_offset = (HISTORY_SIZE - n) as f64 * bar_w;

    let range = (max_freq_ghz - min_freq_ghz).max(0.1);

    // Nord frost-2 (#88C0D0).
    let r = 0x88 as f64 / 255.0;
    let g = 0xc0 as f64 / 255.0;
    let b = 0xd0 as f64 / 255.0;

    for (i, &ghz) in samples.iter().enumerate() {
        let norm = ((ghz - min_freq_ghz) / range).clamp(0.0, 1.0) as f64;
        let bar_h = (norm * (h_f - 1.0) + 1.0).max(1.0);
        let y = h_f - bar_h;
        let x = x_offset + i as f64 * bar_w;
        let alpha = if n <= 1 {
            1.0
        } else {
            0.25 + 0.75 * (i as f64) / ((n - 1) as f64)
        };
        cr.set_source_rgba(r, g, b, alpha);
        cr.rectangle(x, y, bar_w, bar_h);
        let _ = cr.fill();
    }
}

impl NamedWidget for CpuFreqGraph {
    const NAME: &'static str = "cpu-freq-graph";
}
