//! CPU temperature widget.
//!
//! Auto-detects the CPU package/die temperature source:
//!   - Intel: coretemp hwmon, label "Package id 0"
//!   - AMD: k10temp hwmon, label "Tctl"
//!   - Apple Silicon (Asahi): macsmc-hwmon "Charge Regulator Temp" or similar
//!   - Fallback: x86_pkg_temp thermal zone, then thermal_zone0
//!
//! Reads every 2 seconds via `timerfd` + `epoll`. All sysfs, zero subprocesses.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

const ICON_THERMO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/thermometer.svg");

// ── temperature source detection ───────────────────────────────────────

enum TempSource {
    Hwmon(PathBuf),        // direct temp_input file (millidegrees)
    ThermalZone(PathBuf),  // /sys/class/thermal/thermal_zoneN/temp
}

fn detect_temp_source() -> Option<TempSource> {
    // 1. Scan hwmon for known CPU temperature sensors
    if let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") {
        for entry in entries.filter_map(Result::ok) {
            let hw = entry.path();
            let name = std::fs::read_to_string(hw.join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();

            match name.as_str() {
                // Intel: coretemp — look for "Package id 0"
                "coretemp" => {
                    if let Some(path) = find_label_temp(&hw, "Package id 0") {
                        return Some(TempSource::Hwmon(path));
                    }
                    // Fallback to temp1_input (usually Package)
                    let t1 = hw.join("temp1_input");
                    if t1.exists() {
                        return Some(TempSource::Hwmon(t1));
                    }
                }
                // AMD: k10temp — look for "Tctl" or "Tdie"
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
                // Apple Silicon: macsmc-hwmon
                "macsmc" => {
                    // Prefer SoC/heatpipe temp
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

    // 2. Fallback: thermal zones — prefer x86_pkg_temp
    if let Ok(entries) = std::fs::read_dir("/sys/class/thermal") {
        let mut zones: Vec<_> = entries.filter_map(Result::ok).collect();
        zones.sort_by_key(|e| e.file_name());
        // First pass: x86_pkg_temp
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
        // Second pass: any thermal_zone
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

fn read_temp(source: &TempSource) -> Option<u32> {
    let raw = match source {
        TempSource::Hwmon(p) | TempSource::ThermalZone(p) => {
            std::fs::read_to_string(p).ok()?
        }
    };
    let millideg: i64 = raw.trim().parse().ok()?;
    Some((millideg / 1000) as u32)
}

// ── timerfd + epoll monitor ────────────────────────────────────────────

fn temp_monitor(tx: async_channel::Sender<u32>) {
    let source = match detect_temp_source() {
        Some(s) => s,
        None => {
            log::warn!("cpu_temp: no temperature source found");
            return;
        }
    };

    let desc = match &source {
        TempSource::Hwmon(p) => format!("hwmon {}", p.display()),
        TempSource::ThermalZone(p) => format!("thermal {}", p.display()),
    };
    log::info!("cpu_temp: using {desc}");

    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 { return; }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 2, tv_nsec: 0 },
        it_value: libc::timespec { tv_sec: 0, tv_nsec: 1 },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 { return; }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: 0 };
    unsafe { libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, tfd.as_raw_fd(), &mut ev) };

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted { continue; }
            break;
        }
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        if let Some(temp) = read_temp(&source) {
            if tx.try_send(temp).is_err() && tx.is_closed() { break; }
        }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct CpuTemp {
    temp: u32,
    grouped: bool,
}

impl BarWidget for CpuTemp {
    const NAME: &str = "cpu-temp";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<u32>(1);

        std::thread::Builder::new()
            .name("cpu-temp".into())
            .spawn(move || temp_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(temp) = rx.recv().await {
                if this.update(cx, |this, cx| { this.temp = temp; cx.notify(); }).is_err() {
                    break;
                }
            }
        }).detach();

        Self { temp: 0, grouped: false }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let icon_size = crate::config::ICON_SIZE();

        let color = if self.temp >= 88 {
            t.red
        } else if self.temp >= 75 {
            t.orange
        } else if self.temp >= 62 {
            t.yellow
        } else {
            t.fg
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .gap(px(4.0))
                .text_xs()
                .child(
                    svg()
                        .external_path(ICON_THERMO.to_string())
                        .size(px(icon_size))
                        .text_color(rgb(color))
                        .flex_shrink_0(),
                )
                .child(div().text_color(rgb(color)).child(format!("{}°C", self.temp))),
            self.grouped,
        )
    }
}

impl_render!(CpuTemp);
