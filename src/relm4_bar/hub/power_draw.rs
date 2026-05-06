//! Power-draw hub — battery / CPU package / platform / discrete-GPU watts.
//!
//! Auto-detects four independent power sources and publishes a coalesced
//! `PowerDrawSample` (each field is `Option<f64>` watts) every 2 seconds.
//!
//! Sources detected at first-subscriber time:
//!
//! - **Battery** (`battery_w`): scans `/sys/class/power_supply/{BAT0,BAT1,
//!   macsmc-battery,*}` for the first entry exposing `power_now`, or
//!   `current_now` × `voltage_now`. Sign of the discharge/charge current is
//!   discarded — the published value is unsigned watts.
//! - **CPU package** (`cpu_w`): `/sys/class/powercap/intel-rapl*` package
//!   domains (sum of all packages) on Intel/AMD, or the macsmc_hwmon
//!   `Heatpipe Power` sensor on Apple Silicon. RAPL energy counters are
//!   converted to watts via finite differences across each tick (with
//!   `max_energy_range_uj` wraparound handling).
//! - **Platform** (`psys_w`): `/sys/class/powercap/intel-rapl*` `psys` /
//!   `platform` domains, or the macsmc_hwmon `Total System Power` sensor.
//!   Same energy-counter delta technique as CPU package.
//! - **Discrete GPU** (`gpu_w`): scans `/sys/bus/pci/devices` for class
//!   `0x03*` (display controllers) and uses the first hwmon `power*_input` /
//!   `power*_average` (instant) or `energy*_input` (delta). Falls back to
//!   `nvidia-smi --query-gpu=power.draw` (blocking subprocess on the poller
//!   thread) when no NVIDIA hwmon node is exposed but `/proc/driver/nvidia`
//!   is present.
//!
//! Implementation note: rs-bar (the GPUI ancestor) ran four separate
//! polling threads, one per measurement. The Relm4 hub coalesces them into
//! a single `"power-draw"` thread that polls all four every tick — one
//! file-system pass instead of four, one watch publish instead of four.
//! Subscribers pick whichever field(s) they need.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use tokio::sync::watch;

use super::sys::{sysfs_i64, sysfs_readable, sysfs_str, sysfs_u64, timerfd_loop};

/// Coalesced power-draw sample. Each field is `None` until its source has
/// been detected and produced its first reading; absent hardware stays
/// `None` for the lifetime of the program.
#[derive(Clone, Copy, Default)]
pub struct PowerDrawSample {
    pub battery_w: Option<f64>,
    pub cpu_w: Option<f64>,
    pub psys_w: Option<f64>,
    pub gpu_w: Option<f64>,
}

/// 2s poll. Kept at this cadence because nvidia-smi shellout (the GPU
/// fallback path) costs 20–80ms per call.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

// ── battery detection / read ──────────────────────────────────────────

struct BatteryInfo {
    dir: PathBuf,
}

fn is_system_battery(dir: &Path) -> bool {
    dir.join("power_now").exists()
        || (dir.join("current_now").exists() && dir.join("voltage_now").exists())
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

fn read_battery_watts(bat: &BatteryInfo) -> f64 {
    if let Some(w) = sysfs_i64(&bat.dir.join("power_now")) {
        return w.unsigned_abs() as f64 / 1e6;
    }
    if let (Some(ua), Some(uv)) = (
        sysfs_i64(&bat.dir.join("current_now")),
        sysfs_i64(&bat.dir.join("voltage_now")),
    ) {
        return (ua.unsigned_abs() as f64 * uv.unsigned_abs() as f64) / 1e12;
    }
    0.0
}

// ── RAPL detection ────────────────────────────────────────────────────

struct RaplDomain {
    energy_path: PathBuf,
    max_uj: u64,
}

fn detect_rapl() -> (Vec<RaplDomain>, Vec<RaplDomain>) {
    let mut pkg = Vec::new();
    let mut psys = Vec::new();

    let base = Path::new("/sys/class/powercap");
    let Ok(entries) = std::fs::read_dir(base) else {
        return (pkg, psys);
    };

    let push = |path: &Path, pkg: &mut Vec<RaplDomain>, psys: &mut Vec<RaplDomain>| {
        let ep = path.join("energy_uj");
        if !ep.exists() {
            return;
        }
        if !sysfs_readable(&ep) {
            log::warn!(
                "power_draw: {} unreadable (install udev rule or chmod a+r)",
                ep.display(),
            );
            return;
        }
        let name = sysfs_str(&path.join("name"));
        let max = sysfs_u64(&path.join("max_energy_range_uj")).unwrap_or(u64::MAX);
        let dom = RaplDomain {
            energy_path: ep,
            max_uj: max,
        };
        if name.starts_with("package") {
            pkg.push(dom);
        } else if name.starts_with("psys") || name.starts_with("platform") {
            psys.push(dom);
        }
    };

    for entry in entries.filter_map(Result::ok) {
        let fname = entry.file_name();
        let name = fname.to_str().unwrap_or("");
        if !name.starts_with("intel-rapl:") && !name.starts_with("intel-rapl-mmio:") {
            continue;
        }
        push(&entry.path(), &mut pkg, &mut psys);
    }
    (pkg, psys)
}

/// Find a macsmc_hwmon sensor path by label.
fn detect_macsmc_sensor(label: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    for entry in entries.filter_map(Result::ok) {
        let hw = entry.path();
        if sysfs_str(&hw.join("name")) != "macsmc_hwmon" {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(&hw) {
            for f in files.filter_map(Result::ok) {
                let fname = f.file_name();
                let s = fname.to_str().unwrap_or("");
                if !s.starts_with("power") || !s.ends_with("_label") {
                    continue;
                }
                if sysfs_str(&f.path()) == label {
                    let input = hw.join(s.replace("_label", "_input"));
                    if sysfs_readable(&input) {
                        return Some(input);
                    }
                }
            }
        }
    }
    None
}

enum CpuSource {
    Rapl(Vec<RaplDomain>),
    Macsmc(PathBuf),
}

enum PsysSource {
    Rapl(Vec<RaplDomain>),
    Macsmc(PathBuf),
}

// ── GPU detection / read ──────────────────────────────────────────────

struct GpuInfo {
    power_file: Option<PathBuf>,
    energy_file: Option<PathBuf>,
    /// True when no sysfs node is available but `/proc/driver/nvidia/gpus`
    /// exists — we fall back to the `nvidia-smi` subprocess.
    nvidia_smi_fallback: bool,
}

fn find_gpu_hwmon(dev: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut pf = None;
    let mut ef = None;

    let mut scan = |hw: &Path| {
        let Ok(files) = std::fs::read_dir(hw) else {
            return;
        };
        for f in files.filter_map(Result::ok) {
            let n = f.file_name();
            let s = n.to_str().unwrap_or("");
            let p = f.path();
            if pf.is_none()
                && s.starts_with("power")
                && (s.ends_with("_average") || s.ends_with("_input"))
                && sysfs_readable(&p)
            {
                pf = Some(p.clone());
            }
            if ef.is_none()
                && s.starts_with("energy")
                && s.ends_with("_input")
                && sysfs_readable(&p)
            {
                ef = Some(p);
            }
        }
    };

    if let Ok(entries) = std::fs::read_dir(dev.join("hwmon")) {
        for e in entries.filter_map(Result::ok) {
            scan(&e.path());
        }
    }
    let dev_canon = std::fs::canonicalize(dev).ok();
    if let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") {
        for e in entries.filter_map(Result::ok) {
            if let Ok(link) = std::fs::canonicalize(e.path().join("device")) {
                if dev_canon.as_ref() == Some(&link) {
                    scan(&e.path());
                }
            }
        }
    }
    (pf, ef)
}

fn detect_gpu() -> Option<GpuInfo> {
    let mut best: Option<(GpuInfo, u32)> = None;

    if let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") {
        for entry in entries.filter_map(Result::ok) {
            let dev = entry.path();
            let class = sysfs_str(&dev.join("class"));
            if !class.starts_with("0x03") {
                continue;
            }
            let vendor = sysfs_str(&dev.join("vendor"));
            let bus = entry.file_name().to_str().unwrap_or("").to_string();

            // Lower rank wins. Mirrors rs-bar widget ordering.
            let rank = match vendor.as_str() {
                "0x10de" | "0x1002" => 1u32,                     // NVIDIA / AMD discrete
                "0x8086" if bus.starts_with("0000:00:02.") => 4, // Intel iGPU
                "0x8086" => 2,                                   // Intel ARC
                _ => continue,
            };

            let (pf, ef) = find_gpu_hwmon(&dev);
            if pf.is_none() && ef.is_none() {
                continue;
            }
            let r = if pf.is_some() { rank } else { rank + 1 };
            if best.as_ref().map_or(true, |(_, br)| r < *br) {
                best = Some((
                    GpuInfo {
                        power_file: pf,
                        energy_file: ef,
                        nvidia_smi_fallback: false,
                    },
                    r,
                ));
            }
        }
    }

    if best.is_none() && Path::new("/proc/driver/nvidia/gpus").exists() {
        return Some(GpuInfo {
            power_file: None,
            energy_file: None,
            nvidia_smi_fallback: true,
        });
    }

    best.map(|(g, _)| g)
}

fn nvidia_smi_watts() -> Option<f64> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=power.draw",
            "--format=csv,noheader,nounits",
            "-i",
            "0",
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

// ── shared math ───────────────────────────────────────────────────────

fn delta_watts(cur: u64, prev: u64, max: u64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    let d = if cur >= prev {
        cur - prev
    } else {
        // Counter wrapped: cur is post-wrap, prev is pre-wrap.
        cur + (max - prev)
    };
    (d as f64 / 1e6) / dt
}

fn sum_rapl_watts(domains: &[RaplDomain], prev_energies: &mut [u64], dt: f64) -> f64 {
    let mut watts = 0.0;
    for (i, d) in domains.iter().enumerate() {
        let cur = sysfs_u64(&d.energy_path).unwrap_or(0);
        watts += delta_watts(cur, prev_energies[i], d.max_uj, dt);
        prev_energies[i] = cur;
    }
    watts
}

// ── poller thread ─────────────────────────────────────────────────────

fn sender() -> &'static watch::Sender<PowerDrawSample> {
    static S: OnceLock<watch::Sender<PowerDrawSample>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(PowerDrawSample::default());
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("power-draw".into())
            .spawn(move || run_poller(producer))
            .ok();
        tx
    })
}

fn run_poller(producer: watch::Sender<PowerDrawSample>) {
    // ── source detection ──────────────────────────────────────────
    let battery = detect_battery();
    if let Some(b) = &battery {
        log::info!("power_draw: battery {}", b.dir.display());
    }

    let (rapl_pkg, rapl_psys) = detect_rapl();

    let cpu_source: Option<CpuSource> = if !rapl_pkg.is_empty() {
        Some(CpuSource::Rapl(rapl_pkg))
    } else {
        detect_macsmc_sensor("Heatpipe Power").map(CpuSource::Macsmc)
    };
    match &cpu_source {
        Some(CpuSource::Rapl(d)) => log::info!("power_draw: cpu RAPL ({} domain(s))", d.len()),
        Some(CpuSource::Macsmc(p)) => log::info!("power_draw: cpu macsmc {}", p.display()),
        None => log::info!("power_draw: cpu source unavailable"),
    }

    let psys_source: Option<PsysSource> = if !rapl_psys.is_empty() {
        Some(PsysSource::Rapl(rapl_psys))
    } else {
        detect_macsmc_sensor("Total System Power").map(PsysSource::Macsmc)
    };
    match &psys_source {
        Some(PsysSource::Rapl(d)) => log::info!("power_draw: psys RAPL ({} domain(s))", d.len()),
        Some(PsysSource::Macsmc(p)) => log::info!("power_draw: psys macsmc {}", p.display()),
        None => log::info!("power_draw: psys source unavailable"),
    }

    let gpu = detect_gpu();
    match &gpu {
        Some(g) if g.nvidia_smi_fallback => log::info!("power_draw: gpu via nvidia-smi"),
        Some(g) => log::info!(
            "power_draw: gpu sysfs (power={}, energy={})",
            g.power_file.is_some(),
            g.energy_file.is_some(),
        ),
        None => log::info!("power_draw: gpu source unavailable"),
    }

    // If nothing was detected at all, there's nothing to publish.
    if battery.is_none() && cpu_source.is_none() && psys_source.is_none() && gpu.is_none() {
        log::warn!("power_draw: no power sources detected, poller exiting");
        return;
    }

    // ── per-source state ─────────────────────────────────────────
    let mut cpu_prev_energies: Vec<u64> = match &cpu_source {
        Some(CpuSource::Rapl(d)) => d
            .iter()
            .map(|x| sysfs_u64(&x.energy_path).unwrap_or(0))
            .collect(),
        _ => Vec::new(),
    };
    let mut psys_prev_energies: Vec<u64> = match &psys_source {
        Some(PsysSource::Rapl(d)) => d
            .iter()
            .map(|x| sysfs_u64(&x.energy_path).unwrap_or(0))
            .collect(),
        _ => Vec::new(),
    };
    let mut cpu_prev_time = Instant::now();
    let mut psys_prev_time = Instant::now();

    let mut gpu_prev_energy: Option<u64> = gpu
        .as_ref()
        .and_then(|g| g.energy_file.as_ref())
        .and_then(|p| sysfs_u64(p));
    let mut gpu_prev_time = Instant::now();

    // ── tick ─────────────────────────────────────────────────────
    timerfd_loop(POLL_INTERVAL, false, || {
        let mut sample = PowerDrawSample::default();

        if let Some(b) = &battery {
            sample.battery_w = Some(read_battery_watts(b));
        }

        match &cpu_source {
            Some(CpuSource::Rapl(domains)) => {
                let now = Instant::now();
                let dt = now.duration_since(cpu_prev_time).as_secs_f64();
                let w = sum_rapl_watts(domains, &mut cpu_prev_energies, dt);
                cpu_prev_time = now;
                sample.cpu_w = Some(w);
            }
            Some(CpuSource::Macsmc(path)) => {
                sample.cpu_w = Some(sysfs_u64(path).map(|uw| uw as f64 / 1e6).unwrap_or(0.0));
            }
            None => {}
        }

        match &psys_source {
            Some(PsysSource::Rapl(domains)) => {
                let now = Instant::now();
                let dt = now.duration_since(psys_prev_time).as_secs_f64();
                let w = sum_rapl_watts(domains, &mut psys_prev_energies, dt);
                psys_prev_time = now;
                sample.psys_w = Some(w);
            }
            Some(PsysSource::Macsmc(path)) => {
                sample.psys_w = Some(sysfs_u64(path).map(|uw| uw as f64 / 1e6).unwrap_or(0.0));
            }
            None => {}
        }

        if let Some(g) = &gpu {
            let w = if let Some(p) = &g.power_file {
                sysfs_u64(p).map(|uw| uw as f64 / 1e6).unwrap_or(0.0)
            } else if let Some(p) = &g.energy_file {
                let now = Instant::now();
                let dt = now.duration_since(gpu_prev_time).as_secs_f64();
                let cur = sysfs_u64(p).unwrap_or(0);
                let w = match gpu_prev_energy {
                    Some(prev) => delta_watts(cur, prev, u64::MAX, dt),
                    None => 0.0,
                };
                gpu_prev_time = now;
                gpu_prev_energy = Some(cur);
                w
            } else if g.nvidia_smi_fallback {
                nvidia_smi_watts().unwrap_or(0.0)
            } else {
                0.0
            };
            sample.gpu_w = Some(w);
        }

        // Returning false would exit the loop; in practice the sender is
        // held by the OnceLock for the program's lifetime so this never
        // happens.
        producer.send(sample).is_ok()
    });
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<PowerDrawSample> {
    sender().subscribe()
}
