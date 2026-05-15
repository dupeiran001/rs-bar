# Network Traffic Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a network-traffic widget to `relm4_bar` that shows live download and upload bandwidth, placed after the `Power` widget in both config profiles.

**Architecture:** A singleton `hub::net_traffic` background thread samples `/proc/net/dev` once a second, computes per-second byte deltas across physical interfaces, and publishes a `NetTrafficSample` over a `tokio::sync::watch` channel. A `NetTraffic` relm4 widget subscribes and renders `↓ <rate>  ↑ <rate>` in one capsule. This mirrors the existing `cpu_usage` / `power_draw` hub+widget pattern.

**Tech Stack:** Rust 2024, relm4 / GTK4, `tokio::sync::watch`, libc timerfd via the existing `hub::sys` helpers.

**Spec:** `docs/superpowers/specs/2026-05-15-network-traffic-widget-design.md`

---

## File Structure

| File | Status | Responsibility |
|------|--------|----------------|
| `src/relm4_bar/hub/net_traffic.rs` | create | Poller thread: parse `/proc/net/dev`, compute byte-rates, publish `NetTrafficSample`. |
| `src/relm4_bar/hub/mod.rs` | modify | Register `pub mod net_traffic;`. |
| `src/relm4_bar/widgets/net_traffic.rs` | create | `NetTraffic` widget: subscribe to the hub, render two rate labels. |
| `src/relm4_bar/widgets/mod.rs` | modify | Register `mod net_traffic;` + `pub use net_traffic::NetTraffic;`. |
| `assets/default-theme.css` | modify | `/* Net traffic */` color + tabular-numeral rules. |
| `src/relm4_bar/config/macbook.rs` | modify | Add `NetTraffic` after `Power` in the `right` zone. |
| `src/relm4_bar/config/intel.rs` | modify | Add `NetTraffic` after `Power` in the `right` zone. |

**Notes for the engineer:**
- `relm4_bar` is the GTK4 bar, run with `cargo run -- --relm` (the default binary runs the GPUI bar). This plan does not touch the GPUI bar.
- The crate has no pre-existing tests; Tasks 1 and 3 add the first ones. The first `cargo test` recompiles the crate in test mode and may take a couple of minutes.
- The crate builds with warnings (26 pre-existing). "Build succeeds" means `cargo build` exits 0 — warnings are fine. Intermediate tasks may briefly add an unused-code warning that a later task resolves.

---

### Task 1: net_traffic hub — `/proc/net/dev` parser (TDD)

**Files:**
- Create: `src/relm4_bar/hub/net_traffic.rs`
- Modify: `src/relm4_bar/hub/mod.rs`

- [ ] **Step 1: Register the module**

In `src/relm4_bar/hub/mod.rs`, add `pub mod net_traffic;` between `memory` and `niri`:

```rust
pub mod memory;
pub mod net_traffic;
pub mod niri;
```

- [ ] **Step 2: Create the file with the failing test only**

Create `src/relm4_bar/hub/net_traffic.rs` with exactly this content:

```rust
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
```

- [ ] **Step 3: Run the test, verify it fails**

Run: `cargo test hub::net_traffic`
Expected: compile error — `cannot find function 'parse_net_dev' in this scope` (`error[E0425]`).

- [ ] **Step 4: Implement the parser**

In `src/relm4_bar/hub/net_traffic.rs`, insert this function directly above the `#[cfg(test)]` line:

```rust
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
```

- [ ] **Step 5: Run the test, verify it passes**

Run: `cargo test hub::net_traffic`
Expected: PASS — `test result: ok. 2 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/relm4_bar/hub/net_traffic.rs src/relm4_bar/hub/mod.rs
git commit -m "feat: add net_traffic hub /proc/net/dev parser" \
  -m "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: net_traffic hub — poller thread

**Files:**
- Modify: `src/relm4_bar/hub/net_traffic.rs`

- [ ] **Step 1: Replace the file with the complete hub**

Replace the entire contents of `src/relm4_bar/hub/net_traffic.rs` with:

```rust
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
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: build succeeds (exit 0). A transient `subscribe` / `NetTrafficSample` unused warning is acceptable — Task 4 consumes them.

- [ ] **Step 3: Re-run the parser tests**

Run: `cargo test hub::net_traffic`
Expected: PASS — `test result: ok. 2 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/relm4_bar/hub/net_traffic.rs
git commit -m "feat: add net_traffic hub poller thread" \
  -m "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: net_traffic widget — rate formatter (TDD)

**Files:**
- Create: `src/relm4_bar/widgets/net_traffic.rs`
- Modify: `src/relm4_bar/widgets/mod.rs`

- [ ] **Step 1: Register the module**

In `src/relm4_bar/widgets/mod.rs`, add `mod net_traffic;` between `minimap` and `notch` (in the `mod` declaration block):

```rust
mod minimap;
mod net_traffic;
mod notch;
```

Do **not** add a `pub use` line yet — `NetTraffic` does not exist until Task 4.

- [ ] **Step 2: Create the file with the failing test only**

Create `src/relm4_bar/widgets/net_traffic.rs` with exactly this content:

```rust
//! Network-traffic widget. Subscribes to `hub::net_traffic` and renders the
//! aggregate download / upload rate as `↓ <rate>  ↑ <rate>` in one capsule.
//! Each direction's label dims while that direction is idle.

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
```

- [ ] **Step 3: Run the test, verify it fails**

Run: `cargo test widgets::net_traffic`
Expected: compile error — `cannot find function 'format_rate' in this scope` (`error[E0425]`).

- [ ] **Step 4: Implement the formatter**

In `src/relm4_bar/widgets/net_traffic.rs`, insert this function directly above the `#[cfg(test)]` line:

```rust
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
```

- [ ] **Step 5: Run the test, verify it passes**

Run: `cargo test widgets::net_traffic`
Expected: PASS — `test result: ok. 5 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/relm4_bar/widgets/net_traffic.rs src/relm4_bar/widgets/mod.rs
git commit -m "feat: add net_traffic rate formatter" \
  -m "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: net_traffic widget — component + styling

**Files:**
- Modify: `src/relm4_bar/widgets/net_traffic.rs`
- Modify: `src/relm4_bar/widgets/mod.rs`
- Modify: `assets/default-theme.css`

- [ ] **Step 1: Replace the widget file with the complete widget**

Replace the entire contents of `src/relm4_bar/widgets/net_traffic.rs` with:

```rust
//! Network-traffic widget. Subscribes to `hub::net_traffic` and renders the
//! aggregate download / upload rate as `↓ <rate>  ↑ <rate>` in one capsule.
//! Each direction's label dims while that direction is idle.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;
use crate::relm4_bar::hub::net_traffic::NetTrafficSample;

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

/// Dim (idle) / normal color classes, toggled per direction by `update`.
const COLOR_CLASSES: &[&str] = &["net-traffic-norm", "net-traffic-dim"];

pub struct NetTraffic {
    /// Last-displayed label strings, for the coalescing check in `update`.
    down: String,
    up: String,
    /// Held so `update` can rewrite + recolor them.
    down_label: gtk::Label,
    up_label: gtk::Label,
}

#[derive(Debug)]
pub enum NetTrafficMsg {
    Update(NetTrafficSample),
}

#[relm4::component(pub)]
impl SimpleComponent for NetTraffic {
    type Init = WidgetInit;
    type Input = NetTrafficMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 6,
            set_valign: gtk::Align::Center,
            #[name = "down_label"]
            gtk::Label {
                set_label: "↓ 0 KB/s",
                add_css_class: "net-traffic",
            },
            #[name = "up_label"]
            gtk::Label {
                set_label: "↑ 0 KB/s",
                add_css_class: "net-traffic",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = NetTraffic {
            down: String::new(),
            up: String::new(),
            down_label: widgets.down_label.clone(),
            up_label: widgets.up_label.clone(),
        };

        capsule(&root, init.grouped);

        crate::subscribe_into_msg!(
            hub::net_traffic::subscribe(),
            sender,
            NetTrafficMsg::Update
        );

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            NetTrafficMsg::Update(sample) => {
                let rx = format_rate(sample.rx_bps);
                let tx = format_rate(sample.tx_bps);
                let down = format!("↓ {rx}");
                let up = format!("↑ {tx}");

                // Coalescing: skip the GTK writes when nothing visible changed.
                if down == self.down && up == self.up {
                    return;
                }

                if down != self.down {
                    self.down = down;
                    self.down_label.set_label(&self.down);
                    set_exclusive_class(&self.down_label, rate_class(&rx), COLOR_CLASSES);
                }
                if up != self.up {
                    self.up = up;
                    self.up_label.set_label(&self.up);
                    set_exclusive_class(&self.up_label, rate_class(&tx), COLOR_CLASSES);
                }
            }
        }
    }
}

impl NamedWidget for NetTraffic {
    const NAME: &'static str = "net-traffic";
}

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

/// `net-traffic-dim` when the formatted rate is idle (`0 KB/s`), else
/// `net-traffic-norm`.
fn rate_class(rate: &str) -> &'static str {
    if rate == "0 KB/s" {
        "net-traffic-dim"
    } else {
        "net-traffic-norm"
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
```

- [ ] **Step 2: Export the widget**

In `src/relm4_bar/widgets/mod.rs`, add the `pub use` between `minimap` and `notch` (in the `pub use` block):

```rust
pub use minimap::Minimap;
pub use net_traffic::NetTraffic;
pub use notch::Notch;
```

- [ ] **Step 3: Add the theme rules**

In `assets/default-theme.css`, find the `/* Window title */` block:

```css
/* Window title */
.window-title { color: @rs_fg; }
```

Replace it with:

```css
/* Window title */
.window-title { color: @rs_fg; }

/* Net traffic */
.net-traffic      { font-feature-settings: "tnum"; }
.net-traffic-norm { color: @rs_fg;      }
.net-traffic-dim  { color: @rs_fg_dark; }
```

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: build succeeds (exit 0). `NetTraffic` is exported but not yet placed in a config — a transient unused warning is acceptable; Task 5 resolves it.

- [ ] **Step 5: Commit**

```bash
git add src/relm4_bar/widgets/net_traffic.rs src/relm4_bar/widgets/mod.rs assets/default-theme.css
git commit -m "feat: add NetTraffic widget" \
  -m "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Place NetTraffic in both bar profiles

**Files:**
- Modify: `src/relm4_bar/config/macbook.rs`
- Modify: `src/relm4_bar/config/intel.rs`

- [ ] **Step 1: macbook — import**

In `src/relm4_bar/config/macbook.rs`, in the `use crate::relm4_bar::widgets::{ … }` list, change:

```rust
    CpuTemp, CpuUsage, Fcitx, Memory, Minimap, Notch, PkgUpdate, Power, PsysDraw, Tray, Volume,
```

to:

```rust
    CpuTemp, CpuUsage, Fcitx, Memory, Minimap, NetTraffic, Notch, PkgUpdate, Power, PsysDraw, Tray, Volume,
```

- [ ] **Step 2: macbook — right zone**

In the same file's `bar()` function, change:

```rust
        right: widgets!(
            Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
```

to:

```rust
        right: widgets!(
            Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power, NetTraffic
        ),
```

- [ ] **Step 3: intel — import**

In `src/relm4_bar/config/intel.rs`, in the `use crate::relm4_bar::widgets::{ … }` list, change:

```rust
    Fcitx, GpuBusy, GpuDraw, Memory, Minimap, PkgUpdate, Power, Tray, Volume, Wifi, WindowTitle,
```

to:

```rust
    Fcitx, GpuBusy, GpuDraw, Memory, Minimap, NetTraffic, PkgUpdate, Power, Tray, Volume, Wifi, WindowTitle,
```

- [ ] **Step 4: intel — right zone**

In the same file's `bar()` function, change:

```rust
        right: widgets!(Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power),
```

to:

```rust
        right: widgets!(Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power, NetTraffic),
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: build succeeds (exit 0), with no new `net_traffic` / `NetTraffic` warnings — every piece is now wired up.

- [ ] **Step 6: Commit**

```bash
git add src/relm4_bar/config/macbook.rs src/relm4_bar/config/intel.rs
git commit -m "feat: show NetTraffic after Power in both bar profiles" \
  -m "Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Final verification

**Files:** none (verification only — no commit).

- [ ] **Step 1: Release build**

Run: `cargo build --release`
Expected: build succeeds (exit 0).

- [ ] **Step 2: Full test run**

Run: `cargo test net_traffic`
Expected: PASS — `test result: ok. 7 passed` (2 hub + 5 widget tests).

- [ ] **Step 3: Manual check (needs a running niri / Wayland session)**

Run the relm4 bar on the macbook profile:

```bash
RS_BAR_LOG=info cargo run --release -- --relm
```

Confirm:
- A `net_traffic: N physical interface(s): [...]` line appears in the log, listing your wifi/ethernet interface(s).
- A new capsule appears at the far right of the bar, immediately after the circular Power button, showing `↓ <rate>  ↑ <rate>`.
- Generating traffic (e.g. a download) moves the numbers; both labels brighten from dim when active and dim again when idle.

Then repeat on the intel profile:

```bash
RS_BAR_LOG=info cargo run --release -- --relm --config intel
```

Confirm the same capsule appears after the Power button.

If no Wayland/niri session is available, Steps 1–2 are the hard gate; hand the visual check in Step 3 to the user.

---

## Self-Review

**Spec coverage:**
- Hub `hub::net_traffic` + `NetTrafficSample` → Tasks 1–2. ✓
- `/proc/net/dev` parse, physical-interface filter, per-interface prev map, 1 s cadence, `Instant` dt → Task 2. ✓
- `NetTraffic` widget, single capsule, two direction labels, coalescing, `NamedWidget` → Task 4. ✓
- `format_rate` base-1024 rules, `0 KB/s` idle → Tasks 3–4. ✓
- Dim/normal per-direction color, tabular numerals, CSS block → Task 4. ✓
- Config placement after `Power` in both profiles → Task 5. ✓
- Error handling (unreadable file → `0/0`, `saturating_sub`, appear/vanish handling, no startup spike) → Task 2 code + comments. ✓
- Tests for `format_rate` + `parse_net_dev` → Tasks 1, 3. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code; every command shows expected output. ✓

**Type consistency:** `NetTrafficSample { rx_bps: f64, tx_bps: f64 }` defined in Task 2, consumed unchanged in Task 4. `NetTrafficMsg::Update(NetTrafficSample)` matches the `subscribe_into_msg!` contract (tuple-enum constructor). `parse_net_dev`, `is_physical`, `format_rate`, `rate_class` signatures are identical everywhere they appear. `NetTrafficSample` derives `Debug` so the `#[derive(Debug)]` `NetTrafficMsg` compiles. ✓
