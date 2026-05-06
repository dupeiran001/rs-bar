//! Shared system-data hub. Each submodule owns a singleton OS thread that
//! reads from sysfs/procfs/IPC and publishes via a `tokio::sync::watch::Sender`.
//! Widgets subscribe with `<module>::subscribe()`.
//!
//! First call to `subscribe()` lazily spawns the OS thread. Multi-monitor
//! setups still get exactly one poller per data source — `tx.subscribe()`
//! returns a fresh receiver cheaply.

pub mod sys;

pub mod cpu_freq;
pub mod cpu_temp;
pub mod cpu_usage;
pub mod gpu_busy;
pub mod memory;
pub mod niri;
pub mod power_draw;
pub mod tray;

// Per-widget hub stubs (pre-added in Phase 0 so Phase 3 widgets do not have
// to share an edit point on this file). Each is replaced by its owning
// Phase 3 task.
pub mod battery;
pub mod bluetooth;
pub mod brightness;
pub mod capslock;
pub mod fcitx;
pub mod pkg_update;
pub mod volume;
pub mod wifi;
pub mod wireguard;
