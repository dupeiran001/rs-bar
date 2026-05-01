//! CPU frequency hub. Reads per-core current frequency from
//! `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` once per second
//! and publishes a [`FreqReading`] containing both the formatted display
//! string and the average frequency in GHz.
//!
//! Detects hybrid P/E core topology at startup using
//! `/sys/devices/system/cpu/cpu*/topology/core_type` (Intel) and falls back
//! to splitting cores by `cpufreq/cpuinfo_max_freq` when core_type isn't
//! exposed. Uniform CPUs publish a single value; hybrid CPUs publish split
//! P/E values plus a weighted average for sparkline rendering.
//!
//! Singleton background thread (`"cpu-freq"`) shared across every bar
//! instance. Subscribers receive the latest sample via `tokio::sync::watch`.

use std::path::Path;
use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

/// How frequencies are presented to the renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum FreqDisplay {
    /// Uniform topology: a single `"X.XX GHz"` style string.
    Single(String),
    /// Hybrid topology: `(p_text, e_text)` rendered with a vertical separator.
    Split(String, String),
}

/// One frequency sample delivered to subscribers.
#[derive(Clone, Debug, PartialEq)]
pub struct FreqReading {
    pub display: FreqDisplay,
    /// Weighted average frequency across all cores, in GHz. Drives the sparkline.
    pub avg_ghz: f32,
}

impl Default for FreqReading {
    fn default() -> Self {
        Self {
            display: FreqDisplay::Single(String::new()),
            avg_ghz: 0.0,
        }
    }
}

#[derive(Clone)]
enum CoreLayout {
    Uniform { cpus: Vec<u32> },
    Hybrid { p_cpus: Vec<u32>, e_cpus: Vec<u32> },
}

fn detect_layout() -> CoreLayout {
    let mut p_cpus = Vec::new();
    let mut e_cpus = Vec::new();
    let has_core_type =
        Path::new("/sys/devices/system/cpu/cpu0/topology/core_type").exists();

    if has_core_type {
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
            let ct = std::fs::read_to_string(entry.path().join("topology/core_type"))
                .unwrap_or_default();
            match ct.trim() {
                "1" => p_cpus.push(num),
                "0" => e_cpus.push(num),
                _ => p_cpus.push(num),
            }
        }
    } else {
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

    if e_cpus.is_empty() {
        CoreLayout::Uniform { cpus: p_cpus }
    } else {
        CoreLayout::Hybrid { p_cpus, e_cpus }
    }
}

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

fn khz_to_ghz_str(khz: u64) -> String {
    format!("{}.{:02}", khz / 1_000_000, (khz % 1_000_000) / 10_000)
}

fn take_reading(layout: &CoreLayout) -> FreqReading {
    match layout {
        CoreLayout::Uniform { cpus } => {
            let khz = read_avg_freq_khz(cpus);
            FreqReading {
                display: FreqDisplay::Single(format!("{} GHz", khz_to_ghz_str(khz))),
                avg_ghz: khz as f32 / 1_000_000.0,
            }
        }
        CoreLayout::Hybrid { p_cpus, e_cpus } => {
            let p = read_avg_freq_khz(p_cpus);
            let e = read_avg_freq_khz(e_cpus);
            // Weighted average across P + E cores (true "average core frequency").
            let total_cores = (p_cpus.len() + e_cpus.len()) as u64;
            let avg_khz = if total_cores > 0 {
                (p * p_cpus.len() as u64 + e * e_cpus.len() as u64) / total_cores
            } else {
                0
            };
            FreqReading {
                display: FreqDisplay::Split(
                    format!("P:{}", khz_to_ghz_str(p)),
                    format!("E:{}", khz_to_ghz_str(e)),
                ),
                avg_ghz: avg_khz as f32 / 1_000_000.0,
            }
        }
    }
}

/// Detect the min/max scaling frequency of cpu0 to use as a fixed scale for
/// the sparkline. Falls back to a sensible default if sysfs isn't readable.
#[allow(dead_code)]
pub fn detect_freq_range_ghz() -> (f32, f32) {
    let read_ghz = |path: &str| -> Option<f32> {
        std::fs::read_to_string(path)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .map(|khz| khz as f32 / 1_000_000.0)
    };
    let min = read_ghz("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_min_freq").unwrap_or(0.4);
    let max = read_ghz("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq").unwrap_or(5.0);
    (min, max)
}

fn sender() -> &'static watch::Sender<FreqReading> {
    static S: OnceLock<watch::Sender<FreqReading>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(FreqReading::default());
        let producer = tx.clone();
        let layout = detect_layout();
        let desc = match &layout {
            CoreLayout::Uniform { cpus } => format!("uniform {} cores", cpus.len()),
            CoreLayout::Hybrid { p_cpus, e_cpus } => {
                format!("hybrid {}P+{}E cores", p_cpus.len(), e_cpus.len())
            }
        };
        log::info!("cpu_freq: {desc}");
        std::thread::Builder::new()
            .name("cpu-freq".into())
            .spawn(move || {
                timerfd_loop(std::time::Duration::from_millis(500), true, || {
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(take_reading(&layout)).is_ok()
                });
            })
            .ok();
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<FreqReading> {
    sender().subscribe()
}
