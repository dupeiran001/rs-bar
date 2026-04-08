//! CapsLock indicator widget.
//!
//! Monitors CapsLock state via Linux evdev — a background thread uses
//! epoll on `/dev/input/eventN` and filters for `EV_LED LED_CAPSL` events,
//! which carry the absolute on/off state directly from the kernel.
//! Zero polling; the thread sleeps in `epoll_wait` between toggles.
//!
//! Requires the user to be in the `input` group (`/dev/input/event*` is `root:input`).

use std::ffi::CString;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

// ── linux evdev constants ──────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
struct InputEvent {
    _tv_sec: i64,
    _tv_usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

const INPUT_EVENT_SIZE: usize = mem::size_of::<InputEvent>();
const EV_LED: u16 = 0x11;
const LED_CAPSL: u16 = 0x01;

// ── ioctl helpers ──────────────────────────────────────────────────────

/// Linux `_IOC(dir, type, nr, size)`.
const fn ioc(dir: u64, ty: u64, nr: u64, size: u64) -> libc::c_ulong {
    (dir << 30 | ty << 8 | nr | size << 16) as libc::c_ulong
}

const IOC_READ: u64 = 2;
const EVTYPE: u64 = b'E' as u64;

/// `EVIOCGBIT(ev, len)` — capability bits for event type `ev`.
const fn eviocgbit(ev: u16, len: usize) -> libc::c_ulong {
    ioc(IOC_READ, EVTYPE, 0x20 + ev as u64, len as u64)
}

/// `EVIOCGLED(len)` — current LED state bits.
const fn eviocgled(len: usize) -> libc::c_ulong {
    ioc(IOC_READ, EVTYPE, 0x19, len as u64)
}

// ── device helpers ─────────────────────────────────────────────────────

fn open_readonly(path: &PathBuf) -> std::io::Result<OwnedFd> {
    let c = CString::new(path.as_os_str().as_encoded_bytes().to_vec())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let fd = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// Read current CapsLock LED state via ioctl.
fn read_led_state(fd: i32) -> bool {
    let mut bits = [0u8; 1];
    let ret = unsafe { libc::ioctl(fd, eviocgled(bits.len()), bits.as_mut_ptr()) };
    ret >= 0 && bits[0] & (1 << LED_CAPSL) != 0
}

/// Scan `/dev/input/event*` for the first device with `LED_CAPSL` capability.
fn find_capslock_device() -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir("/dev/input")
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("event"))
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let fd = match open_readonly(&path) {
            Ok(fd) => fd,
            Err(_) => continue,
        };
        let raw = fd.as_raw_fd();

        // Does the device emit EV_LED?
        let mut ev_bits = [0u8; 4];
        if unsafe { libc::ioctl(raw, eviocgbit(0, ev_bits.len()), ev_bits.as_mut_ptr()) } < 0 {
            continue;
        }
        if ev_bits[(EV_LED / 8) as usize] & (1 << (EV_LED % 8)) == 0 {
            continue;
        }

        // Does it have LED_CAPSL?
        let mut led_bits = [0u8; 1];
        if unsafe {
            libc::ioctl(raw, eviocgbit(EV_LED, led_bits.len()), led_bits.as_mut_ptr())
        } < 0
        {
            continue;
        }
        if led_bits[0] & (1 << LED_CAPSL) == 0 {
            continue;
        }

        return Some(path);
    }
    None
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct CapsLock {
    on: bool,
}

impl BarWidget for CapsLock {
    const NAME: &str = "capslock";

    fn new(cx: &mut Context<Self>) -> Self {
        let device_path = find_capslock_device();

        let initial = device_path.as_ref().map_or(false, |p| {
            open_readonly(p).map_or(false, |fd| read_led_state(fd.as_raw_fd()))
        });

        if let Some(path) = device_path {
            let (tx, rx) = async_channel::unbounded::<bool>();

            std::thread::Builder::new()
                .name("capslock-epoll".into())
                .spawn(move || {
                    let fd = match open_readonly(&path) {
                        Ok(fd) => fd,
                        Err(e) => {
                            log::warn!("capslock: open {}: {e}", path.display());
                            return;
                        }
                    };
                    let raw = fd.as_raw_fd();

                    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
                    if epfd < 0 {
                        log::warn!("capslock: epoll_create1: {}", std::io::Error::last_os_error());
                        return;
                    }
                    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

                    let mut ev = libc::epoll_event {
                        events: libc::EPOLLIN as u32,
                        u64: 0,
                    };
                    if unsafe {
                        libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, raw, &mut ev)
                    } < 0
                    {
                        log::warn!("capslock: epoll_ctl: {}", std::io::Error::last_os_error());
                        return;
                    }

                    log::info!("capslock: monitoring {}", path.display());

                    loop {
                        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
                        let n = unsafe {
                            libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1)
                        };
                        if n < 0 {
                            if std::io::Error::last_os_error().kind()
                                == std::io::ErrorKind::Interrupted
                            {
                                continue;
                            }
                            break;
                        }

                        // Drain all pending events, keep last LED_CAPSL state.
                        let mut new_state: Option<bool> = None;
                        loop {
                            let mut buf = [0u8; INPUT_EVENT_SIZE];
                            let n = unsafe {
                                libc::read(raw, buf.as_mut_ptr().cast(), INPUT_EVENT_SIZE)
                            };
                            if n != INPUT_EVENT_SIZE as isize {
                                break;
                            }
                            let ev: InputEvent =
                                unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
                            if ev.type_ == EV_LED && ev.code == LED_CAPSL {
                                new_state = Some(ev.value != 0);
                            }
                        }

                        if let Some(state) = new_state {
                            if tx.send_blocking(state).is_err() {
                                break;
                            }
                        }
                    }
                })
                .ok();

            cx.spawn(async move |this, cx| {
                while let Ok(caps_on) = rx.recv().await {
                    if this
                        .update(cx, |this, cx| {
                            if this.on != caps_on {
                                this.on = caps_on;
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
        } else {
            log::warn!("capslock: no device with LED_CAPSL found (are you in the `input` group?)");
        }

        Self { on: initial }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let content_h = crate::config::CONTENT_HEIGHT();

        if self.on {
            div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(content_h))
                .px(px(6.0))
                .bg(rgb(t.border))
                .child(
                    svg()
                        .external_path(
                            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/capslock.svg")
                                .to_string(),
                        )
                        .size(px(crate::config::ICON_SIZE()))
                        .text_color(rgb(t.yellow)),
                )
        } else {
            div()
        }
    }
}

impl_render!(CapsLock);
