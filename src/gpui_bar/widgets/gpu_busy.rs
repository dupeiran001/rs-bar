//! GPU busy percent widget.
//!
//! Reads AMD `gpu_busy_percent` or Intel `gt_busy_percent` from
//! `/sys/class/drm/*/device/`. Uses `timerfd` + `epoll` (1s interval).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use crate::gpui_bar::hub::Broadcast;

use super::{BarWidget, impl_render};

const ICON_GPU_BUSY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/gpu-busy.svg");

enum GpuBusySource {
    /// Direct percentage file (e.g. AMD `gpu_busy_percent`, older Intel `gt_busy_percent`).
    Direct { path: PathBuf },
    /// xe driver: compute busy% from `idle_residency_ms` deltas.
    Residency { path: PathBuf },
}

fn detect_gpu_busy() -> Option<GpuBusySource> {
    for entry in std::fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(Result::ok)
    {
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
                return Some(GpuBusySource::Direct { path });
            }
        }
        // xe driver (Intel Battlemage / Arc): use gtidle residency
        if vendor.trim() == "0x8086" {
            for tile in std::fs::read_dir(dev.join("tile0"))
                .ok()?
                .filter_map(Result::ok)
            {
                let residency = tile.path().join("gtidle/idle_residency_ms");
                if residency.exists() && std::fs::read_to_string(&residency).is_ok() {
                    return Some(GpuBusySource::Residency { path: residency });
                }
            }
        }
    }
    None
}

fn read_busy_direct(path: &PathBuf) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_residency_ms(path: &PathBuf) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn broadcast() -> Option<&'static Broadcast<u32>> {
    static BC: OnceLock<Option<Broadcast<u32>>> = OnceLock::new();
    BC.get_or_init(|| {
        let src = detect_gpu_busy()?;
        let src_path = match &src {
            GpuBusySource::Direct { path } | GpuBusySource::Residency { path } => {
                path.display().to_string()
            }
        };
        log::info!("gpu-busy: {src_path}");
        let bc = Broadcast::<u32>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("gpu-busy".into())
            .spawn(move || gpu_busy_monitor(src, producer))
            .ok();
        Some(bc)
    })
    .as_ref()
}

fn gpu_busy_monitor(src: GpuBusySource, bc: Broadcast<u32>) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: 1,
            tv_nsec: 0,
        },
        it_value: libc::timespec {
            tv_sec: 0,
            tv_nsec: 1,
        },
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
    unsafe {
        libc::epoll_ctl(
            epfd.as_raw_fd(),
            libc::EPOLL_CTL_ADD,
            tfd.as_raw_fd(),
            &mut ev,
        )
    };

    let mut prev_residency: Option<u64> = match &src {
        GpuBusySource::Residency { path } => read_residency_ms(path),
        _ => None,
    };

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        let pct = match &src {
            GpuBusySource::Direct { path } => read_busy_direct(path),
            GpuBusySource::Residency { path } => {
                let cur = read_residency_ms(path);
                let result = match (prev_residency, cur) {
                    (Some(prev), Some(cur)) if cur >= prev => {
                        let idle_ms = cur - prev;
                        // interval is 1000ms; clamp to 0..100
                        Some(100u32.saturating_sub(idle_ms.min(1000) as u32 * 100 / 1000))
                    }
                    _ => None,
                };
                prev_residency = cur;
                result
            }
        };
        if let Some(pct) = pct {
            bc.publish(pct);
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
        if let Some(bc) = broadcast() {
            let sub = bc.subscribe();
            cx.spawn(async move |this, cx| {
                while let Some(pct) = sub.next().await {
                    if this
                        .update(cx, |this, cx| {
                            if this.pct != Some(pct) {
                                this.pct = Some(pct);
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }

        Self {
            pct: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let icon_size = crate::gpui_bar::config::ICON_SIZE();

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
