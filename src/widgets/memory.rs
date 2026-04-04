//! Memory usage widget.
//!
//! Reads `/proc/meminfo` every 2 seconds via `timerfd` + `epoll`.
//! Displays used percentage: `(MemTotal - MemAvailable) / MemTotal * 100`.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

const ICON_MEM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/memory.svg");

fn read_mem_usage() -> f32 {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total: u64 = 0;
    let mut available: u64 = 0;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
        }
        if total > 0 && available > 0 {
            break;
        }
    }
    if total == 0 {
        return 0.0;
    }
    ((total - available) as f32 / total as f32) * 100.0
}

fn mem_monitor(tx: async_channel::Sender<f32>) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 { return; }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 2, tv_nsec: 0 },
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

        let usage = read_mem_usage();
        if tx.try_send(usage).is_err() && tx.is_closed() { break; }
    }
}

pub struct Memory {
    usage: f32,
}

impl BarWidget for Memory {
    const NAME: &str = "memory";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<f32>(1);

        std::thread::Builder::new()
            .name("memory".into())
            .spawn(move || mem_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(usage) = rx.recv().await {
                if this.update(cx, |this, cx| { this.usage = usage; cx.notify(); }).is_err() {
                    break;
                }
            }
        }).detach();

        Self { usage: 0.0 }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let icon_size = crate::config::ICON_SIZE;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let pct = self.usage.round() as u32;

        let color = if self.usage >= 90.0 {
            t.red
        } else if self.usage >= 75.0 {
            t.orange
        } else {
            t.fg
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
                    .external_path(ICON_MEM.to_string())
                    .size(px(icon_size))
                    .text_color(rgb(color))
                    .flex_shrink_0(),
            )
            .child(div().text_color(rgb(color)).child(format!("{:>2}%", pct)))
    }
}

impl_render!(Memory);
