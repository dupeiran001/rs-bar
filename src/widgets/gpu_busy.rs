//! GPU busy percent widget.
//!
//! Reads AMD `gpu_busy_percent` or Intel `gt_busy_percent` from
//! `/sys/class/drm/*/device/`. Uses `timerfd` + `epoll` (1s interval).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

const ICON_GPU_BUSY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/gpu-busy.svg");

struct GpuBusySource {
    path: PathBuf,
}

fn detect_gpu_busy() -> Option<GpuBusySource> {
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.filter_map(Result::ok) {
        let dev = entry.path().join("device");
        if !dev.is_dir() {
            continue;
        }
        let class = std::fs::read_to_string(dev.join("class")).unwrap_or_default();
        if !class.trim().starts_with("0x03") {
            continue;
        }
        let vendor = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
        let files = match vendor.trim() {
            "0x1002" => vec!["gpu_busy_percent"],
            "0x8086" => vec!["gpu_busy_percent", "gt_busy_percent"],
            _ => continue,
        };
        for f in files {
            let path = dev.join(f);
            if path.exists() && std::fs::read_to_string(&path).is_ok() {
                return Some(GpuBusySource { path });
            }
        }
    }
    None
}

fn read_busy(src: &GpuBusySource) -> Option<u32> {
    std::fs::read_to_string(&src.path)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn gpu_busy_monitor(src: GpuBusySource, tx: async_channel::Sender<u32>) {
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

        if let Some(pct) = read_busy(&src) {
            if tx.try_send(pct).is_err() && tx.is_closed() { break; }
        }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct GpuBusy {
    pct: Option<u32>,
    grouped: bool,
}

impl BarWidget for GpuBusy {
    const NAME: &str = "gpu-busy";

    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(src) = detect_gpu_busy() {
            log::info!("gpu-busy: {}", src.path.display());
            let (tx, rx) = async_channel::bounded::<u32>(1);

            std::thread::Builder::new()
                .name("gpu-busy".into())
                .spawn(move || gpu_busy_monitor(src, tx))
                .ok();

            cx.spawn(async move |this, cx| {
                while let Ok(pct) = rx.recv().await {
                    if this.update(cx, |this, cx| { this.pct = Some(pct); cx.notify(); }).is_err() {
                        break;
                    }
                }
            }).detach();
        } else {
            log::info!("gpu-busy: no source found");
        }

        Self {
            pct: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let icon_size = crate::config::ICON_SIZE();

        let Some(pct) = self.pct else {
            return super::capsule(div(), self.grouped);
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .gap(px(3.0))
                .child(
                    svg()
                        .external_path(ICON_GPU_BUSY.to_string())
                        .size(px(icon_size))
                        .text_color(rgb(t.text_dim))
                        .flex_shrink_0(),
                )
                .child(div().text_color(rgb(t.text_dim)).child(format!("{pct}%"))),
            self.grouped,
        )
    }
}

impl_render!(GpuBusy);
