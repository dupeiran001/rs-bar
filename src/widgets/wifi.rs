//! Wi-Fi indicator widget.
//!
//! Reads state from sysfs/procfs (no subprocesses). Monitors link changes
//! via a netlink RTMGRP_LINK socket — the kernel pushes events on
//! connect/disconnect/rfkill. Epoll on the netlink fd for zero polling.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

#[derive(Clone, PartialEq)]
enum WifiState {
    Disabled,
    Disconnected,
    Connected { signal: u32 },
}

// ── sysfs / procfs readers (no subprocesses) ───────────────────────────

fn find_wifi_interface() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/net")
        .ok()?
        .filter_map(Result::ok)
    {
        if entry.path().join("wireless").is_dir() {
            return entry.file_name().to_str().map(|s| s.to_string());
        }
    }
    None
}

fn is_wifi_rfkill_blocked() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/rfkill") else {
        return false;
    };
    for entry in dir.filter_map(Result::ok) {
        let path = entry.path();
        let ty = std::fs::read_to_string(path.join("type")).unwrap_or_default();
        if ty.trim() != "wlan" {
            continue;
        }
        let state = std::fs::read_to_string(path.join("state")).unwrap_or_default();
        if state.trim() == "0" {
            return true;
        }
        let hard = std::fs::read_to_string(path.join("hard")).unwrap_or_default();
        if hard.trim() == "1" {
            return true;
        }
    }
    false
}

fn read_signal_strength(iface: &str) -> u32 {
    let wireless = std::fs::read_to_string("/proc/net/wireless").unwrap_or_default();
    for line in wireless.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(iface) {
            // Format: "iface: status  link  level  noise ..."
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                return parts[2]
                    .trim_end_matches('.')
                    .parse::<u32>()
                    .unwrap_or(0)
                    .min(100);
            }
        }
    }
    0
}

fn read_wifi_state() -> WifiState {
    if is_wifi_rfkill_blocked() {
        return WifiState::Disabled;
    }
    let Some(iface) = find_wifi_interface() else {
        return WifiState::Disconnected;
    };
    let operstate =
        std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate")).unwrap_or_default();
    if operstate.trim() != "up" {
        return WifiState::Disconnected;
    }
    WifiState::Connected {
        signal: read_signal_strength(&iface),
    }
}

// ── netlink monitor ────────────────────────────────────────────────────

fn netlink_monitor(tx: async_channel::Sender<()>) {
    // Create netlink socket subscribed to link state changes
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        log::warn!("wifi: netlink socket: {}", std::io::Error::last_os_error());
        return;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = libc::RTMGRP_LINK as u32;
    if unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        )
    } < 0
    {
        log::warn!("wifi: netlink bind: {}", std::io::Error::last_os_error());
        return;
    }

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 0,
    };
    if unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, fd.as_raw_fd(), &mut ev) }
        < 0
    {
        return;
    }

    log::info!("wifi: netlink monitor started");

    let mut buf = [0u8; 4096];
    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n =
            unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }

        // Drain the socket
        loop {
            let n = unsafe {
                libc::recv(
                    fd.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n <= 0 {
                break;
            }
        }

        // Debounce: coalesce rapid events (connect produces several)
        std::thread::sleep(std::time::Duration::from_millis(100));
        // Drain anything that arrived during debounce
        loop {
            let n = unsafe {
                libc::recv(
                    fd.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n <= 0 {
                break;
            }
        }

        if tx.try_send(()).is_err() {
            // Channel full or closed
            if tx.is_closed() {
                break;
            }
        }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

const WIFI_OFF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-off.svg");
const WIFI_WEAK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-weak.svg");
const WIFI_FAIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-fair.svg");
const WIFI_GOOD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-good.svg");
const WIFI_EXCELLENT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-excellent.svg");

fn wifi_icon(state: &WifiState) -> &'static str {
    match state {
        WifiState::Disabled | WifiState::Disconnected => WIFI_OFF,
        WifiState::Connected { signal } if *signal >= 80 => WIFI_EXCELLENT,
        WifiState::Connected { signal } if *signal >= 60 => WIFI_GOOD,
        WifiState::Connected { signal } if *signal >= 40 => WIFI_FAIR,
        WifiState::Connected { signal } if *signal >= 20 => WIFI_WEAK,
        WifiState::Connected { .. } => WIFI_OFF,
    }
}

pub struct Wifi {
    state: WifiState,
}

impl BarWidget for Wifi {
    const NAME: &str = "wifi";

    fn new(cx: &mut Context<Self>) -> Self {
        let initial = read_wifi_state();
        let (tx, rx) = async_channel::bounded::<()>(1);

        std::thread::Builder::new()
            .name("wifi-netlink".into())
            .spawn(move || netlink_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let new = read_wifi_state();
                if this
                    .update(cx, |this, cx| {
                        if this.state != new {
                            this.state = new;
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

        Self { state: initial }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;

        let icon_path = wifi_icon(&self.state);
        let color = match &self.state {
            WifiState::Disabled | WifiState::Disconnected => t.fg_dark,
            WifiState::Connected { signal } if *signal >= 60 => t.fg,
            WifiState::Connected { signal } if *signal >= 20 => t.yellow,
            WifiState::Connected { .. } => t.red,
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .px(px(6.0))
            .child(
                svg()
                    .external_path(icon_path.to_string())
                    .size(px(crate::config::ICON_SIZE))
                    .text_color(rgb(color))
                    .flex_shrink_0(),
            )
    }
}

impl_render!(Wifi);
