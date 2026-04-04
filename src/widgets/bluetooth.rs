//! Bluetooth indicator widget.
//!
//! Reads initial power state from rfkill sysfs. Runs a single long-lived
//! `bluetoothctl` process and uses epoll on its stdout fd to parse
//! CHG events — no polling, no re-querying.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

#[derive(Clone, PartialEq)]
enum BtState {
    Off,
    On,
    Connected,
}

// ── sysfs ──────────────────────────────────────────────────────────────

fn is_bt_powered() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/rfkill") else {
        return false;
    };
    for entry in dir.filter_map(Result::ok) {
        let p = entry.path();
        let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        if ty.trim() != "bluetooth" {
            continue;
        }
        let state = std::fs::read_to_string(p.join("state")).unwrap_or_default();
        let hard = std::fs::read_to_string(p.join("hard")).unwrap_or_default();
        return state.trim() == "1" && hard.trim() != "1";
    }
    false
}

fn query_initial_connected() -> bool {
    std::process::Command::new("bluetoothctl")
        .args(["devices", "Connected"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.starts_with("Device"))
        })
        .unwrap_or(false)
}

// ── epoll monitor ──────────────────────────────────────────────────────

fn bt_monitor(tx: async_channel::Sender<BtState>, shared: Arc<Mutex<BtState>>) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        let Ok(mut child) = std::process::Command::new("bluetoothctl")
            .stdout(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped()) // keep pipe open so bluetoothctl doesn't exit on EOF
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            log::warn!("bluetooth: bluetoothctl not found, retrying in {backoff:?}");
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(max_backoff);
            continue;
        };

        let stdout = child.stdout.take().unwrap();
        let raw_fd = stdout.as_raw_fd();

        // Set stdout non-blocking for epoll
        unsafe {
            let flags = libc::fcntl(raw_fd, libc::F_GETFL);
            libc::fcntl(raw_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if epfd < 0 {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }
        let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

        let mut ev = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: 0,
        };
        if unsafe {
            libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, raw_fd, &mut ev)
        } < 0
        {
            let _ = child.kill();
            let _ = child.wait();
            continue;
        }

        log::info!("bluetooth: epoll monitor started");
        let started = std::time::Instant::now();

        let mut line_buf = Vec::new();
        let mut read_buf = [0u8; 1024];

        'outer: loop {
            let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
            let n =
                unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
            if n < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }

            // Drain readable bytes, split into lines
            loop {
                let n = unsafe {
                    libc::read(raw_fd, read_buf.as_mut_ptr().cast(), read_buf.len())
                };
                if n <= 0 {
                    if n == 0 {
                        break 'outer; // EOF — bluetoothctl exited
                    }
                    break; // EAGAIN — back to epoll
                }

                for &b in &read_buf[..n as usize] {
                    if b == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let mut changed = false;
                        let mut state = shared.lock().unwrap();

                        if line.contains("Powered: yes") && *state == BtState::Off {
                            *state = BtState::On;
                            changed = true;
                        } else if line.contains("Powered: no") && *state != BtState::Off {
                            *state = BtState::Off;
                            changed = true;
                        } else if line.contains("Connected: yes")
                            && *state != BtState::Connected
                        {
                            *state = BtState::Connected;
                            changed = true;
                        } else if line.contains("Connected: no")
                            && *state == BtState::Connected
                        {
                            *state = BtState::On;
                            changed = true;
                        }

                        if changed {
                            let new = state.clone();
                            drop(state);
                            let _ = tx.try_send(new);
                        }

                        line_buf.clear();
                    } else if b != b'\r' {
                        line_buf.push(b);
                    }
                }
            }
        }

        drop(stdout);
        let _ = child.kill();
        let _ = child.wait();
        // Only reset backoff if the process ran for at least 5 seconds
        if started.elapsed() > Duration::from_secs(5) {
            backoff = Duration::from_secs(1);
        }
        log::debug!("bluetooth: bluetoothctl exited after {:?}, retrying in {backoff:?}", started.elapsed());
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    }
}

// ── widget ─────────────────────────────────────────────────────────────

const BT_OFF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/bluetooth-off.svg");
const BT_ON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/bluetooth-on.svg");
const BT_CONNECTED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/bluetooth-connected.svg");

fn bt_icon(state: &BtState) -> &'static str {
    match state {
        BtState::Off => BT_OFF,
        BtState::On => BT_ON,
        BtState::Connected => BT_CONNECTED,
    }
}

pub struct Bluetooth {
    state: BtState,
    grouped: bool,
}

impl BarWidget for Bluetooth {
    const NAME: &str = "bluetooth";

    fn new(cx: &mut Context<Self>) -> Self {
        let powered = is_bt_powered();
        let connected = powered && query_initial_connected();
        let initial = if !powered {
            BtState::Off
        } else if connected {
            BtState::Connected
        } else {
            BtState::On
        };

        let (tx, rx) = async_channel::bounded::<BtState>(4);
        let shared = Arc::new(Mutex::new(initial.clone()));

        std::thread::Builder::new()
            .name("bt-epoll".into())
            .spawn({
                let shared = shared.clone();
                move || bt_monitor(tx, shared)
            })
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

        Self { state: initial, grouped: false }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;

        let icon_path = bt_icon(&self.state);
        let color = match self.state {
            BtState::Off => t.fg_dark,
            BtState::On => t.fg,
            BtState::Connected => t.blue,
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .justify_center()
                .px(px(6.0))
                .child(
                    svg()
                        .external_path(icon_path.to_string())
                        .size(px(crate::config::ICON_SIZE))
                        .text_color(rgb(color))
                        .flex_shrink_0(),
                ),
            self.grouped,
        )
    }
}

impl_render!(Bluetooth);
