//! Power draw widgets — individual, composable via `group!()`.
//!
//! Four independent widgets, each with its own timerfd + epoll monitor:
//!   - `BatteryDraw` — battery discharge/charge watts
//!   - `CpuDraw`     — CPU package watts (RAPL or macsmc Heatpipe)
//!   - `PsysDraw`    — platform/system watts (RAPL psys or macsmc Total System)
//!   - `GpuDraw`     — discrete GPU watts (hwmon or nvidia-smi)
//!
//! All reads are sysfs — zero subprocesses (nvidia-smi as last resort for GPU).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use crate::gpui_bar::hub::Broadcast;

use super::{BarWidget, impl_render};

// ── sysfs helpers ──────────────────────────────────────────────────────

pub(crate) fn sysfs_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub(crate) fn sysfs_i64(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub(crate) fn sysfs_str(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn sysfs_readable(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok()
}

// ── timerfd + epoll helper ─────────────────────────────────────────────

/// Run `tick` every `interval_secs` on a timerfd + epoll loop.
/// Returns when `tick` returns `false` (channel closed).
pub(crate) fn timerfd_loop(
    interval_secs: i64,
    fire_immediately: bool,
    mut tick: impl FnMut() -> bool,
) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec {
            tv_sec: interval_secs,
            tv_nsec: 0,
        },
        it_value: libc::timespec {
            tv_sec: if fire_immediately { 0 } else { interval_secs },
            tv_nsec: if fire_immediately { 1 } else { 0 },
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
        )
    };

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

// ── shared types & detection ───────────────────────────────────────────

pub(crate) struct BatteryInfo {
    pub(crate) dir: PathBuf,
}

struct RaplDomain {
    energy_path: PathBuf,
    max_uj: u64,
}

#[allow(dead_code)]
struct GpuInfo {
    power_file: Option<PathBuf>,
    energy_file: Option<PathBuf>,
    label: String,
}

#[derive(Clone, Copy)]
enum CpuVendor {
    Intel,
    Amd,
    Apple,
    Unknown,
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
    if Path::new("/sys/class/power_supply/macsmc-battery").exists() || cpuinfo.contains("Apple") {
        return CpuVendor::Apple;
    }
    CpuVendor::Unknown
}

// Icon paths
const ICON_AMD_CPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/amd-cpu.svg");
const ICON_INTEL_CPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/intel-cpu.svg");
const ICON_APPLE_CHIP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/apple-chip.svg");
#[allow(dead_code)]
const ICON_AMD_RADEON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/amd-radeon.svg");
#[allow(dead_code)]
const ICON_NVIDIA_GPU: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/nvidia-gpu.svg");
#[allow(dead_code)]
const ICON_INTEL_ARC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/icons/intel-arc-gpu.svg"
);
const ICON_PSYS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/psys.svg");
const ICON_BATTERY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/battery.svg");
const ICON_BATTERY_CHARGING: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/icons/battery-charging.svg"
);

fn is_system_battery(dir: &Path) -> bool {
    dir.join("power_now").exists()
        || (dir.join("current_now").exists() && dir.join("voltage_now").exists())
}

pub(crate) fn detect_battery() -> Option<BatteryInfo> {
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
                "power: {} unreadable (install udev rule or chmod a+r)",
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

/// Find macsmc_hwmon sensor path by label.
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

#[allow(dead_code)]
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

        let (label, rank) = match vendor.as_str() {
            "0x10de" => ("NVIDIA", 1u32),
            "0x1002" => ("AMD", 1),
            "0x8086" if bus.starts_with("0000:00:02.") => ("iGPU", 4),
            "0x8086" => ("ARC", 2),
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
                },
                r,
            ));
        }
    }

    if best.is_none() && Path::new("/proc/driver/nvidia/gpus").exists() {
        return Some(GpuInfo {
            power_file: None,
            energy_file: None,
            label: "NVIDIA".to_string(),
        });
    }

    best.map(|(g, _)| g)
}

#[allow(dead_code)]
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

fn read_battery(bat: &BatteryInfo) -> (f64, bool) {
    let charging = sysfs_str(&bat.dir.join("status")) == "Charging";
    if let Some(w) = sysfs_i64(&bat.dir.join("power_now")) {
        return (w.unsigned_abs() as f64 / 1e6, charging);
    }
    if let (Some(ua), Some(uv)) = (
        sysfs_i64(&bat.dir.join("current_now")),
        sysfs_i64(&bat.dir.join("voltage_now")),
    ) {
        return (
            (ua.unsigned_abs() as f64 * uv.unsigned_abs() as f64) / 1e12,
            charging,
        );
    }
    (0.0, charging)
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

// ── render helpers ─────────────────────────────────────────────────────

fn icon_el(path: &str, color: u32) -> impl IntoElement {
    svg()
        .external_path(path.to_string())
        .size(px(crate::gpui_bar::config::ICON_SIZE()))
        .text_color(rgb(color))
        .flex_shrink_0()
}

fn watts_el(watts: f64, color: u32) -> impl IntoElement {
    div().text_color(rgb(color)).child(format!("{:.1}W", watts))
}

fn icon_watts(icon_path: &str, watts: f64, icon_color: u32, text_color: u32) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .child(icon_el(icon_path, icon_color))
        .child(watts_el(watts, text_color))
}

fn cpu_vendor_icon(v: CpuVendor) -> &'static str {
    match v {
        CpuVendor::Amd => ICON_AMD_CPU,
        CpuVendor::Intel => ICON_INTEL_CPU,
        CpuVendor::Apple => ICON_APPLE_CHIP,
        CpuVendor::Unknown => "",
    }
}

#[allow(dead_code)]
fn gpu_vendor_icon(label: &str) -> &'static str {
    match label {
        "AMD" => ICON_AMD_RADEON,
        "NVIDIA" => ICON_NVIDIA_GPU,
        "ARC" => ICON_INTEL_ARC,
        _ => "",
    }
}

// ════════════════════════════════════════════════════════════════════════
// BatteryDraw
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
struct BatteryReading {
    watts: f64,
    charging: bool,
}

pub struct BatteryDraw {
    reading: Option<BatteryReading>,
    grouped: bool,
}

fn battery_draw_broadcast() -> Option<&'static Broadcast<BatteryReading>> {
    static BC: OnceLock<Option<Broadcast<BatteryReading>>> = OnceLock::new();
    BC.get_or_init(|| {
        let bat = detect_battery()?;
        log::info!("battery-draw: {}", bat.dir.display());
        let bc = Broadcast::<BatteryReading>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("battery-draw".into())
            .spawn(move || {
                timerfd_loop(2, true, || {
                    let (watts, charging) = read_battery(&bat);
                    producer.publish(BatteryReading { watts, charging });
                    true
                });
            })
            .ok();
        Some(bc)
    })
    .as_ref()
}

/// Quantise a float watts value to the precision of its display
/// (one decimal place: `X.X`). Used by all *_draw widgets to skip
/// redundant paints when the displayed string would be identical.
fn quantise_watts(w: f64) -> i32 {
    (w * 10.0).round() as i32
}

impl BarWidget for BatteryDraw {
    const NAME: &str = "battery-draw";

    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(bc) = battery_draw_broadcast() {
            let sub = bc.subscribe();
            cx.spawn(async move |this, cx| {
                while let Some(r) = sub.next().await {
                    if this
                        .update(cx, |this, cx| {
                            // Only notify when the rendered `X.XW` or
                            // the charging state would change.
                            let changed = match &this.reading {
                                Some(prev) => {
                                    quantise_watts(prev.watts) != quantise_watts(r.watts)
                                        || prev.charging != r.charging
                                }
                                None => true,
                            };
                            if changed {
                                this.reading = Some(r);
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
        }

        Self {
            reading: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();

        let Some(r) = &self.reading else {
            return super::capsule(div(), self.grouped);
        };

        let bat_icon = if r.charging {
            ICON_BATTERY_CHARGING
        } else {
            ICON_BATTERY
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .child(icon_watts(bat_icon, r.watts, t.fg, t.fg)),
            self.grouped,
        )
    }
}

impl_render!(BatteryDraw);

// ════════════════════════════════════════════════════════════════════════
// CpuDraw — RAPL package or macsmc Heatpipe Power
// ════════════════════════════════════════════════════════════════════════

enum CpuPowerSource {
    Rapl(Vec<RaplDomain>),
    Macsmc(PathBuf),
}

#[derive(Clone)]
struct CpuPowerReading {
    watts: f64,
    vendor: CpuVendor,
}

pub struct CpuDraw {
    reading: Option<CpuPowerReading>,
    grouped: bool,
}

fn cpu_draw_broadcast() -> Option<&'static Broadcast<CpuPowerReading>> {
    static BC: OnceLock<Option<Broadcast<CpuPowerReading>>> = OnceLock::new();
    BC.get_or_init(|| {
        let vendor = detect_cpu_vendor();
        let (pkg, _) = detect_rapl();
        let source = if !pkg.is_empty() {
            Some(CpuPowerSource::Rapl(pkg))
        } else {
            detect_macsmc_sensor("Heatpipe Power").map(CpuPowerSource::Macsmc)
        }?;
        log::info!("cpu-draw: source detected (vendor={:?})", vendor as u8);
        let bc = Broadcast::<CpuPowerReading>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("cpu-draw".into())
            .spawn(move || match source {
                CpuPowerSource::Macsmc(path) => {
                    timerfd_loop(2, true, || {
                        let watts = sysfs_u64(&path).map(|uw| uw as f64 / 1e6).unwrap_or(0.0);
                        producer.publish(CpuPowerReading { watts, vendor });
                        true
                    });
                }
                CpuPowerSource::Rapl(domains) => {
                    let mut prev_time = Instant::now();
                    let mut prev_energies: Vec<u64> = domains
                        .iter()
                        .map(|d| sysfs_u64(&d.energy_path).unwrap_or(0))
                        .collect();
                    timerfd_loop(2, false, || {
                        let now = Instant::now();
                        let dt = now.duration_since(prev_time).as_secs_f64();
                        let mut watts = 0.0;
                        for (i, d) in domains.iter().enumerate() {
                            let cur = sysfs_u64(&d.energy_path).unwrap_or(0);
                            watts += delta_watts(cur, prev_energies[i], d.max_uj, dt);
                            prev_energies[i] = cur;
                        }
                        prev_time = now;
                        producer.publish(CpuPowerReading { watts, vendor });
                        true
                    });
                }
            })
            .ok();
        Some(bc)
    })
    .as_ref()
}

impl BarWidget for CpuDraw {
    const NAME: &str = "cpu-draw";

    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(bc) = cpu_draw_broadcast() {
            let sub = bc.subscribe();
            cx.spawn(async move |this, cx| {
                while let Some(r) = sub.next().await {
                    if this
                        .update(cx, |this, cx| {
                            let changed = match &this.reading {
                                Some(prev) => quantise_watts(prev.watts) != quantise_watts(r.watts),
                                None => true,
                            };
                            if changed {
                                this.reading = Some(r);
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
        }

        Self {
            reading: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();

        let Some(r) = &self.reading else {
            return super::capsule(div(), self.grouped);
        };

        let icon = cpu_vendor_icon(r.vendor);
        let content = if !icon.is_empty() {
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .child(icon_watts(icon, r.watts, t.fg, t.fg))
        } else {
            div().flex().items_center().px(px(4.0)).text_xs().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(div().text_color(rgb(t.text_dim)).child("CPU"))
                    .child(watts_el(r.watts, t.fg)),
            )
        };

        super::capsule(content, self.grouped)
    }
}

impl_render!(CpuDraw);

// ════════════════════════════════════════════════════════════════════════
// PsysDraw — RAPL psys/platform or macsmc Total System Power
// ════════════════════════════════════════════════════════════════════════

enum PsysPowerSource {
    Rapl(Vec<RaplDomain>),
    Macsmc(PathBuf),
}

#[derive(Clone)]
struct PsysReading {
    watts: f64,
}

pub struct PsysDraw {
    reading: Option<PsysReading>,
    grouped: bool,
}

fn psys_draw_broadcast() -> Option<&'static Broadcast<PsysReading>> {
    static BC: OnceLock<Option<Broadcast<PsysReading>>> = OnceLock::new();
    BC.get_or_init(|| {
        let (_, psys) = detect_rapl();
        let source = if !psys.is_empty() {
            Some(PsysPowerSource::Rapl(psys))
        } else {
            detect_macsmc_sensor("Total System Power").map(PsysPowerSource::Macsmc)
        }?;
        log::info!("psys-draw: source detected");
        let bc = Broadcast::<PsysReading>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("psys-draw".into())
            .spawn(move || match source {
                PsysPowerSource::Macsmc(path) => {
                    timerfd_loop(2, true, || {
                        let watts = sysfs_u64(&path).map(|uw| uw as f64 / 1e6).unwrap_or(0.0);
                        producer.publish(PsysReading { watts });
                        true
                    });
                }
                PsysPowerSource::Rapl(domains) => {
                    let mut prev_time = Instant::now();
                    let mut prev_energies: Vec<u64> = domains
                        .iter()
                        .map(|d| sysfs_u64(&d.energy_path).unwrap_or(0))
                        .collect();
                    timerfd_loop(2, false, || {
                        let now = Instant::now();
                        let dt = now.duration_since(prev_time).as_secs_f64();
                        let mut watts = 0.0;
                        for (i, d) in domains.iter().enumerate() {
                            let cur = sysfs_u64(&d.energy_path).unwrap_or(0);
                            watts += delta_watts(cur, prev_energies[i], d.max_uj, dt);
                            prev_energies[i] = cur;
                        }
                        prev_time = now;
                        producer.publish(PsysReading { watts });
                        true
                    });
                }
            })
            .ok();
        Some(bc)
    })
    .as_ref()
}

impl BarWidget for PsysDraw {
    const NAME: &str = "psys-draw";

    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(bc) = psys_draw_broadcast() {
            let sub = bc.subscribe();
            cx.spawn(async move |this, cx| {
                while let Some(r) = sub.next().await {
                    if this
                        .update(cx, |this, cx| {
                            let changed = match &this.reading {
                                Some(prev) => quantise_watts(prev.watts) != quantise_watts(r.watts),
                                None => true,
                            };
                            if changed {
                                this.reading = Some(r);
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
        }

        Self {
            reading: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();

        let Some(r) = &self.reading else {
            return super::capsule(div(), self.grouped);
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .child(icon_watts(ICON_PSYS, r.watts, t.fg, t.fg)),
            self.grouped,
        )
    }
}

impl_render!(PsysDraw);

// ════════════════════════════════════════════════════════════════════════
// GpuDraw — discrete GPU power
// ════════════════════════════════════════════════════════════════════════

#[derive(Clone)]
#[allow(dead_code)]
struct GpuPowerReading {
    watts: f64,
    label: String,
    icon: &'static str,
}

#[allow(dead_code)]
pub struct GpuDraw {
    reading: Option<GpuPowerReading>,
    grouped: bool,
}

fn gpu_draw_broadcast() -> Option<&'static Broadcast<GpuPowerReading>> {
    static BC: OnceLock<Option<Broadcast<GpuPowerReading>>> = OnceLock::new();
    BC.get_or_init(|| {
        let gpu = detect_gpu()?;
        log::info!("gpu-draw: {} detected", gpu.label);
        let icon = gpu_vendor_icon(&gpu.label);
        let label = gpu.label.clone();
        let bc = Broadcast::<GpuPowerReading>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("gpu-draw".into())
            .spawn(move || {
                let mut prev_time = Instant::now();
                let mut prev_energy: Option<u64> =
                    gpu.energy_file.as_ref().and_then(|p| sysfs_u64(p));
                timerfd_loop(2, true, || {
                    let watts = if let Some(ref p) = gpu.power_file {
                        sysfs_u64(p).map(|uw| uw as f64 / 1e6).unwrap_or(0.0)
                    } else if let Some(ref p) = gpu.energy_file {
                        let now = Instant::now();
                        let dt = now.duration_since(prev_time).as_secs_f64();
                        let cur = sysfs_u64(p).unwrap_or(0);
                        let w = if let Some(prev) = prev_energy {
                            delta_watts(cur, prev, u64::MAX, dt)
                        } else {
                            0.0
                        };
                        prev_time = now;
                        prev_energy = Some(cur);
                        w
                    } else {
                        std::process::Command::new("nvidia-smi")
                            .args([
                                "--query-gpu=power.draw",
                                "--format=csv,noheader,nounits",
                                "-i",
                                "0",
                            ])
                            .output()
                            .ok()
                            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
                            .unwrap_or(0.0)
                    };
                    producer.publish(GpuPowerReading {
                        watts,
                        label: label.clone(),
                        icon,
                    });
                    true
                });
            })
            .ok();
        Some(bc)
    })
    .as_ref()
}

impl BarWidget for GpuDraw {
    const NAME: &str = "gpu-draw";

    fn new(cx: &mut Context<Self>) -> Self {
        if let Some(bc) = gpu_draw_broadcast() {
            let sub = bc.subscribe();
            cx.spawn(async move |this, cx| {
                while let Some(r) = sub.next().await {
                    if this
                        .update(cx, |this, cx| {
                            let changed = match &this.reading {
                                Some(prev) => quantise_watts(prev.watts) != quantise_watts(r.watts),
                                None => true,
                            };
                            if changed {
                                this.reading = Some(r);
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
        }

        Self {
            reading: None,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();

        let Some(r) = &self.reading else {
            return super::capsule(div(), self.grouped);
        };

        let content = if !r.icon.is_empty() {
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .child(icon_watts(r.icon, r.watts, t.fg, t.fg))
        } else {
            div().flex().items_center().px(px(4.0)).text_xs().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(div().text_color(rgb(t.text_dim)).child(r.label.clone()))
                    .child(watts_el(r.watts, t.fg)),
            )
        };

        super::capsule(content, self.grouped)
    }
}

impl_render!(GpuDraw);
