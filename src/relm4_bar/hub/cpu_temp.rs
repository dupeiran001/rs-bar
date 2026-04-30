//! CPU temperature hub. Auto-detects the CPU package/die temperature source
//! and publishes degrees Celsius via `tokio::sync::watch`.
//!
//! Source detection (in order):
//!   - Intel: `/sys/class/hwmon/*` where `name == "coretemp"`, label
//!     `"Package id 0"` (falls back to `temp1_input`).
//!   - AMD:   `/sys/class/hwmon/*` where `name == "k10temp"`, label `"Tctl"`
//!     or `"Tdie"` (falls back to `temp1_input`).
//!   - Apple Silicon (Asahi): `/sys/class/hwmon/*` where `name == "macsmc"`,
//!     label `"Heatpipe Temp"` or `"Charge Regulator Temp"` (falls back to
//!     `temp1_input`).
//!   - Fallback: `/sys/class/thermal/thermal_zone*/temp` where the zone's
//!     `type` is `x86_pkg_temp`, then any thermal zone.
//!
//! Polled every 2 seconds via `super::sys::timerfd_loop`. All sysfs, zero
//! subprocesses. Singleton background thread (`"cpu-temp"`) shared across
//! every bar instance.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::{sysfs_i64, timerfd_loop};

/// Where the package/die temperature lives. Both variants point at a
/// millidegree-Celsius file.
enum TempSource {
    Hwmon(PathBuf),
    ThermalZone(PathBuf),
}

fn detect_temp_source() -> Option<TempSource> {
    // 1. Scan hwmon for known CPU temperature sensors.
    if let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") {
        for entry in entries.filter_map(Result::ok) {
            let hw = entry.path();
            let name = std::fs::read_to_string(hw.join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();

            match name.as_str() {
                // Intel coretemp — prefer "Package id 0".
                "coretemp" => {
                    if let Some(path) = find_label_temp(&hw, "Package id 0") {
                        return Some(TempSource::Hwmon(path));
                    }
                    let t1 = hw.join("temp1_input");
                    if t1.exists() {
                        return Some(TempSource::Hwmon(t1));
                    }
                }
                // AMD k10temp — prefer "Tctl", then "Tdie".
                "k10temp" => {
                    if let Some(path) = find_label_temp(&hw, "Tctl") {
                        return Some(TempSource::Hwmon(path));
                    }
                    if let Some(path) = find_label_temp(&hw, "Tdie") {
                        return Some(TempSource::Hwmon(path));
                    }
                    let t1 = hw.join("temp1_input");
                    if t1.exists() {
                        return Some(TempSource::Hwmon(t1));
                    }
                }
                // Apple Silicon macsmc — prefer SoC/heatpipe.
                "macsmc" => {
                    if let Some(path) = find_label_temp(&hw, "Heatpipe Temp") {
                        return Some(TempSource::Hwmon(path));
                    }
                    if let Some(path) = find_label_temp(&hw, "Charge Regulator Temp") {
                        return Some(TempSource::Hwmon(path));
                    }
                    let t1 = hw.join("temp1_input");
                    if t1.exists() {
                        return Some(TempSource::Hwmon(t1));
                    }
                }
                _ => {}
            }
        }
    }

    // 2. Fallback: thermal zones. Prefer x86_pkg_temp, otherwise any zone.
    if let Ok(entries) = std::fs::read_dir("/sys/class/thermal") {
        let mut zones: Vec<_> = entries.filter_map(Result::ok).collect();
        zones.sort_by_key(|e| e.file_name());

        for entry in &zones {
            let p = entry.path();
            let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            if ty.trim() == "x86_pkg_temp" {
                let temp = p.join("temp");
                if temp.exists() {
                    return Some(TempSource::ThermalZone(temp));
                }
            }
        }
        for entry in &zones {
            let p = entry.path();
            let temp = p.join("temp");
            if temp.exists() {
                return Some(TempSource::ThermalZone(temp));
            }
        }
    }

    None
}

fn find_label_temp(hwmon: &Path, target_label: &str) -> Option<PathBuf> {
    for i in 1..=20 {
        let label_path = hwmon.join(format!("temp{i}_label"));
        let input_path = hwmon.join(format!("temp{i}_input"));
        if let Ok(label) = std::fs::read_to_string(&label_path) {
            if label.trim() == target_label && input_path.exists() {
                return Some(input_path);
            }
        }
    }
    None
}

fn read_temp(source: &TempSource) -> Option<f32> {
    let path = match source {
        TempSource::Hwmon(p) | TempSource::ThermalZone(p) => p,
    };
    let millideg = sysfs_i64(path)?;
    Some(millideg as f32 / 1000.0)
}

fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("cpu-temp".into())
            .spawn(move || {
                let source = match detect_temp_source() {
                    Some(s) => s,
                    None => {
                        log::warn!("cpu_temp: no temperature source found");
                        return;
                    }
                };

                match &source {
                    TempSource::Hwmon(p) => {
                        log::info!("cpu_temp: using hwmon {}", p.display());
                    }
                    TempSource::ThermalZone(p) => {
                        log::info!("cpu_temp: using thermal {}", p.display());
                    }
                }

                // Publish an initial reading immediately so subscribers
                // don't wait 2s for the first sample.
                if let Some(t) = read_temp(&source) {
                    let _ = producer.send(t);
                }

                timerfd_loop(2, false, || {
                    if let Some(t) = read_temp(&source) {
                        // Returning false would exit the loop; in practice
                        // the sender is held by the OnceLock for the
                        // program's lifetime so this never happens.
                        producer.send(t).is_ok()
                    } else {
                        true
                    }
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
