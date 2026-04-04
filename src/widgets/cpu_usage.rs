//! CPU usage widget.
//!
//! Reads `/proc/stat` every second via `timerfd` + `epoll`.
//! Computes delta between consecutive samples for accurate usage %.
//! All sysfs/procfs reads — zero subprocesses.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

const ICON_CPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/cpu-usage.svg");

// ── /proc/stat reader ──────────────────────────────────────────────────

/// Raw CPU time counters from `/proc/stat` (aggregate "cpu" line).
#[derive(Clone, Copy, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    fn idle_total(&self) -> u64 {
        self.idle + self.iowait
    }
}

fn read_cpu_times() -> CpuTimes {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    for line in stat.lines() {
        // The aggregate line starts with "cpu " (with trailing space)
        if let Some(rest) = line.strip_prefix("cpu ") {
            let vals: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if vals.len() >= 8 {
                return CpuTimes {
                    user: vals[0],
                    nice: vals[1],
                    system: vals[2],
                    idle: vals[3],
                    iowait: vals[4],
                    irq: vals[5],
                    softirq: vals[6],
                    steal: vals[7],
                };
            }
        }
    }
    CpuTimes::default()
}

fn compute_usage(prev: &CpuTimes, cur: &CpuTimes) -> f32 {
    let dt = cur.total().saturating_sub(prev.total());
    if dt == 0 {
        return 0.0;
    }
    let di = cur.idle_total().saturating_sub(prev.idle_total());
    ((dt - di) as f32 / dt as f32) * 100.0
}

// ── timerfd + epoll monitor ────────────────────────────────────────────

fn cpu_monitor(tx: async_channel::Sender<f32>) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        log::warn!("cpu_usage: timerfd_create: {}", std::io::Error::last_os_error());
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    // 1-second interval
    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 1, tv_nsec: 0 },
        it_value: libc::timespec { tv_sec: 1, tv_nsec: 0 },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 0,
    };
    unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, tfd.as_raw_fd(), &mut ev) };

    let mut prev = read_cpu_times();

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }

        // Consume timerfd
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        let cur = read_cpu_times();
        let usage = compute_usage(&prev, &cur);
        prev = cur;

        if tx.try_send(usage).is_err() && tx.is_closed() {
            break;
        }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct CpuUsage {
    usage: f32,
}

impl BarWidget for CpuUsage {
    const NAME: &str = "cpu-usage";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<f32>(1);

        std::thread::Builder::new()
            .name("cpu-usage".into())
            .spawn(move || cpu_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(usage) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.usage = usage;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self { usage: 0.0 }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let icon_size = crate::config::ICON_SIZE;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let pct = self.usage.round() as u32;

        let color = if self.usage >= 80.0 {
            t.red
        } else if self.usage >= 60.0 {
            t.orange
        } else if self.usage >= 25.0 {
            t.fg
        } else {
            t.fg_dark
        };

        div()
            .flex()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .px(px(4.0))
            .gap(px(4.0))
            .text_xs()
            .child(
                svg()
                    .external_path(ICON_CPU.to_string())
                    .size(px(icon_size))
                    .text_color(rgb(color))
                    .flex_shrink_0(),
            )
            .child(
                div()
                    .text_color(rgb(color))
                    .child(format!("{:>2}%", pct)),
            )
    }
}

impl_render!(CpuUsage);
