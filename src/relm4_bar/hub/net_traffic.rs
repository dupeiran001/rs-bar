//! Network-traffic hub. Samples `/proc/net/dev` on a 1 s timerfd and publishes
//! aggregate download / upload throughput in bytes per second.
//!
//! Singleton background thread (`"net-traffic"`) shared across every bar
//! instance; subscribers receive the latest sample via `tokio::sync::watch`.
//!
//! Only *physical* interfaces are summed — an interface counts when
//! `/sys/class/net/<iface>/device` exists, which is true for real NICs
//! (ethernet, wifi, USB tethering) and false for `lo`, `docker*`, `veth*`,
//! bridges, `wg*`, and `tun*`/`tap*`. Per-interface previous counters are kept
//! in a map so an interface appearing or vanishing mid-run does not spike the
//! published rate.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::sys::timerfd_loop;

/// Aggregate network throughput across every physical interface.
#[derive(Clone, Copy, Default, Debug)]
pub struct NetTrafficSample {
    /// Download rate, bytes per second.
    pub rx_bps: f64,
    /// Upload rate, bytes per second.
    pub tx_bps: f64,
}

const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Parse `/proc/net/dev` into `(interface, (rx_bytes, tx_bytes))` entries.
///
/// Every data line is `<iface>: <rx_bytes> <rx_packets> … <tx_bytes> …` — the
/// receive byte count is the 1st field after the colon, transmit the 9th. The
/// two header lines have no `:` and are skipped.
fn parse_net_dev(content: &str) -> Vec<(String, (u64, u64))> {
    let mut out = Vec::new();
    for line in content.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if fields.len() >= 9 {
            out.push((name.trim().to_string(), (fields[0], fields[8])));
        }
    }
    out
}

/// True when `iface` is a physical device — real NICs expose a `device`
/// symlink in sysfs; `lo`, bridges, `veth*`, `docker*`, `wg*`, `tun*` do not.
fn is_physical(iface: &str) -> bool {
    Path::new(&format!("/sys/class/net/{iface}/device")).exists()
}

fn read_net_dev() -> String {
    std::fs::read_to_string("/proc/net/dev").unwrap_or_default()
}

fn sender() -> &'static watch::Sender<NetTrafficSample> {
    static S: OnceLock<watch::Sender<NetTrafficSample>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(NetTrafficSample::default());
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("net-traffic".into())
            .spawn(move || run_poller(producer))
            .ok();
        tx
    })
}

fn run_poller(producer: watch::Sender<NetTrafficSample>) {
    // Per-interface previous (rx, tx) byte counters, keyed by name so an
    // interface appearing or vanishing mid-run doesn't produce a spike.
    let mut prev: HashMap<String, (u64, u64)> = HashMap::new();

    // Seed the baseline so the first published sample is a true 0/0 rather
    // than every byte transferred since boot.
    for (name, counters) in parse_net_dev(&read_net_dev()) {
        if is_physical(&name) {
            prev.insert(name, counters);
        }
    }
    let ifaces: Vec<&str> = prev.keys().map(String::as_str).collect();
    log::info!(
        "net_traffic: {} physical interface(s): {:?}",
        ifaces.len(),
        ifaces,
    );
    let mut prev_time = Instant::now();

    timerfd_loop(POLL_INTERVAL, false, || {
        let now = Instant::now();
        let dt = now.duration_since(prev_time).as_secs_f64();
        prev_time = now;

        let mut rx_delta: u64 = 0;
        let mut tx_delta: u64 = 0;
        let mut cur: HashMap<String, (u64, u64)> = HashMap::new();

        for (name, (rx, tx)) in parse_net_dev(&read_net_dev()) {
            if !is_physical(&name) {
                continue;
            }
            // An interface with no prev entry (just appeared) is skipped this
            // tick and counted from the next.
            if let Some(&(prx, ptx)) = prev.get(&name) {
                rx_delta += rx.saturating_sub(prx);
                tx_delta += tx.saturating_sub(ptx);
            }
            cur.insert(name, (rx, tx));
        }
        prev = cur;

        let sample = if dt > 0.0 {
            NetTrafficSample {
                rx_bps: rx_delta as f64 / dt,
                tx_bps: tx_delta as f64 / dt,
            }
        } else {
            NetTrafficSample::default()
        };

        // Returning false would exit the loop; in practice the sender is held
        // by the OnceLock for the program's lifetime so this never happens.
        producer.send(sample).is_ok()
    });
}

pub fn subscribe() -> watch::Receiver<NetTrafficSample> {
    sender().subscribe()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_lines_and_skips_headers() {
        let sample = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  123456     789    0    0    0     0          0         0   123456     789    0    0    0     0       0          0
  eth0: 1000000    1234    0    0    0     0          0         0   500000     678    0    0    0     0       0          0
";
        let parsed = parse_net_dev(sample);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], ("lo".to_string(), (123456, 123456)));
        assert_eq!(parsed[1], ("eth0".to_string(), (1_000_000, 500_000)));
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_net_dev("").is_empty());
    }
}
