//! GPU busy-percent hub.
//!
//! Reads the GPU's busy percentage from sysfs on a 1-second timerfd. Detects
//! the active GPU by walking `/sys/class/drm/card*/device/` and matching PCI
//! class `0x03xxxx` (Display Controller) plus the vendor ID:
//!
//! - AMD     (`0x1002`): reads `gpu_busy_percent`.
//! - Intel   (`0x8086`): reads `gpu_busy_percent` or `gt_busy_percent` if
//!   present (i915 driver). Falls back to the `xe` driver's
//!   `tile0/gtidle/idle_residency_ms`, computing busy% from idle-residency
//!   deltas over the 1-second tick.
//! - NVIDIA: not exposed via sysfs in a stable way; not implemented here.
//!
//! Singleton background thread (`"gpu-busy"`) shared across every bar
//! instance. Subscribers receive the latest sample via `tokio::sync::watch`.
//! The published sample also carries a `GpuVendor` so widgets can pick a
//! vendor-appropriate icon (amd-radeon / intel-arc-gpu / nvidia-gpu / generic).

use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

/// Detected GPU vendor for icon selection.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuVendor {
    Amd,
    Intel,
    Nvidia,
    #[default]
    Unknown,
}

/// One GPU-busy sample.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuBusySample {
    /// Busy percentage in `0..=100`. `None` until the first successful read,
    /// or when the underlying source returns an unreadable value.
    pub busy_pct: Option<u32>,
    /// Detected GPU vendor (constant for the program's lifetime).
    pub vendor: GpuVendor,
}

/// How the busy percentage is computed for the detected GPU.
enum GpuBusySource {
    /// Direct percentage file (e.g. AMD `gpu_busy_percent`, older Intel
    /// `gt_busy_percent`).
    Direct { path: PathBuf },
    /// xe driver: compute busy% from `idle_residency_ms` deltas.
    Residency { path: PathBuf },
}

/// Walk `/sys/class/drm/*/device/` and pick the first GPU we know how to read.
fn detect_gpu() -> Option<(GpuBusySource, GpuVendor)> {
    let drm = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in drm.filter_map(Result::ok) {
        let dev = entry.path().join("device");
        if !dev.is_dir() {
            continue;
        }
        let class = std::fs::read_to_string(dev.join("class")).unwrap_or_default();
        if !class.trim().starts_with("0x03") {
            continue;
        }
        let vendor_id = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
        let vendor_id = vendor_id.trim();

        let (vendor, files): (GpuVendor, &[&str]) = match vendor_id {
            "0x1002" => (GpuVendor::Amd, &["gpu_busy_percent"]),
            "0x8086" => (GpuVendor::Intel, &["gpu_busy_percent", "gt_busy_percent"]),
            "0x10de" => (GpuVendor::Nvidia, &[]),
            _ => continue,
        };

        for f in files {
            let path = dev.join(f);
            if path.exists() && std::fs::read_to_string(&path).is_ok() {
                return Some((GpuBusySource::Direct { path }, vendor));
            }
        }

        // xe driver (Intel Battlemage / Arc): use gtidle residency.
        if vendor == GpuVendor::Intel
            && let Ok(tiles) = std::fs::read_dir(dev.join("tile0"))
        {
            for tile in tiles.filter_map(Result::ok) {
                let residency = tile.path().join("gtidle/idle_residency_ms");
                if residency.exists() && std::fs::read_to_string(&residency).is_ok() {
                    return Some((GpuBusySource::Residency { path: residency }, vendor));
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

fn sender() -> &'static watch::Sender<GpuBusySample> {
    static S: OnceLock<watch::Sender<GpuBusySample>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(GpuBusySample::default());

        let Some((src, vendor)) = detect_gpu() else {
            log::info!("gpu-busy: no readable GPU found in /sys/class/drm");
            return tx;
        };

        let src_path = match &src {
            GpuBusySource::Direct { path } | GpuBusySource::Residency { path } => {
                path.display().to_string()
            }
        };
        log::info!("gpu-busy: {src_path} (vendor: {vendor:?})");

        let producer = tx.clone();
        let _ = std::thread::Builder::new()
            .name("gpu-busy".into())
            .spawn(move || {
                let mut prev_residency: Option<u64> = match &src {
                    GpuBusySource::Residency { path } => read_residency_ms(path),
                    _ => None,
                };
                timerfd_loop(std::time::Duration::from_millis(500), false, || {
                    let busy_pct = match &src {
                        GpuBusySource::Direct { path } => read_busy_direct(path),
                        GpuBusySource::Residency { path } => {
                            let cur = read_residency_ms(path);
                            let result = match (prev_residency, cur) {
                                (Some(prev), Some(cur)) if cur >= prev => {
                                    let idle_ms = cur - prev;
                                    // interval is 1000 ms; clamp to 0..=100.
                                    Some(
                                        100u32.saturating_sub(
                                            idle_ms.min(1000) as u32 * 100 / 1000,
                                        ),
                                    )
                                }
                                _ => None,
                            };
                            prev_residency = cur;
                            result
                        }
                    };
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(GpuBusySample { busy_pct, vendor }).is_ok()
                });
            });
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<GpuBusySample> {
    sender().subscribe()
}
