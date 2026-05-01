//! Brightness hub. Polls a configured shell command every 2s and publishes the
//! current backlight level as a `u32` percentage.
//!
//! Singleton background thread (`"brightness"`) shared across every bar
//! instance. Subscribers receive the latest sample via `tokio::sync::watch`.
//! Adjustments shell out to `config::BRIGHTNESS_UP_CMD()` /
//! `BRIGHTNESS_DOWN_CMD()` from foreground code; the next poll picks up the
//! resulting value automatically. A `bump()` helper schedules an immediate
//! re-poll so click/scroll feedback feels snappy rather than waiting up to 2s.

use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;

use crate::relm4_bar::config;

use super::sys::timerfd_loop;

/// Poll interval. Brightness queries shell out (e.g. brightnessctl), so
/// keep the cadence at 2s to avoid burning a process exec twice a second.
const INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Set on each successful adjustment so the producer thread re-reads on the
/// next epoll wake even if the timer hasn't fired yet. The producer also reads
/// it after each timer tick — i.e. it just acts as a "skip the displayed-value
/// coalescing on the next tick" hint, since the watch channel itself does the
/// coalescing.
static DIRTY: AtomicBool = AtomicBool::new(false);

/// Run a shell command via `sh -c`. Used for the configured `brightnessctl`-
/// style commands which may contain pipes (`brightnessctl -m | cut -d, -f4`).
fn run_shell(cmd: &str) -> Option<std::process::Output> {
    Command::new("sh").args(["-c", cmd]).output().ok()
}

/// Read the current brightness percent by running `BRIGHTNESS_GET_CMD()` and
/// parsing the integer it prints. Returns 0 if the command fails or the
/// output is unparseable — same behaviour as rs-bar's GPUI version.
fn read_brightness() -> u32 {
    let cmd = config::BRIGHTNESS_GET_CMD();
    let Some(out) = run_shell(cmd) else {
        return 0;
    };
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim()
        .parse::<u32>()
        .ok()
        .map(|p| p.min(100))
        .unwrap_or(0)
}

fn sender() -> &'static watch::Sender<u32> {
    static S: OnceLock<watch::Sender<u32>> = OnceLock::new();
    S.get_or_init(|| {
        let initial = read_brightness();
        let (tx, _rx) = watch::channel(initial);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("brightness".into())
            .spawn(move || {
                timerfd_loop(INTERVAL, false, || {
                    // Clear the dirty flag — we're about to re-read anyway.
                    DIRTY.store(false, Ordering::Relaxed);
                    let pct = read_brightness();
                    producer.send(pct).is_ok()
                });
            })
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<u32> {
    sender().subscribe()
}

// ── command API ────────────────────────────────────────────────────────

/// Increase brightness by one step via `config::BRIGHTNESS_UP_CMD()`. Runs the
/// command on a detached thread so the GTK main loop never blocks on the
/// subprocess. After the command completes, an immediate re-poll is forced so
/// the displayed value updates without waiting for the next 2s tick.
pub fn brightness_up() {
    let cmd = config::BRIGHTNESS_UP_CMD();
    spawn_command(cmd);
}

/// Decrease brightness by one step via `config::BRIGHTNESS_DOWN_CMD()`. Same
/// non-blocking shape as `brightness_up`.
pub fn brightness_down() {
    let cmd = config::BRIGHTNESS_DOWN_CMD();
    spawn_command(cmd);
}

fn spawn_command(cmd: &'static str) {
    std::thread::Builder::new()
        .name("brightness-cmd".into())
        .spawn(move || {
            let _ = run_shell(cmd);
            // Force a re-read so the watch channel gets the new value
            // promptly, instead of waiting for the next 2s tick.
            let pct = read_brightness();
            let _ = sender().send(pct);
            DIRTY.store(true, Ordering::Relaxed);
        })
        .ok();
}
