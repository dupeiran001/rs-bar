//! Wi-Fi hub. Publishes connection state plus a scanned list of nearby
//! networks.
//!
//! The connection state half mirrors rs-bar's GPUI widget: it reads from
//! sysfs/procfs/NL80211 with no subprocesses on the hot path and is updated by
//! a netlink RTMGRP_LINK socket plus a 5 s timerfd, both watched on a single
//! epoll instance. This keeps the bar icon updates cheap and instant.
//!
//! The networks-list half is unique to the relm4 popover. We shell out to
//! `nmcli` periodically (15 s) to obtain the list of known/visible networks
//! and merge it into the published state. Connect / disconnect / refresh
//! commands are also nmcli-based and run on dedicated detached threads so they
//! never block the hub loop or the GTK main loop.
//!
//! Singleton background thread (`"wifi"`) shared across every bar instance.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::OnceLock;
use std::sync::mpsc;

use tokio::sync::watch;

/// Currently-connected network (matches rs-bar's `Connected { signal }`
/// variant, plus the SSID we now also need for the popover).
#[derive(Clone, Debug)]
pub struct ConnectedNetwork {
    pub ssid: String,
    /// Signal strength as a 0..=100 percent, normalised the same way rs-bar
    /// normalises procfs / NL80211 dBm readings.
    pub signal: i32,
}

/// One row in the popover's network list. `known` and `secured` come from
/// nmcli; `signal` is 0..=100 percent.
#[derive(Clone, Debug)]
pub struct KnownNetwork {
    pub ssid: String,
    pub signal: i32,
    pub known: bool,
    pub secured: bool,
}

/// Published state. `networks` is non-empty only after the first nmcli scan
/// succeeds; widgets that only care about connection state should look at
/// `enabled` and `connected` first.
#[derive(Clone, Debug, Default)]
pub struct WifiState {
    pub enabled: bool,
    pub connected: Option<ConnectedNetwork>,
    pub networks: Vec<KnownNetwork>,
}

// ────────── sysfs / procfs / rfkill helpers (no subprocess) ──────────

fn find_wifi_interface() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/net")
        .ok()?
        .filter_map(Result::ok)
    {
        if entry.path().join("wireless").is_dir() {
            return entry.file_name().to_str().map(|s| s.to_string());
        }
    }
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

fn read_signal_procfs(iface: &str) -> Option<i32> {
    let wireless = std::fs::read_to_string("/proc/net/wireless").ok()?;
    for line in wireless.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(iface) {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let val = parts[2]
                    .trim_end_matches('.')
                    .parse::<i32>()
                    .unwrap_or(0)
                    .clamp(0, 100);
                if val > 0 {
                    return Some(val);
                }
            }
        }
    }
    None
}

// ────────── NL80211 generic netlink signal reader ──────────

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
    Some(i32::from_ne_bytes([ifr[16], ifr[17], ifr[18], ifr[19]]))
}

fn build_genl_msg(msg_type: u16, flags: u16, cmd: u8, seq: u32, attrs: &[u8]) -> Vec<u8> {
    let total_len = 16 + 4 + attrs.len();
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(&(total_len as u32).to_ne_bytes());
    buf.extend_from_slice(&msg_type.to_ne_bytes());
    buf.extend_from_slice(&flags.to_ne_bytes());
    buf.extend_from_slice(&seq.to_ne_bytes());
    buf.extend_from_slice(&0u32.to_ne_bytes());
    buf.push(cmd);
    buf.push(1);
    buf.extend_from_slice(&[0u8; 2]);
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

fn dbm_to_quality(dbm: i8) -> i32 {
    if dbm >= -50 {
        100
    } else if dbm <= -100 {
        0
    } else {
        2 * (dbm as i32 + 100)
    }
}

fn read_signal_nl80211(iface: &str) -> Option<i32> {
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

    let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    addr.nl_family = libc::AF_NETLINK as u16;
    unsafe {
        libc::bind(
            fd.as_raw_fd(),
            &addr as *const _ as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as u32,
        );
    }

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
                let attr_start = off + 20;
                let attr_end = off + msg_len;
                let mut aoff = attr_start;
                while aoff + 4 <= attr_end {
                    let nla_len = u16::from_ne_bytes([data[aoff], data[aoff + 1]]) as usize;
                    let nla_type = u16::from_ne_bytes([data[aoff + 2], data[aoff + 3]]) & 0x7FFF;
                    if nla_len < 4 {
                        break;
                    }
                    if nla_type == NL80211_ATTR_STA_INFO && nla_len > 4 {
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

fn read_signal_strength(iface: &str) -> i32 {
    if let Some(sig) = read_signal_procfs(iface) {
        return sig;
    }
    if let Some(sig) = read_signal_nl80211(iface) {
        return sig;
    }
    0
}

// ────────── nmcli helpers (network list + connect/disconnect/refresh) ──

/// Split an nmcli `-t` (terse) row, respecting `\:` escaped colons. nmcli
/// terse output uses `:` as the field separator and escapes literal colons
/// (and backslashes) with a leading backslash.
fn split_nmcli_terse(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(nc) = chars.next() {
                cur.push(nc);
            }
        } else if c == ':' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Set of SSIDs that have a saved nmcli connection (i.e. "known"). Used to
/// flag rows in the popover so the user can see which ones will autoconnect.
fn nmcli_known_ssids() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let Ok(out) = std::process::Command::new("nmcli")
        .args(["-t", "-f", "NAME,TYPE", "con", "show"])
        .output()
    else {
        return set;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let parts = split_nmcli_terse(line);
        if parts.len() >= 2 && parts[1] == "802-11-wireless" {
            set.insert(parts[0].clone());
        }
    }
    set
}

/// Run an nmcli scan and parse the result into a deduplicated, signal-sorted
/// list of `KnownNetwork` rows. Empty SSIDs are dropped.
fn nmcli_scan_networks() -> Vec<KnownNetwork> {
    let known = nmcli_known_ssids();
    let Ok(out) = std::process::Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "IN-USE,SSID,SIGNAL,SECURITY",
            "device",
            "wifi",
            "list",
        ])
        .output()
    else {
        return Vec::new();
    };

    // nmcli can return duplicate SSID rows (one per BSSID). Keep the strongest.
    let mut by_ssid: std::collections::HashMap<String, KnownNetwork> =
        std::collections::HashMap::new();

    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let parts = split_nmcli_terse(line);
        if parts.len() < 4 {
            continue;
        }
        let ssid = parts[1].trim().to_string();
        if ssid.is_empty() {
            continue;
        }
        let signal: i32 = parts[2].trim().parse().unwrap_or(0);
        let security = parts[3].trim();
        let secured = !security.is_empty() && security != "--";
        let entry = KnownNetwork {
            known: known.contains(&ssid),
            secured,
            signal,
            ssid: ssid.clone(),
        };
        by_ssid
            .entry(ssid)
            .and_modify(|cur| {
                if entry.signal > cur.signal {
                    *cur = entry.clone();
                }
            })
            .or_insert(entry);
    }

    let mut list: Vec<KnownNetwork> = by_ssid.into_values().collect();
    list.sort_by(|a, b| b.signal.cmp(&a.signal).then_with(|| a.ssid.cmp(&b.ssid)));
    list
}

/// Best-effort SSID of the current connection. Returns `None` if nothing is
/// connected or nmcli isn't available.
fn nmcli_active_ssid() -> Option<String> {
    let out = std::process::Command::new("nmcli")
        .args([
            "-t",
            "-f",
            "ACTIVE,SSID",
            "device",
            "wifi",
            "list",
            "--rescan",
            "no",
        ])
        .output()
        .ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let parts = split_nmcli_terse(line);
        if parts.len() >= 2 && parts[0] == "yes" && !parts[1].is_empty() {
            return Some(parts[1].clone());
        }
    }
    None
}

// ────────── command channel: connect / disconnect / refresh ──────────

#[derive(Debug)]
enum Cmd {
    Connect(String),
    Disconnect,
    Refresh,
}

fn cmd_sender() -> &'static mpsc::Sender<Cmd> {
    static S: OnceLock<mpsc::Sender<Cmd>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("wifi-cmd".into())
            .spawn(move || {
                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Cmd::Connect(ssid) => {
                            let _ = std::process::Command::new("nmcli")
                                .args(["device", "wifi", "connect", &ssid])
                                .output();
                            let _ = sender().send(snapshot_now());
                        }
                        Cmd::Disconnect => {
                            if let Some(iface) = find_wifi_interface() {
                                let _ = std::process::Command::new("nmcli")
                                    .args(["device", "disconnect", &iface])
                                    .output();
                            }
                            let _ = sender().send(snapshot_now());
                        }
                        Cmd::Refresh => {
                            let _ = std::process::Command::new("nmcli")
                                .args(["device", "wifi", "rescan"])
                                .output();
                            let _ = sender().send(snapshot_now());
                        }
                    }
                }
            })
            .ok();
        tx
    })
}

/// Connect to `ssid` via nmcli. Returns immediately; the actual subprocess
/// runs on the dedicated `wifi-cmd` thread so the GTK main loop never blocks.
pub fn connect(ssid: &str) {
    let _ = cmd_sender().send(Cmd::Connect(ssid.to_string()));
}

/// Disconnect the currently associated wifi interface via nmcli.
pub fn disconnect() {
    let _ = cmd_sender().send(Cmd::Disconnect);
}

/// Force a `nmcli device wifi rescan` and re-publish state. Use sparingly:
/// rescans are slow (~3 s) and disrupt the connection briefly.
pub fn refresh() {
    let _ = cmd_sender().send(Cmd::Refresh);
}

// ────────── snapshot assembly ──────────

/// Read the connection-state half from sysfs/procfs/NL80211 only. Skipping
/// the nmcli scan keeps this fast enough to run on every netlink wake-up.
fn read_connection_state() -> (bool, Option<ConnectedNetwork>) {
    if is_wifi_rfkill_blocked() {
        return (false, None);
    }
    let Some(iface) = find_wifi_interface() else {
        return (true, None);
    };
    let operstate =
        std::fs::read_to_string(format!("/sys/class/net/{iface}/operstate")).unwrap_or_default();
    if operstate.trim() != "up" {
        return (true, None);
    }
    let signal = read_signal_strength(&iface);
    let ssid = nmcli_active_ssid().unwrap_or_default();
    (true, Some(ConnectedNetwork { ssid, signal }))
}

/// Build a full snapshot, including the slow nmcli scan. Use this only after
/// scan-driven events (the timer or an explicit refresh).
fn snapshot_now() -> WifiState {
    let (enabled, connected) = read_connection_state();
    let networks = if enabled { nmcli_scan_networks() } else { Vec::new() };
    WifiState {
        enabled,
        connected,
        networks,
    }
}

// ────────── epoll monitor: RTMGRP_LINK + connection-poll + scan timer ──

const TAG_NETLINK: u64 = 0;
const TAG_CONN_TIMER: u64 = 1;
const TAG_SCAN_TIMER: u64 = 2;

fn make_timerfd(secs: i64) -> Option<OwnedFd> {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        return None;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };
    let spec = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: secs,
            tv_nsec: 0,
        },
        it_value: libc::timespec {
            tv_sec: secs,
            tv_nsec: 0,
        },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };
    Some(tfd)
}

fn wifi_monitor(tx: watch::Sender<WifiState>) {
    // ── netlink RTMGRP_LINK socket: instant link up/down events ──
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

    // 5 s for connection-state / signal polling, 60 s for full nmcli scan.
    let Some(conn_tfd) = make_timerfd(5) else {
        return;
    };
    let Some(scan_tfd) = make_timerfd(60) else {
        return;
    };

    // ── epoll watching all three fds ──
    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    for (fd, tag) in [
        (nl_fd.as_raw_fd(), TAG_NETLINK),
        (conn_tfd.as_raw_fd(), TAG_CONN_TIMER),
        (scan_tfd.as_raw_fd(), TAG_SCAN_TIMER),
    ] {
        let mut ev = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: tag,
        };
        unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut ev) };
    }

    log::info!("wifi: epoll monitor started (netlink + 5s conn + 60s scan)");

    // Seed the initial state with a full scan so the popover is populated on
    // first open.
    let _ = tx.send(snapshot_now());

    let mut drain_buf = [0u8; 4096];

    loop {
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 3];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), events.as_mut_ptr(), 3, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }

        let mut do_conn = false;
        let mut do_scan = false;

        for i in 0..n as usize {
            match events[i].u64 {
                TAG_NETLINK => {
                    // Drain socket; debounce briefly to coalesce flurries.
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
                    do_conn = true;
                }
                TAG_CONN_TIMER => {
                    let mut tbuf = [0u8; 8];
                    unsafe {
                        libc::read(conn_tfd.as_raw_fd(), tbuf.as_mut_ptr().cast(), 8);
                    }
                    do_conn = true;
                }
                TAG_SCAN_TIMER => {
                    let mut tbuf = [0u8; 8];
                    unsafe {
                        libc::read(scan_tfd.as_raw_fd(), tbuf.as_mut_ptr().cast(), 8);
                    }
                    do_scan = true;
                }
                _ => {}
            }
        }

        // Build the new state. If a scan tick fired we re-list networks;
        // otherwise we keep the previous list and only refresh the
        // connection half (cheap).
        let new_state = if do_scan {
            snapshot_now()
        } else if do_conn {
            let (enabled, connected) = read_connection_state();
            let networks = tx.borrow().networks.clone();
            WifiState {
                enabled,
                connected,
                networks,
            }
        } else {
            continue;
        };

        tx.send_if_modified(|cur| {
            if !wifi_state_eq(cur, &new_state) {
                *cur = new_state;
                true
            } else {
                false
            }
        });
    }
}

/// Manual equality: avoids requiring `PartialEq` derives on the public types
/// (the spec leaves equality semantics to consumers; we want only "does the
/// rendered state actually differ" here).
fn wifi_state_eq(a: &WifiState, b: &WifiState) -> bool {
    if a.enabled != b.enabled {
        return false;
    }
    match (&a.connected, &b.connected) {
        (None, None) => {}
        (Some(x), Some(y)) if x.ssid == y.ssid && x.signal == y.signal => {}
        _ => return false,
    }
    if a.networks.len() != b.networks.len() {
        return false;
    }
    a.networks.iter().zip(b.networks.iter()).all(|(x, y)| {
        x.ssid == y.ssid && x.signal == y.signal && x.known == y.known && x.secured == y.secured
    })
}

// ────────── public API ──────────

fn sender() -> &'static watch::Sender<WifiState> {
    static S: OnceLock<watch::Sender<WifiState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(WifiState::default());
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("wifi".into())
            .spawn(move || wifi_monitor(producer))
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<WifiState> {
    sender().subscribe()
}
