//! CPU frequency + GPU busy % widget.
//!
//! Detects hybrid P/E core topology (Intel core_type sysfs or max_freq split).
//! Displays:
//!   - Hybrid: "P: X.XX GHz | E: X.XX GHz"
//!   - Uniform: "X.XX GHz"
//!   - GPU suffix: "| Gfx X%" (AMD gpu_busy_percent or Intel gt_busy_percent)
//!
//! Reads every second via `timerfd` + `epoll`. All sysfs, zero subprocesses.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

const ICON_FREQ: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/cpu-freq.svg");

// ── topology detection (once at startup) ───────────────────────────────

#[derive(Clone)]
enum CoreLayout {
    Uniform { cpus: Vec<u32> },
    Hybrid { p_cpus: Vec<u32>, e_cpus: Vec<u32> },
}

struct GpuBusy {
    path: PathBuf,
    label: String,
}

struct FreqSources {
    layout: CoreLayout,
    gpu: Option<GpuBusy>,
}

fn detect_freq_sources() -> FreqSources {
    let mut p_cpus = Vec::new();
    let mut e_cpus = Vec::new();
    let has_core_type =
        std::path::Path::new("/sys/devices/system/cpu/cpu0/topology/core_type").exists();

    if has_core_type {
        // Intel hybrid: core_type 1=P, 0=E
        for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            if !name.starts_with("cpu") {
                continue;
            }
            let num: u32 = match name.strip_prefix("cpu").and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            let ct = std::fs::read_to_string(entry.path().join("topology/core_type"))
                .unwrap_or_default();
            match ct.trim() {
                "1" => p_cpus.push(num),
                "0" => e_cpus.push(num),
                _ => p_cpus.push(num),
            }
        }
    } else {
        // Check for hybrid via max_freq split (≥1.2x ratio)
        let mut all_cpus: Vec<(u32, u64)> = Vec::new();
        for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten() {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            let num: u32 = match name.strip_prefix("cpu").and_then(|s| s.parse().ok()) {
                Some(n) => n,
                None => continue,
            };
            let max_freq = std::fs::read_to_string(
                entry.path().join("cpufreq/cpuinfo_max_freq"),
            )
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
            if max_freq > 0 {
                all_cpus.push((num, max_freq));
            }
        }

        let mut unique_freqs: Vec<u64> = all_cpus.iter().map(|(_, f)| *f).collect();
        unique_freqs.sort_unstable();
        unique_freqs.dedup();

        if unique_freqs.len() == 2
            && unique_freqs[0] > 0
            && unique_freqs[1] * 100 / unique_freqs[0] >= 120
        {
            let p_max = unique_freqs[1];
            for (num, freq) in &all_cpus {
                if *freq == p_max {
                    p_cpus.push(*num);
                } else {
                    e_cpus.push(*num);
                }
            }
        } else {
            for (num, _) in &all_cpus {
                p_cpus.push(*num);
            }
        }
    }

    p_cpus.sort_unstable();
    e_cpus.sort_unstable();

    let layout = if e_cpus.is_empty() {
        CoreLayout::Uniform { cpus: p_cpus }
    } else {
        CoreLayout::Hybrid { p_cpus, e_cpus }
    };

    // GPU busy detection
    let gpu = detect_gpu_busy();

    let desc = match &layout {
        CoreLayout::Uniform { cpus } => format!("uniform {} cores", cpus.len()),
        CoreLayout::Hybrid { p_cpus, e_cpus } => {
            format!("hybrid {}P+{}E cores", p_cpus.len(), e_cpus.len())
        }
    };
    log::info!(
        "cpu_freq: {desc}, gpu={}",
        gpu.as_ref().map_or("none".into(), |g| g.label.clone())
    );

    FreqSources { layout, gpu }
}

fn detect_gpu_busy() -> Option<GpuBusy> {
    for entry in std::fs::read_dir("/sys/class/drm").ok()?.filter_map(Result::ok) {
        let dev = entry.path().join("device");
        if !dev.is_dir() {
            continue;
        }
        let vendor = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
        let class = std::fs::read_to_string(dev.join("class")).unwrap_or_default();
        if !class.trim().starts_with("0x03") {
            continue;
        }

        let (label, files) = match vendor.trim() {
            "0x1002" => ("Gfx", vec!["gpu_busy_percent"]),
            "0x8086" => ("iGPU", vec!["gpu_busy_percent", "gt_busy_percent"]),
            _ => continue,
        };

        for f in files {
            let path = dev.join(f);
            if path.exists() && std::fs::read_to_string(&path).is_ok() {
                return Some(GpuBusy {
                    path,
                    label: label.to_string(),
                });
            }
        }
    }
    None
}

// ── reading ────────────────────────────────────────────────────────────

fn read_avg_freq_khz(cpus: &[u32]) -> u64 {
    if cpus.is_empty() {
        return 0;
    }
    let mut total: u64 = 0;
    let mut count: u64 = 0;
    for &num in cpus {
        let path = format!("/sys/devices/system/cpu/cpu{num}/cpufreq/scaling_cur_freq");
        if let Some(khz) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            total += khz;
            count += 1;
        }
    }
    if count > 0 { total / count } else { 0 }
}

fn read_gpu_busy(gpu: &GpuBusy) -> Option<u32> {
    std::fs::read_to_string(&gpu.path)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn khz_to_ghz_str(khz: u64) -> String {
    format!("{}.{:02}", khz / 1_000_000, (khz % 1_000_000) / 10_000)
}

// ── state ──────────────────────────────────────────────────────────────

const ICON_GPU_BUSY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/gpu-busy.svg");

#[derive(Clone)]
struct FreqReading {
    text: String,
    gpu_pct: Option<u32>,
}

fn take_reading(src: &FreqSources) -> FreqReading {
    let text = match &src.layout {
        CoreLayout::Uniform { cpus } => {
            let avg = read_avg_freq_khz(cpus);
            format!("{} GHz", khz_to_ghz_str(avg))
        }
        CoreLayout::Hybrid { p_cpus, e_cpus } => {
            let p = read_avg_freq_khz(p_cpus);
            let e = read_avg_freq_khz(e_cpus);
            format!("P:{} | E:{}", khz_to_ghz_str(p), khz_to_ghz_str(e))
        }
    };

    let gpu_pct = src
        .gpu
        .as_ref()
        .and_then(|g| read_gpu_busy(g));

    FreqReading { text, gpu_pct }
}

// ── timerfd + epoll monitor ────────────────────────────────────────────

fn freq_monitor(tx: async_channel::Sender<FreqReading>) {
    let src = detect_freq_sources();

    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 { return; }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: 1, tv_nsec: 0 },
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

        let reading = take_reading(&src);
        if tx.try_send(reading).is_err() && tx.is_closed() { break; }
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct CpuFreq {
    reading: FreqReading,
    grouped: bool,
}

impl BarWidget for CpuFreq {
    const NAME: &str = "cpu-freq";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<FreqReading>(1);

        std::thread::Builder::new()
            .name("cpu-freq".into())
            .spawn(move || freq_monitor(tx))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(reading) = rx.recv().await {
                if this.update(cx, |this, cx| { this.reading = reading; cx.notify(); }).is_err() {
                    break;
                }
            }
        }).detach();

        Self {
            reading: FreqReading {
                text: String::new(),
                gpu_pct: None,
            },
            grouped: false,
        }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let icon_size = crate::config::ICON_SIZE;
        let r = &self.reading;

        let sep = || div().text_color(rgb(t.border)).child("│");

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .external_path(ICON_FREQ.to_string())
                    .size(px(icon_size))
                    .text_color(rgb(t.fg))
                    .flex_shrink_0(),
            )
            .child(div().text_color(rgb(t.fg)).child(r.text.clone()));

        if let Some(pct) = r.gpu_pct {
            row = row
                .child(sep())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(
                            svg()
                                .external_path(ICON_GPU_BUSY.to_string())
                                .size(px(icon_size))
                                .text_color(rgb(t.text_dim))
                                .flex_shrink_0(),
                        )
                        .child(div().text_color(rgb(t.text_dim)).child(format!("{pct}%"))),
                );
        }

        super::capsule(
            div()
                .flex()
                .items_center()
                .px(px(4.0))
                .text_xs()
                .child(row),
            self.grouped,
        )
    }
}

impl_render!(CpuFreq);
