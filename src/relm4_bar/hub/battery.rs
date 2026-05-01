//! Battery hub. `/sys/class/power_supply/BAT*/` reader on a 5-second timerfd,
//! publishes a `BatteryState` with capacity, status, time-remaining estimate,
//! cycle count, and health percentage.
//!
//! Source detection: scan known names (`BAT0`, `BAT1`, `macsmc-battery`) then
//! any directory whose `type` is `Battery` and which exposes the minimum set
//! of files needed for energy/charge math. When no battery is present the
//! published `BatteryState` carries `present: false` and the widget hides.
//!
//! Time-remaining: estimated locally from `energy_now` / `power_now` (or the
//! `current_now` × `voltage_now` fallback) since stock kernels don't expose
//! `time_to_empty_now` or `time_to_full_now` for non-mac batteries.
//! Health: `energy_full / energy_full_design * 100` (or `charge_full /
//! charge_full_design`). Both are computed only when the underlying counters
//! exist.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::{sysfs_i64, sysfs_str, sysfs_u64, timerfd_loop};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatteryStatus {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Full,
    NotCharging,
}

impl BatteryStatus {
    fn from_str(s: &str) -> Self {
        match s {
            "Charging" => BatteryStatus::Charging,
            "Discharging" => BatteryStatus::Discharging,
            "Full" => BatteryStatus::Full,
            "Not charging" => BatteryStatus::NotCharging,
            _ => BatteryStatus::Unknown,
        }
    }
}

/// Latest observed battery state. `present == false` carries through every
/// other field as default; subscribers should branch on `present` first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryState {
    pub present: bool,
    /// 0..=100 (rounded).
    pub capacity_pct: u32,
    pub status: BatteryStatus,
    /// Local estimate from energy/power. `None` when the battery is full,
    /// at zero current, or when the underlying counters are missing.
    pub time_remaining_minutes: Option<u32>,
    pub cycles: Option<u32>,
    /// `energy_full / energy_full_design * 100`, rounded. `None` when the
    /// design counter is absent or zero.
    pub health_pct: Option<u32>,
}

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

struct BatteryInfo {
    dir: PathBuf,
}

/// A directory under `/sys/class/power_supply` qualifies as a system battery
/// if it exposes either `power_now` or the (`current_now`, `voltage_now`)
/// pair. Mirrors the rs-bar gpui_bar `power_draw` detection.
fn is_system_battery(dir: &Path) -> bool {
    dir.join("power_now").exists()
        || (dir.join("current_now").exists() && dir.join("voltage_now").exists())
        // Some laptops only expose charge_now (µAh); accept those too so the
        // capacity/status fields still publish even if power can't be derived.
        || dir.join("charge_now").exists()
        || dir.join("energy_now").exists()
}

fn detect_battery() -> Option<BatteryInfo> {
    for name in ["BAT0", "BAT1", "macsmc-battery"] {
        let dir = Path::new("/sys/class/power_supply").join(name);
        if dir.exists() && is_system_battery(&dir) {
            return Some(BatteryInfo { dir });
        }
    }
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.filter_map(Result::ok) {
        let dir = entry.path();
        if sysfs_str(&dir.join("type")) == "Battery" && is_system_battery(&dir) {
            return Some(BatteryInfo { dir });
        }
    }
    None
}

/// Read µW from `power_now`, falling back to `current_now * voltage_now` (in
/// µA × µV → pW → scaled to µW). Returns `None` if neither is available or
/// the value is zero (so callers can decide whether to estimate time).
fn read_power_uw(bat: &BatteryInfo) -> Option<u64> {
    if let Some(w) = sysfs_i64(&bat.dir.join("power_now")) {
        let uw = w.unsigned_abs();
        if uw > 0 {
            return Some(uw);
        }
    }
    if let (Some(ua), Some(uv)) = (
        sysfs_i64(&bat.dir.join("current_now")),
        sysfs_i64(&bat.dir.join("voltage_now")),
    ) {
        // µA × µV / 1e6 → µW.
        let uw = (ua.unsigned_abs() as u128 * uv.unsigned_abs() as u128) / 1_000_000;
        if uw > 0 {
            return Some(uw as u64);
        }
    }
    None
}

/// Energy_now / charge_now in µWh / µAh, with a sibling reading for the matching
/// "full" counter. Returns (now, full, design) — the last two are `Option`s.
struct EnergyTriple {
    now: u64,
    full: Option<u64>,
    full_design: Option<u64>,
}

fn read_energy(bat: &BatteryInfo) -> Option<EnergyTriple> {
    if let Some(now) = sysfs_u64(&bat.dir.join("energy_now")) {
        return Some(EnergyTriple {
            now,
            full: sysfs_u64(&bat.dir.join("energy_full")),
            full_design: sysfs_u64(&bat.dir.join("energy_full_design")),
        });
    }
    if let Some(now) = sysfs_u64(&bat.dir.join("charge_now")) {
        return Some(EnergyTriple {
            now,
            full: sysfs_u64(&bat.dir.join("charge_full")),
            full_design: sysfs_u64(&bat.dir.join("charge_full_design")),
        });
    }
    None
}

/// Estimate time-to-empty (when discharging) or time-to-full (when charging)
/// in minutes. Returns `None` when the underlying counters can't form a
/// consistent unit pair, when the rate is zero, or when the resulting estimate
/// is implausible (negative, infinite, or > 100h).
///
/// Same-domain math only: if the directory exposes `energy_now` (µWh) we pair
/// it with `power_now` (µW) — both energy. If it exposes `charge_now` (µAh)
/// instead, we pair it with `current_now` (µA) — both charge. Mixing those is
/// nonsense and would produce a meaningless number, so we simply refuse.
fn estimate_time_remaining(
    bat: &BatteryInfo,
    status: BatteryStatus,
    energy: &EnergyTriple,
) -> Option<u32> {
    let energy_path_exists = bat.dir.join("energy_now").exists();
    let charge_path_exists = bat.dir.join("charge_now").exists();

    let rate: u64 = if energy_path_exists {
        read_power_uw(bat)?
    } else if charge_path_exists {
        let i = sysfs_i64(&bat.dir.join("current_now"))?;
        let v = i.unsigned_abs();
        if v == 0 { return None; }
        v
    } else {
        return None;
    };

    let remaining_capacity_u = match status {
        BatteryStatus::Charging => energy.full.unwrap_or(energy.now).saturating_sub(energy.now),
        _ => energy.now,
    };

    // Minutes = (capacity / rate) × 60.
    let minutes = (remaining_capacity_u as f64 / rate as f64 * 60.0).round();
    if minutes.is_finite() && minutes > 0.0 && minutes < 6000.0 {
        Some(minutes as u32)
    } else {
        None
    }
}

fn read_state(bat: &BatteryInfo) -> BatteryState {
    let capacity_pct = sysfs_u64(&bat.dir.join("capacity"))
        .map(|c| c.min(100) as u32)
        .unwrap_or(0);
    let status = BatteryStatus::from_str(&sysfs_str(&bat.dir.join("status")));
    let cycles = sysfs_u64(&bat.dir.join("cycle_count")).map(|c| c as u32);

    let energy = read_energy(bat);

    let health_pct = energy.as_ref().and_then(|e| {
        let full = e.full?;
        let design = e.full_design?;
        if design == 0 {
            None
        } else {
            Some(((full as f64 / design as f64) * 100.0).round() as u32)
        }
    });

    // Time remaining: only meaningful while charging or discharging.
    let time_remaining_minutes = match status {
        BatteryStatus::Charging | BatteryStatus::Discharging => energy
            .as_ref()
            .and_then(|e| estimate_time_remaining(bat, status, e)),
        _ => None,
    };

    BatteryState {
        present: true,
        capacity_pct,
        status,
        time_remaining_minutes,
        cycles,
        health_pct,
    }
}

fn sender() -> &'static watch::Sender<BatteryState> {
    static S: OnceLock<watch::Sender<BatteryState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(BatteryState::default());
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("battery".into())
            .spawn(move || {
                let bat = match detect_battery() {
                    Some(b) => {
                        log::info!("battery: {}", b.dir.display());
                        b
                    }
                    None => {
                        log::info!("battery: no battery found");
                        // Publish the default (present=false) once so
                        // subscribers can render the hidden state.
                        let _ = producer.send(BatteryState::default());
                        return;
                    }
                };

                // Publish an initial reading immediately so subscribers don't
                // wait `POLL_SECS` for the first sample.
                let _ = producer.send(read_state(&bat));

                timerfd_loop(POLL_INTERVAL, false, || {
                    let s = read_state(&bat);
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(s).is_ok()
                });
            })
            .ok();
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<BatteryState> {
    sender().subscribe()
}
