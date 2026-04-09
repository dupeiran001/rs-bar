//! CPU frequency widget.
//!
//! Detects hybrid P/E core topology (Intel core_type sysfs or max_freq split).
//! Displays:
//!   - Hybrid: "P: X.XX GHz | E: X.XX GHz"
//!   - Uniform: "X.XX GHz"
//!
//! Reads every second via `timerfd` + `epoll`. All sysfs, zero subprocesses.

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use gpui::{
    Context, Hsla, IntoElement, ParentElement, Styled, Window, div, linear_color_stop,
    linear_gradient, px, rgb,
};

use super::{BarWidget, impl_render};

/// Number of historical samples to plot (one per second).
const HISTORY_SIZE: usize = 24;
/// Sparkline visual dimensions (px).
const SPARK_W: f32 = 28.0;
const SPARK_H: f32 = 14.0;

// ── topology detection (once at startup) ───────────────────────────────

#[derive(Clone)]
enum CoreLayout {
    Uniform { cpus: Vec<u32> },
    Hybrid { p_cpus: Vec<u32>, e_cpus: Vec<u32> },
}

fn detect_layout() -> CoreLayout {
    let mut p_cpus = Vec::new();
    let mut e_cpus = Vec::new();
    let has_core_type =
        std::path::Path::new("/sys/devices/system/cpu/cpu0/topology/core_type").exists();

    if has_core_type {
        for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            let num: u32 = match name.strip_prefix("cpu").and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            let ct = std::fs::read_to_string(entry.path().join("topology/core_type"))
                .unwrap_or_default();
            match ct.trim() {
                "1" => p_cpus.push(num),
                "0" => e_cpus.push(num),
                _ => p_cpus.push(num),
            }
        }
    } else {
        let mut all_cpus: Vec<(u32, u64)> = Vec::new();
        for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            let num: u32 = match name.strip_prefix("cpu").and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            let max_freq = std::fs::read_to_string(
                entry.path().join("cpufreq/cpuinfo_max_freq"),
            )
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
            if max_freq > 0 {
                all_cpus.push((num, max_freq));
            }
        }

        let mut unique_freqs: Vec<u64> = all_cpus.iter().map(|(_, f)| *f).collect();
        unique_freqs.sort_unstable();
        unique_freqs.dedup();

        if unique_freqs.len() == 2
            && unique_freqs[0] > 0
            && unique_freqs[1] * 100 / unique_freqs[0] >= 120
        {
            let p_max = unique_freqs[1];
            for (num, freq) in &all_cpus {
                if *freq == p_max {
                    p_cpus.push(*num);
                } else {
                    e_cpus.push(*num);
                }
            }
        } else {
            for (num, _) in &all_cpus {
                p_cpus.push(*num);
            }
        }
    }

    p_cpus.sort_unstable();
    e_cpus.sort_unstable();

    if e_cpus.is_empty() {
        CoreLayout::Uniform { cpus: p_cpus }
    } else {
        CoreLayout::Hybrid { p_cpus, e_cpus }
    }
}

// ── reading ────────────────────────────────────────────────────────────

fn read_avg_freq_khz(cpus: &[u32]) -> u64 {
    if cpus.is_empty() {
        return 0;
    }
    let mut total: u64 = 0;
    let mut count: u64 = 0;
    for &num in cpus {
        let path = format!("/sys/devices/system/cpu/cpu{num}/cpufreq/scaling_cur_freq");
        if let Some(khz) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            total += khz;
            count += 1;
        }
    }
    if count > 0 { total / count } else { 0 }
}

fn khz_to_ghz_str(khz: u64) -> String {
    format!("{}.{:02}", khz / 1_000_000, (khz % 1_000_000) / 10_000)
}

#[derive(Clone)]
enum FreqDisplay {
    Single(String),
    /// `(p_text, e_text)` — rendered with a vertical line separator between them.
    Split(String, String),
}

#[derive(Clone)]
struct FreqReading {
    display: FreqDisplay,
    /// Average frequency across *all* cores, in GHz. Used to drive the sparkline.
    avg_ghz: f32,
}

fn take_reading(layout: &CoreLayout) -> FreqReading {
    match layout {
        CoreLayout::Uniform { cpus } => {
            let khz = read_avg_freq_khz(cpus);
            FreqReading {
                display: FreqDisplay::Single(format!("{} GHz", khz_to_ghz_str(khz))),
                avg_ghz: khz as f32 / 1_000_000.0,
            }
        }
        CoreLayout::Hybrid { p_cpus, e_cpus } => {
            let p = read_avg_freq_khz(p_cpus);
            let e = read_avg_freq_khz(e_cpus);
            // Weighted average across P + E cores (true "average core frequency").
            let total_cores = (p_cpus.len() + e_cpus.len()) as u64;
            let avg_khz = if total_cores > 0 {
                (p * p_cpus.len() as u64 + e * e_cpus.len() as u64) / total_cores
            } else {
                0
            };
            FreqReading {
                display: FreqDisplay::Split(
                    format!("P:{}", khz_to_ghz_str(p)),
                    format!("E:{}", khz_to_ghz_str(e)),
                ),
                avg_ghz: avg_khz as f32 / 1_000_000.0,
            }
        }
    }
}

/// Detect the min/max scaling frequency of cpu0 to use as a fixed scale for the
/// sparkline. Falls back to a sensible default if sysfs isn't readable.
fn detect_freq_range_ghz() -> (f32, f32) {
    let read_ghz = |path: &str| -> Option<f32> {
        std::fs::read_to_string(path)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .map(|khz| khz as f32 / 1_000_000.0)
    };
    let min = read_ghz("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_min_freq").unwrap_or(0.4);
    let max = read_ghz("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").unwrap_or(5.0);
    (min, max)
}

// ── timerfd + epoll monitor ────────────────────────────────────────────

fn freq_monitor(layout: CoreLayout, tx: async_channel::Sender<FreqReading>) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 { return; }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 1, tv_nsec: 0 },
        it_value: libc::timespec { tv_sec: 0, tv_nsec: 1 },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 { return; }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: 0 };
    unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, tfd.as_raw_fd(), &mut ev) };

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
            break;
        }
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        let reading = take_reading(&layout);
        if tx.try_send(reading).is_err() && tx.is_closed() { break; }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct CpuFreq {
    display: FreqDisplay,
    history: VecDeque<f32>,
    min_freq_ghz: f32,
    max_freq_ghz: f32,
    grouped: bool,
}

impl BarWidget for CpuFreq {
    const NAME: &str = "cpu-freq";

    fn new(cx: &mut Context<Self>) -> Self {
        let layout = detect_layout();
        let desc = match &layout {
            CoreLayout::Uniform { cpus } => format!("uniform {} cores", cpus.len()),
            CoreLayout::Hybrid { p_cpus, e_cpus } => {
                format!("hybrid {}P+{}E cores", p_cpus.len(), e_cpus.len())
            }
        };
        let (min_freq_ghz, max_freq_ghz) = detect_freq_range_ghz();
        log::info!("cpu_freq: {desc}, range {min_freq_ghz:.2}–{max_freq_ghz:.2} GHz");

        let (tx, rx) = async_channel::bounded::<FreqReading>(1);

        std::thread::Builder::new()
            .name("cpu-freq".into())
            .spawn(move || freq_monitor(layout, tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(reading) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.display = reading.display;
                        this.history.push_back(reading.avg_ghz);
                        while this.history.len() > HISTORY_SIZE {
                            this.history.pop_front();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            display: FreqDisplay::Single(String::new()),
            history: VecDeque::with_capacity(HISTORY_SIZE),
            min_freq_ghz,
            max_freq_ghz,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();

        // Build sparkline: filled vertical bars, one per sample, right-aligned
        // (newest sample on the right). The top edge of the bars traces the
        // average core frequency over the last HISTORY_SIZE seconds.
        let bar_w: f32 = SPARK_W / HISTORY_SIZE as f32;
        let min_freq = self.min_freq_ghz;
        let range = (self.max_freq_ghz - self.min_freq_ghz).max(0.1);
        let history = self.history.clone();
        let n = history.len();
        // Right-align: leave empty space on the left if history isn't full yet.
        let x_offset = (HISTORY_SIZE - n) as f32 * bar_w;

        // Surface color used by the capsule background; the decay overlay fades
        // from this opaque color (left) to fully transparent (right) so the
        // oldest bars dissolve into the capsule background.
        let surface_opaque: Hsla = rgb(t.surface).into();
        let surface_clear = Hsla {
            a: 0.0,
            ..surface_opaque
        };
        // Fraction of the sparkline width covered by the decay gradient.
        let decay_w = SPARK_W * 0.55;

        let sparkline = div()
            .relative()
            .flex_shrink_0()
            .w(px(SPARK_W))
            .h(px(SPARK_H))
            .children(history.into_iter().enumerate().map(move |(i, ghz)| {
                let norm = ((ghz - min_freq) / range).clamp(0.0, 1.0);
                // Reserve a 1px floor so even idle samples are visible.
                let h = (norm * (SPARK_H - 1.0) + 1.0).max(1.0);
                let y = SPARK_H - h;
                let x = x_offset + i as f32 * bar_w;
                // Brighten newer samples slightly for a fading-in feel.
                let color = if (i + 1) == n { t.accent } else { t.accent_dim };
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(bar_w))
                    .h(px(h))
                    .bg(rgb(color))
            }))
            // Decay overlay: opaque surface on the left → transparent on the
            // right, drawn on top of all bars so the oldest samples fade out.
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(decay_w))
                    .h(px(SPARK_H))
                    .bg(linear_gradient(
                        90.0,
                        linear_color_stop(surface_opaque, 0.0),
                        linear_color_stop(surface_clear, 1.0),
                    )),
            );

        // Build the text portion. For hybrid CPUs we render P and E values
        // with a 1px vertical line between them — visually consistent with
        // the group!() macro separator.
        let content_h = crate::config::CONTENT_HEIGHT();
        let sep_h = ((content_h - 4.0) - 10.0).max(6.0);
        let text_el = match &self.display {
            FreqDisplay::Single(s) => div().text_color(rgb(t.fg)).child(s.clone()).into_any_element(),
            FreqDisplay::Split(p, e) => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(div().text_color(rgb(t.fg)).child(p.clone()))
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(1.0))
                        .h(px(sep_h))
                        .bg(rgb(t.fg_gutter)),
                )
                .child(div().text_color(rgb(t.fg)).child(e.clone()))
                .into_any_element(),
        };

        let spark_text_sep = div()
            .flex_shrink_0()
            .w(px(1.0))
            .h(px(sep_h))
            .bg(rgb(t.fg_gutter));

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(6.0))
                .text_xs()
                .gap(px(6.0))
                .child(sparkline)
                .child(spark_text_sep)
                .child(text_el),
            self.grouped,
        )
    }
}

impl_render!(CpuFreq);
