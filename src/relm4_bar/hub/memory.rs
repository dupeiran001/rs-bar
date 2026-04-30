//! Memory usage hub. /proc/meminfo reader on a 2-second timerfd, publishes %.
//!
//! Computes used percentage as `(MemTotal - MemAvailable) / MemTotal * 100`.
//! Singleton background thread (`"memory"`) shared across every bar instance.
//! Subscribers receive the latest sample via `tokio::sync::watch`.

use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

fn read_mem_usage() -> f32 {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total: u64 = 0;
    let mut available: u64 = 0;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
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

fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("memory".into())
            .spawn(move || {
                timerfd_loop(2, true, || {
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(read_mem_usage()).is_ok()
                });
            })
            .ok();
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<f32> {
    sender().subscribe()
}
