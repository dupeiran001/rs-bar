//! Shared low-level helpers used by hub poller threads:
//! - sysfs file readers (u64 / i64 / String)
//! - timerfd + epoll loop for periodic polling
//!
//! No dependency on hub channels; pure libc + std::fs.

#![allow(dead_code)]

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

pub fn sysfs_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn sysfs_i64(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn sysfs_str(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn sysfs_readable(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok()
}

/// Run `tick` at every `interval` on a timerfd + epoll loop.
/// Returns when `tick` returns `false`.
pub fn timerfd_loop(interval: Duration, fire_immediately: bool, mut tick: impl FnMut() -> bool) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let int_secs = interval.as_secs() as i64;
    let int_nsecs = interval.subsec_nanos() as i64;
    let spec = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: int_secs,
            tv_nsec: int_nsecs,
        },
        it_value: libc::timespec {
            tv_sec: if fire_immediately { 0 } else { int_secs },
            tv_nsec: if fire_immediately {
                1
            } else {
                int_nsecs.max(1)
            },
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
        );
    }

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

        if !tick() {
            break;
        }
    }
}
