//! Wi-Fi indicator widget.
//!
//! Reads state from sysfs/procfs/NL80211 (no subprocesses). Monitors link
//! changes via a netlink RTMGRP_LINK socket and polls signal strength via
//! timerfd — both on a single epoll instance.
//!
//! Signal strength: tries `/proc/net/wireless` first (fast, works on
//! Intel/Atheros), falls back to NL80211 generic netlink (works on
//! Broadcom/Asahi and other drivers that don't populate procfs).

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
    // Fallback: check ieee80211 phys and match via phy80211 symlink
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.filter_map(Result::ok) {
            if entry.path().join("phy80211").exists() {
                return entry.file_name().to_str().map(|s| s.to_string());
            }
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

fn read_signal_procfs(iface: &str) -> Option<u32> {
    let wireless = std::fs::read_to_string("/proc/net/wireless").ok()?;
    for line in wireless.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(iface) {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let val = parts[2]
                    .trim_end_matches('.')
                    .parse::<u32>()
                    .unwrap_or(0)
                    .min(100);
                if val > 0 {
                    return Some(val);
                }
            }
        }
    }
    None
}

// ── NL80211 generic netlink signal reader ──────────────────────────────

const NETLINK_GENERIC: i32 = 16;
const GENL_ID_CTRL: u16 = 0x10;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_FAMILY_ID: u16 = 1;

const NL80211_CMD_GET_STATION: u8 = 17;
const NL80211_ATTR_IFINDEX: u16 = 3;
const NL80211_ATTR_STA_INFO: u16 = 21;
const NL80211_STA_INFO_SIGNAL: u16 = 7;

const NLM_F_REQUEST: u16 = 1;
const NLM_F_DUMP: u16 = 0x300;
const NLMSG_DONE: u16 = 3;
const NLMSG_ERROR: u16 = 2;

fn nla_align(len: usize) -> usize {
    (len + 3) & !3
}

fn nla_put(buf: &mut Vec<u8>, nla_type: u16, data: &[u8]) {
    let nla_len = (4 + data.len()) as u16;
    buf.extend_from_slice(&nla_len.to_ne_bytes());
    buf.extend_from_slice(&nla_type.to_ne_bytes());
    buf.extend_from_slice(data);
    let pad = nla_align(nla_len as usize) - nla_len as usize;
    buf.extend(std::iter::repeat_n(0u8, pad));
}

fn get_ifindex(iface: &str) -> Option<i32> {
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if sock < 0 {
        return None;
    }
    let sock = unsafe { OwnedFd::from_raw_fd(sock) };

    // struct ifreq: IFNAMSIZ(16) + union(24) = 40 bytes
    let mut ifr = [0u8; 40];
    let bytes = iface.as_bytes();
    let len = bytes.len().min(15);
    ifr[..len].copy_from_slice(&bytes[..len]);

    if unsafe { libc::ioctl(sock.as_raw_fd(), libc::SIOCGIFINDEX, ifr.as_mut_ptr()) } < 0 {
        return None;
    }
    // ifr_ifindex is at offset 16
    Some(i32::from_ne_bytes([ifr[16], ifr[17], ifr[18], ifr[19]]))
}

/// Build a netlink message: nlmsghdr + genlmsghdr + attributes
fn build_genl_msg(msg_type: u16, flags: u16, cmd: u8, seq: u32, attrs: &[u8]) -> Vec<u8> {
    let total_len = 16 + 4 + attrs.len(); // nlmsghdr(16) + genlmsghdr(4) + attrs
    let mut buf = Vec::with_capacity(total_len);
    // nlmsghdr
    buf.extend_from_slice(&(total_len as u32).to_ne_bytes());
    buf.extend_from_slice(&msg_type.to_ne_bytes());
    buf.extend_from_slice(&flags.to_ne_bytes());
    buf.extend_from_slice(&seq.to_ne_bytes());
    buf.extend_from_slice(&0u32.to_ne_bytes()); // pid = 0
    // genlmsghdr
    buf.push(cmd);
    buf.push(1); // version
    buf.extend_from_slice(&[0u8; 2]); // reserved
    // attrs
    buf.extend_from_slice(attrs);
    buf
}

fn resolve_nl80211_family(fd: i32) -> Option<u16> {
    let mut attrs = Vec::new();
    let name = b"nl80211\0";
    nla_put(&mut attrs, CTRL_ATTR_FAMILY_NAME, name);
    let msg = build_genl_msg(GENL_ID_CTRL, NLM_F_REQUEST, CTRL_CMD_GETFAMILY, 1, &attrs);

    if unsafe { libc::send(fd, msg.as_ptr().cast(), msg.len(), 0) } < 0 {
        return None;
    }

    let mut buf = [0u8; 4096];
    let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
    if n < 20 {
        return None;
    }
    let data = &buf[..n as usize];

    // Parse: skip nlmsghdr(16) + genlmsghdr(4), iterate attrs
    let msg_type = u16::from_ne_bytes([data[4], data[5]]);
    if msg_type == NLMSG_ERROR {
        return None;
    }

    let mut off = 20;
    while off + 4 <= data.len() {
        let nla_len = u16::from_ne_bytes([data[off], data[off + 1]]) as usize;
        let nla_type = u16::from_ne_bytes([data[off + 2], data[off + 3]]) & 0x7FFF;
        if nla_len < 4 {
            break;
        }
        if nla_type == CTRL_ATTR_FAMILY_ID && nla_len >= 6 {
            return Some(u16::from_ne_bytes([data[off + 4], data[off + 5]]));
        }
        off += nla_align(nla_len);
    }
    None
}

fn parse_nested_signal(data: &[u8]) -> Option<i8> {
    let mut off = 0;
    while off + 4 <= data.len() {
        let nla_len = u16::from_ne_bytes([data[off], data[off + 1]]) as usize;
        let nla_type = u16::from_ne_bytes([data[off + 2], data[off + 3]]) & 0x7FFF;
        if nla_len < 4 {
            break;
        }
        if nla_type == NL80211_STA_INFO_SIGNAL && nla_len >= 5 {
            return Some(data[off + 4] as i8);
        }
        off += nla_align(nla_len);
    }
    None
}

fn dbm_to_quality(dbm: i8) -> u32 {
    if dbm >= -50 {
        100
    } else if dbm <= -100 {
        0
    } else {
        (2 * (dbm as i32 + 100)) as u32
    }
}

fn read_signal_nl80211(iface: &str) -> Option<u32> {
    let ifindex = get_ifindex(iface)?;

    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            NETLINK_GENERIC,
        )
    };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    // Bind
    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        );
    }

    // Set receive timeout (500ms) to avoid hanging
    let tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 500_000,
    };
    unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        );
    }

    let family_id = resolve_nl80211_family(fd.as_raw_fd())?;

    // Build GET_STATION dump request
    let mut attrs = Vec::new();
    nla_put(&mut attrs, NL80211_ATTR_IFINDEX, &ifindex.to_ne_bytes());
    let msg = build_genl_msg(
        family_id,
        NLM_F_REQUEST | NLM_F_DUMP,
        NL80211_CMD_GET_STATION,
        2,
        &attrs,
    );

    if unsafe { libc::send(fd.as_raw_fd(), msg.as_ptr().cast(), msg.len(), 0) } < 0 {
        return None;
    }

    // Receive and parse multi-part response
    let mut buf = [0u8; 8192];
    for _ in 0..8 {
        let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n <= 0 {
            return None;
        }
        let data = &buf[..n as usize];

        let mut off = 0;
        while off + 16 <= data.len() {
            let msg_len = u32::from_ne_bytes(data[off..off + 4].try_into().ok()?) as usize;
            if msg_len < 16 || off + msg_len > data.len() {
                break;
            }
            let msg_type = u16::from_ne_bytes(data[off + 4..off + 6].try_into().ok()?);

            if msg_type == NLMSG_DONE || msg_type == NLMSG_ERROR {
                return None;
            }

            if msg_type == family_id && msg_len > 20 {
                // Parse top-level attributes for NL80211_ATTR_STA_INFO
                let attr_start = off + 20;
                let attr_end = off + msg_len;
                let mut aoff = attr_start;
                while aoff + 4 <= attr_end {
                    let nla_len =
                        u16::from_ne_bytes([data[aoff], data[aoff + 1]]) as usize;
                    let nla_type =
                        u16::from_ne_bytes([data[aoff + 2], data[aoff + 3]]) & 0x7FFF;
                    if nla_len < 4 {
                        break;
                    }
                    if nla_type == NL80211_ATTR_STA_INFO && nla_len > 4 {
                        // Nested: parse inner attrs for signal
                        if let Some(dbm) = parse_nested_signal(&data[aoff + 4..aoff + nla_len]) {
                            return Some(dbm_to_quality(dbm));
                        }
                    }
                    aoff += nla_align(nla_len);
                }
            }

            off += nla_align(msg_len);
        }
    }
    None
}

/// Read signal strength: try procfs first, fall back to NL80211.
fn read_signal_strength(iface: &str) -> u32 {
    if let Some(sig) = read_signal_procfs(iface) {
        return sig;
    }
    if let Some(sig) = read_signal_nl80211(iface) {
        return sig;
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

// ── epoll monitor: RTMGRP_LINK + timerfd ──────────────────────────────

const TAG_NETLINK: u64 = 0;
const TAG_TIMER: u64 = 1;

fn wifi_monitor(tx: async_channel::Sender<WifiState>) {
    // ── netlink RTMGRP_LINK socket ──
    let nl_fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if nl_fd < 0 {
        log::warn!("wifi: netlink socket: {}", std::io::Error::last_os_error());
        return;
    }
    let nl_fd = unsafe { OwnedFd::from_raw_fd(nl_fd) };

    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    addr.nl_groups = libc::RTMGRP_LINK as u32;
    if unsafe {
        libc::bind(
            nl_fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        )
    } < 0
    {
        log::warn!("wifi: netlink bind: {}", std::io::Error::last_os_error());
        return;
    }

    // ── timerfd for periodic signal polling (5s) ──
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        log::warn!("wifi: timerfd: {}", std::io::Error::last_os_error());
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: 5,
            tv_nsec: 0,
        },
        it_value: libc::timespec {
            tv_sec: 5,
            tv_nsec: 0,
        },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    // ── epoll watching both fds ──
    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev_nl = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: TAG_NETLINK,
    };
    let mut ev_tm = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: TAG_TIMER,
    };
    unsafe {
        libc::epoll_ctl(
            epfd.as_raw_fd(),
            libc::EPOLL_CTL_ADD,
            nl_fd.as_raw_fd(),
            &mut ev_nl,
        );
        libc::epoll_ctl(
            epfd.as_raw_fd(),
            libc::EPOLL_CTL_ADD,
            tfd.as_raw_fd(),
            &mut ev_tm,
        );
    }

    log::info!("wifi: epoll monitor started (netlink + timerfd)");

    let mut drain_buf = [0u8; 4096];
    let mut prev_state = WifiState::Disconnected;

    loop {
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 2];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), events.as_mut_ptr(), 2, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }

        for i in 0..n as usize {
            match events[i].u64 {
                TAG_NETLINK => {
                    // Drain netlink socket
                    loop {
                        let r = unsafe {
                            libc::recv(
                                nl_fd.as_raw_fd(),
                                drain_buf.as_mut_ptr().cast(),
                                drain_buf.len(),
                                libc::MSG_DONTWAIT,
                            )
                        };
                        if r <= 0 {
                            break;
                        }
                    }
                    // Debounce rapid link events
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    loop {
                        let r = unsafe {
                            libc::recv(
                                nl_fd.as_raw_fd(),
                                drain_buf.as_mut_ptr().cast(),
                                drain_buf.len(),
                                libc::MSG_DONTWAIT,
                            )
                        };
                        if r <= 0 {
                            break;
                        }
                    }
                }
                TAG_TIMER => {
                    // Consume timerfd expiration
                    let mut tbuf = [0u8; 8];
                    unsafe { libc::read(tfd.as_raw_fd(), tbuf.as_mut_ptr().cast(), 8) };
                }
                _ => {}
            }
        }

        // Re-read state after any event
        let new_state = read_wifi_state();
        if new_state != prev_state {
            prev_state = new_state.clone();
            if tx.try_send(new_state).is_err() && tx.is_closed() {
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
const WIFI_EXCELLENT: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/wifi-excellent.svg");

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
    grouped: bool,
}

impl BarWidget for Wifi {
    const NAME: &str = "wifi";

    fn new(cx: &mut Context<Self>) -> Self {
        let initial = read_wifi_state();
        let (tx, rx) = async_channel::bounded::<WifiState>(1);

        std::thread::Builder::new()
            .name("wifi-monitor".into())
            .spawn(move || wifi_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(new) = rx.recv().await {
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

        Self {
            state: initial,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();

        let icon_path = wifi_icon(&self.state);
        let color = match &self.state {
            WifiState::Disabled | WifiState::Disconnected => t.fg_dark,
            WifiState::Connected { signal } if *signal >= 60 => t.fg,
            WifiState::Connected { signal } if *signal >= 20 => t.yellow,
            WifiState::Connected { .. } => t.red,
        };

        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let button_h = content_h - 4.0;

        super::capsule(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(button_h))
                .child(
                    svg()
                        .external_path(icon_path.to_string())
                        .size(px(crate::gpui_bar::config::ICON_SIZE()))
                        .text_color(rgb(color))
                        .flex_shrink_0(),
                ),
            self.grouped,
        )
    }
}

impl_render!(Wifi);
