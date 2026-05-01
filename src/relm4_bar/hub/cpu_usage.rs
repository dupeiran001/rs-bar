//! CPU usage hub. /proc/stat reader on a 1-second timerfd, publishes %.
//!
//! Singleton background thread (`"cpu-usage"`) shared across every bar
//! instance. Subscribers receive the latest sample via `tokio::sync::watch`.

use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

/// Raw CPU time counters from `/proc/stat` (aggregate "cpu" line).
#[derive(Clone, Copy, Default)]
struct CpuTimes {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuTimes {
    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }
    fn idle_total(&self) -> u64 {
        self.idle + self.iowait
    }
}

fn read_cpu_times() -> CpuTimes {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    for line in stat.lines() {
        // The aggregate line starts with "cpu " (with trailing space).
        if let Some(rest) = line.strip_prefix("cpu ") {
            let vals: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            if vals.len() >= 8 {
                return CpuTimes {
                    user: vals[0],
                    nice: vals[1],
                    system: vals[2],
                    idle: vals[3],
                    iowait: vals[4],
                    irq: vals[5],
                    softirq: vals[6],
                    steal: vals[7],
                };
            }
        }
    }
    CpuTimes::default()
}

fn compute_usage(prev: &CpuTimes, cur: &CpuTimes) -> f32 {
    let dt = cur.total().saturating_sub(prev.total());
    if dt == 0 {
        return 0.0;
    }
    let di = cur.idle_total().saturating_sub(prev.idle_total());
    ((dt - di) as f32 / dt as f32) * 100.0
}

fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("cpu-usage".into())
            .spawn(move || {
                let mut prev = read_cpu_times();
                timerfd_loop(std::time::Duration::from_millis(500), false, || {
                    let cur = read_cpu_times();
                    let usage = compute_usage(&prev, &cur);
                    prev = cur;
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(usage).is_ok()
                });
            })
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<f32> {
    sender().subscribe()
}
