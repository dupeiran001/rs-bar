//! Power draw widget.
//!
//! Mirrors the logic of `~/.config/waybar/scripts/power-draw.sh`:
//! battery, RAPL (CPU package + PSYS/platform), dGPU hwmon.
//! All reads are sysfs — zero subprocesses (nvidia-smi as last resort).
//!
//! Uses `timerfd` + `epoll` for a 2-second sampling interval.
//! Energy counters are delta'd to compute watts.

use std::collections::HashMap;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::Instant;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

// ── sysfs helpers ──────────────────────────────────────────────────────

fn sysfs_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn sysfs_str(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn sysfs_readable(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok()
}

// ── source detection (runs once at startup) ────────────────────────────

struct BatteryInfo {
    dir: PathBuf,
}

struct RaplDomain {
    energy_path: PathBuf,
    max_uj: u64,
}

struct GpuInfo {
    power_file: Option<PathBuf>,
    energy_file: Option<PathBuf>,
    label: String,
    is_dgpu: bool,
}

#[derive(Clone, Copy)]
enum CpuVendor {
    Intel,
    Amd,
    Apple,
    Unknown,
}

struct PowerSources {
    battery: Option<BatteryInfo>,
    cpu_vendor: CpuVendor,
    pkg: Vec<RaplDomain>,
    psys: Vec<RaplDomain>,
    gpu: Option<GpuInfo>,
}

fn detect_cpu_vendor() -> CpuVendor {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    for line in cpuinfo.lines() {
        if line.starts_with("vendor_id") {
            if line.contains("GenuineIntel") {
                return CpuVendor::Intel;
            } else if line.contains("AuthenticAMD") {
                return CpuVendor::Amd;
            }
        }
    }
    // Apple Silicon check
    if Path::new("/sys/class/power_supply/macsmc-battery").exists()
        || std::fs::read_to_string("/proc/cpuinfo")
            .unwrap_or_default()
            .contains("Apple")
    {
        return CpuVendor::Apple;
    }
    CpuVendor::Unknown
}

// Icon paths (resolved at compile time)
const ICON_AMD_CPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/amd-cpu.svg");
const ICON_INTEL_CPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/intel-cpu.svg");
const ICON_APPLE_CHIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/apple-chip.svg");
const ICON_AMD_RADEON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/amd-radeon.svg");
const ICON_NVIDIA_GPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/nvidia-gpu.svg");
const ICON_INTEL_ARC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/intel-arc-gpu.svg");
const ICON_PSYS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/psys.svg");

fn is_system_battery(dir: &Path) -> bool {
    // Must have power_now or current_now+voltage_now to be a real system battery
    // (filters out Logitech HID++ peripheral batteries etc.)
    dir.join("power_now").exists()
        || (dir.join("current_now").exists() && dir.join("voltage_now").exists())
}

fn detect_battery() -> Option<BatteryInfo> {
    // Prefer well-known names, then fall back to scanning
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

fn detect_rapl() -> (Vec<RaplDomain>, Vec<RaplDomain>) {
    let mut pkg = Vec::new();
    let mut psys = Vec::new();

    let base = Path::new("/sys/class/powercap");
    let Ok(entries) = std::fs::read_dir(base) else {
        return (pkg, psys);
    };

    let mut push = |path: &Path, pkg: &mut Vec<RaplDomain>, psys: &mut Vec<RaplDomain>| {
        let ep = path.join("energy_uj");
        if !ep.exists() {
            return;
        }
        if !sysfs_readable(&ep) {
            log::warn!(
                "power: {} exists but unreadable (run: sudo chmod a+r {} or install udev rule)",
                ep.display(),
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

    // All domains (including subdomains like intel-rapl:0:0) are
    // top-level symlinks in /sys/class/powercap/. No need to recurse —
    // that would hit `device/` symlinks causing duplicates.
    for entry in entries.filter_map(Result::ok) {
        let fname = entry.file_name();
        let name = fname.to_str().unwrap_or("");
        // Only process "intel-rapl:N" or "intel-rapl:N:M" entries
        if !name.starts_with("intel-rapl:") && !name.starts_with("intel-rapl-mmio:") {
            continue;
        }
        push(&entry.path(), &mut pkg, &mut psys);
    }
    (pkg, psys)
}

fn detect_gpu() -> Option<GpuInfo> {
    let mut best: Option<(GpuInfo, u32)> = None;

    let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return None;
    };

    for entry in entries.filter_map(Result::ok) {
        let dev = entry.path();
        let class = sysfs_str(&dev.join("class"));
        if !class.starts_with("0x03") {
            continue;
        }
        let vendor = sysfs_str(&dev.join("vendor"));
        let bus = entry.file_name().to_str().unwrap_or("").to_string();

        let (label, is_dgpu, rank) = match vendor.as_str() {
            "0x10de" => ("NVIDIA", true, 1u32),
            "0x1002" => ("AMD", true, 1),
            "0x8086" if bus.starts_with("0000:00:02.") => ("iGPU", false, 4),
            "0x8086" => ("ARC", true, 2),
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
                    label: label.to_string(),
                    is_dgpu,
                },
                r,
            ));
        }
    }

    if best.is_none() && Path::new("/proc/driver/nvidia/gpus").exists() {
        // nvidia-smi fallback
        return Some(GpuInfo {
            power_file: None,
            energy_file: None,
            label: "NVIDIA".to_string(),
            is_dgpu: true,
        });
    }

    best.map(|(g, _)| g)
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

    // Direct hwmon under the PCI device
    if let Ok(entries) = std::fs::read_dir(dev.join("hwmon")) {
        for e in entries.filter_map(Result::ok) {
            scan(&e.path());
        }
    }
    // Also check /sys/class/hwmon entries pointing to this device
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

fn detect_sources() -> PowerSources {
    let battery = detect_battery();
    let cpu_vendor = detect_cpu_vendor();
    let (pkg, psys) = detect_rapl();
    let gpu = detect_gpu();
    log::info!(
        "power: battery={} rapl_pkg={} rapl_psys={} gpu={}",
        battery.is_some(),
        pkg.len(),
        psys.len(),
        gpu.as_ref().map_or("none".into(), |g| g.label.clone()),
    );
    PowerSources {
        battery,
        cpu_vendor,
        pkg,
        psys,
        gpu,
    }
}

// ── snapshot & delta computation ───────────────────────────────────────

struct Snapshot {
    time: Instant,
    energies: HashMap<PathBuf, u64>,
}

fn take_snapshot(src: &PowerSources) -> Snapshot {
    let mut energies = HashMap::new();
    for d in src.pkg.iter().chain(src.psys.iter()) {
        if let Some(v) = sysfs_u64(&d.energy_path) {
            energies.insert(d.energy_path.clone(), v);
        }
    }
    if let Some(ref g) = src.gpu {
        if let Some(ref p) = g.energy_file {
            if let Some(v) = sysfs_u64(p) {
                energies.insert(p.clone(), v);
            }
        }
    }
    Snapshot {
        time: Instant::now(),
        energies,
    }
}

fn delta_watts(cur: u64, prev: u64, max: u64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    let d = if cur >= prev {
        cur - prev
    } else {
        cur + (max - prev)
    };
    (d as f64 / 1e6) / dt
}

fn read_battery(bat: &BatteryInfo) -> (f64, bool) {
    let charging = sysfs_str(&bat.dir.join("status")) == "Charging";
    if let Some(uw) = sysfs_u64(&bat.dir.join("power_now")) {
        return (uw as f64 / 1e6, charging);
    }
    if let (Some(ua), Some(uv)) = (
        sysfs_u64(&bat.dir.join("current_now")),
        sysfs_u64(&bat.dir.join("voltage_now")),
    ) {
        return ((ua as f64 * uv as f64) / 1e12, charging);
    }
    (0.0, charging)
}

fn read_gpu_direct(gpu: &GpuInfo) -> Option<f64> {
    if let Some(ref p) = gpu.power_file {
        return sysfs_u64(p).map(|uw| uw as f64 / 1e6);
    }
    // nvidia-smi fallback (only when no hwmon files exist)
    if gpu.power_file.is_none() && gpu.energy_file.is_none() {
        let o = std::process::Command::new("nvidia-smi")
            .args(["--query-gpu=power.draw", "--format=csv,noheader,nounits", "-i", "0"])
            .output()
            .ok()?;
        return String::from_utf8_lossy(&o.stdout).trim().parse().ok();
    }
    None
}

#[derive(Clone)]
struct PowerReading {
    battery_watts: f64,
    battery_charging: bool,
    has_battery: bool,
    gpu_watts: f64,
    gpu_label: String,
    gpu_icon: &'static str,
    has_dgpu: bool,
    cpu_watts: f64,
    cpu_vendor: CpuVendor,
    has_cpu: bool,
    psys_watts: f64,
    has_psys: bool,
}

fn compute(src: &PowerSources, prev: &Snapshot, cur: &Snapshot) -> PowerReading {
    let dt = cur.time.duration_since(prev.time).as_secs_f64();

    // Battery
    let (battery_watts, battery_charging) = src
        .battery
        .as_ref()
        .map(|b| read_battery(b))
        .unwrap_or((0.0, false));

    // CPU package watts (sum across sockets)
    let cpu_watts: f64 = src
        .pkg
        .iter()
        .map(|d| {
            match (
                cur.energies.get(&d.energy_path),
                prev.energies.get(&d.energy_path),
            ) {
                (Some(&c), Some(&p)) => delta_watts(c, p, d.max_uj, dt),
                _ => 0.0,
            }
        })
        .sum();

    // PSYS/platform watts
    let psys_watts: f64 = src
        .psys
        .iter()
        .map(|d| {
            match (
                cur.energies.get(&d.energy_path),
                prev.energies.get(&d.energy_path),
            ) {
                (Some(&c), Some(&p)) => delta_watts(c, p, d.max_uj, dt),
                _ => 0.0,
            }
        })
        .sum();

    // GPU watts
    let gpu_watts = src.gpu.as_ref().map_or(0.0, |g| {
        if let Some(w) = read_gpu_direct(g) {
            return w;
        }
        if let Some(ref p) = g.energy_file {
            if let (Some(&c), Some(&pr)) = (cur.energies.get(p), prev.energies.get(p)) {
                return delta_watts(c, pr, u64::MAX, dt);
            }
        }
        0.0
    });

    let gpu_icon = src.gpu.as_ref().map_or("", |g| match g.label.as_str() {
        "AMD" => ICON_AMD_RADEON,
        "NVIDIA" => ICON_NVIDIA_GPU,
        "ARC" => ICON_INTEL_ARC,
        _ => "",
    });

    PowerReading {
        battery_watts,
        battery_charging,
        has_battery: src.battery.is_some(),
        gpu_watts,
        gpu_label: src
            .gpu
            .as_ref()
            .map_or(String::new(), |g| g.label.clone()),
        gpu_icon,
        has_dgpu: src.gpu.as_ref().map_or(false, |g| g.is_dgpu),
        cpu_watts,
        cpu_vendor: src.cpu_vendor,
        has_cpu: !src.pkg.is_empty(),
        psys_watts,
        has_psys: !src.psys.is_empty(),
    }
}

// ── timerfd + epoll monitor thread ─────────────────────────────────────

fn power_monitor(tx: async_channel::Sender<PowerReading>) {
    let src = detect_sources();

    // Create timerfd (2-second interval)
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        log::warn!("power: timerfd_create: {}", std::io::Error::last_os_error());
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 2, tv_nsec: 0 },
        it_value: libc::timespec { tv_sec: 0, tv_nsec: 1 }, // fire immediately
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    // Epoll on the timerfd
    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 0,
    };
    unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, tfd.as_raw_fd(), &mut ev) };

    log::info!("power: timerfd+epoll monitor started");

    let mut prev = take_snapshot(&src);

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }

        // Consume timerfd expiration count
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        let cur = take_snapshot(&src);
        let reading = compute(&src, &prev, &cur);
        prev = cur;

        if tx.try_send(reading).is_err() && tx.is_closed() {
            break;
        }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct PowerDraw {
    reading: PowerReading,
}

impl BarWidget for PowerDraw {
    const NAME: &str = "power-draw";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<PowerReading>(1);

        std::thread::Builder::new()
            .name("power-draw".into())
            .spawn(move || power_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(reading) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.reading = reading;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            reading: PowerReading {
                battery_watts: 0.0,
                battery_charging: false,
                has_battery: false,
                gpu_watts: 0.0,
                gpu_label: String::new(),
                gpu_icon: "",
                has_dgpu: false,
                cpu_watts: 0.0,
                cpu_vendor: CpuVendor::Unknown,
                has_cpu: false,
                psys_watts: 0.0,
                has_psys: false,
            },
        }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let r = &self.reading;

        let sep = || div().text_color(rgb(t.border)).child("│");

        let segment = |label: &str, watts: f64, color: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(div().text_color(rgb(t.text_dim)).child(label.to_string()))
                .child(
                    div()
                        .text_color(rgb(color))
                        .child(format!("{:.1}W", watts)),
                )
        };

        let icon_size = crate::config::ICON_SIZE;

        let icon_el = |path: &str, color: u32| {
            svg()
                .external_path(path.to_string())
                .size(px(icon_size))
                .text_color(rgb(color))
                .flex_shrink_0()
        };

        let watts_el = |watts: f64, color: u32| {
            div()
                .text_color(rgb(color))
                .child(format!("{:.1}W", watts))
        };

        let icon_segment = |icon_path: &str, watts: f64, icon_color: u32, text_color: u32| {
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(icon_el(icon_path, icon_color))
                .child(watts_el(watts, text_color))
        };

        let mut row = div().flex().items_center().gap(px(4.0));
        let mut segments = 0;

        // Battery
        if r.has_battery {
            let icon = if r.battery_charging { "" } else { "" };
            row = row.child(
                div()
                    .text_color(rgb(t.fg))
                    .child(format!("{} {:.1}W", icon, r.battery_watts)),
            );
            segments += 1;
        }

        // dGPU
        if r.has_dgpu && !r.gpu_icon.is_empty() {
            if segments > 0 {
                row = row.child(sep());
            }
            row = row.child(icon_segment(r.gpu_icon, r.gpu_watts, t.fg, t.fg));
            segments += 1;
        } else if r.has_dgpu {
            if segments > 0 {
                row = row.child(sep());
            }
            row = row.child(segment(&r.gpu_label, r.gpu_watts, t.fg));
            segments += 1;
        }

        // CPU (RAPL)
        if r.has_cpu {
            if segments > 0 {
                row = row.child(sep());
            }
            let cpu_icon = match r.cpu_vendor {
                CpuVendor::Amd => ICON_AMD_CPU,
                CpuVendor::Intel => ICON_INTEL_CPU,
                CpuVendor::Apple => ICON_APPLE_CHIP,
                CpuVendor::Unknown => "",
            };
            if !cpu_icon.is_empty() {
                row = row.child(icon_segment(cpu_icon, r.cpu_watts, t.fg, t.fg));
            } else {
                row = row.child(segment("CPU", r.cpu_watts, t.fg));
            }
            segments += 1;
        }

        // PSYS
        if r.has_psys {
            if segments > 0 {
                row = row.child(sep());
            }
            row = row.child(icon_segment(ICON_PSYS, r.psys_watts, t.fg, t.fg));
            segments += 1;
        }

        // Nothing available
        if segments == 0 {
            row = row.child(div().text_color(rgb(t.fg_dark)).child("⚡ —"));
        }

        div()
            .flex()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .px(px(8.0))
            .text_xs()
            .child(row)
    }
}

impl_render!(PowerDraw);
