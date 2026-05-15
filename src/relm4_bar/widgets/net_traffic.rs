//! Network-traffic widget. Subscribes to `hub::net_traffic` and renders the
//! aggregate download / upload rate as `↓ <rate>  ↑ <rate>` in one capsule.
//! Each direction's label dims while that direction is idle.

/// Format a bytes-per-second rate, base-1024: `KB/s` with no decimals,
/// `MB/s` / `GB/s` with one. Zero renders as `0 KB/s`.
fn format_rate(bps: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;

    if bps < MB {
        format!("{:.0} KB/s", bps / KB)
    } else if bps < GB {
        format!("{:.1} MB/s", bps / MB)
    } else {
        format!("{:.1} GB/s", bps / GB)
    }
}

#[cfg(test)]
mod tests {
    use super::format_rate;

    #[test]
    fn zero_renders_as_kb() {
        assert_eq!(format_rate(0.0), "0 KB/s");
    }

    #[test]
    fn kb_range_has_no_decimals() {
        assert_eq!(format_rate(856.0 * 1024.0), "856 KB/s");
    }

    #[test]
    fn mb_range_has_one_decimal() {
        assert_eq!(format_rate(3.4 * 1024.0 * 1024.0), "3.4 MB/s");
    }

    #[test]
    fn gb_range_has_one_decimal() {
        assert_eq!(format_rate(1.2 * 1024.0 * 1024.0 * 1024.0), "1.2 GB/s");
    }

    #[test]
    fn crosses_kb_to_mb_at_one_mb() {
        assert_eq!(format_rate(1024.0 * 1024.0 - 1.0), "1024 KB/s");
        assert_eq!(format_rate(1024.0 * 1024.0), "1.0 MB/s");
    }
}
