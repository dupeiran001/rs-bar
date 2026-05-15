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
