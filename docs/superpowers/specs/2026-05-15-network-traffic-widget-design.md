# Network Traffic Widget — Design

**Date:** 2026-05-15
**Status:** Approved
**Target:** `relm4_bar` only — both the `macbook` and `intel` config profiles. `gpui_bar` is untouched.

## Goal

Add a bar widget showing current download and upload bandwidth. It sits in the
`right` zone immediately after the `Power` widget in both relm4_bar profiles.

## Architecture

Follows the established relm4_bar hub + widget split (as used by `cpu_usage`,
`memory`, `power_draw`):

- A singleton background OS thread (`hub::net_traffic`) samples `/proc/net/dev`
  and publishes a `NetTrafficSample` over a `tokio::sync::watch` channel.
- A `NetTraffic` widget subscribes and renders. Multi-monitor setups share the
  single poller.

Data flow:

```
"net-traffic" thread → read /proc/net/dev → delta vs prev → NetTrafficSample
    → watch::Sender → widget subscribe → component Update msg → GTK labels
```

## Component 1 — `src/relm4_bar/hub/net_traffic.rs` (new)

Published type:

```rust
#[derive(Clone, Copy, Default)]
pub struct NetTrafficSample {
    pub rx_bps: f64,  // download, bytes/sec
    pub tx_bps: f64,  // upload,   bytes/sec
}
```

- Singleton `OnceLock<watch::Sender<NetTrafficSample>>`; the first `subscribe()`
  spawns the `"net-traffic"` thread.
- Thread runs `timerfd_loop(Duration::from_secs(1), …)`.
- Each tick:
  1. Read `/proc/net/dev` via `std::fs::read_to_string`; on error, treat as
     empty (→ `0/0` sample).
  2. For each data line, parse the interface name and, after the `<iface>:`,
     the 1st field (receive bytes) and 9th field (transmit bytes). The two
     `Inter-|` / column-name header lines have no `:` and are skipped.
  3. Keep only **physical** interfaces — those for which
     `/sys/class/net/<iface>/device` exists. This excludes `lo`, `docker*`,
     `veth*`, bridges, `wg*`, `tun*`/`tap*`, and counts wifi + ethernet.
  4. Look up each interface's previous `(rx, tx)` in a
     `HashMap<String, (u64, u64)>`. Per-counter delta = `cur.saturating_sub(prev)`.
     An interface with no prev entry (just appeared) contributes 0 this tick.
  5. Sum the deltas across interfaces and divide by the elapsed time (an
     `Instant` delta, like `power_draw`) → `rx_bps`, `tx_bps`.
  6. Replace the prev map with the current readings; publish the sample.
- The first tick seeds the prev map and publishes `0.0/0.0` — no startup spike.
- `pub fn subscribe() -> watch::Receiver<NetTrafficSample>`.
- Registered as `pub mod net_traffic;` in `hub/mod.rs`.

## Component 2 — `src/relm4_bar/widgets/net_traffic.rs` (new)

A standard relm4 widget mirroring `cpu_usage` (the file documented as the
canonical widget pattern):

- `NetTraffic` model: `grouped: bool`, the last-displayed `rx`/`tx` strings
  (for coalescing), and the held `down_label` / `up_label` `gtk::Label`s.
- `view!`: a horizontal `gtk::Box` (single capsule) with two `gtk::Label`s —
  one rendering `↓ <rate>`, one rendering `↑ <rate>` — separated by spacing.
  The arrows are font glyphs in the label text (no new icon assets). `init`
  calls `capsule(&root, grouped)`.
- Subscribes via
  `subscribe_into_msg!(hub::net_traffic::subscribe(), sender, NetTrafficMsg::Update)`.
- `update`: format both rates; if both display strings are unchanged, return
  early (coalescing). Otherwise set the label text and apply the per-direction
  color class.
- `impl NamedWidget` with `const NAME = "net-traffic"`.
- Registered in `widgets/mod.rs` (`mod net_traffic;` + `pub use
  net_traffic::NetTraffic;`).

Rate formatting — `format_rate(bps: f64) -> String`, base-1024:

- `< 1024 KB/s` → `"<n> KB/s"` — 0 decimals.
- `< 1024 MB/s` → `"<n.n> MB/s"` — 1 decimal.
- otherwise → `"<n.n> GB/s"` — 1 decimal.
- `0.0` → `"0 KB/s"`.

## CSS — `assets/default-theme.css`

New block:

```css
/* Net traffic */
.net-traffic-norm { color: @rs_fg;      }
.net-traffic-dim  { color: @rs_fg_dark; }
```

Each direction's label uses `net-traffic-dim` when its rate displays as
`0 KB/s` and `net-traffic-norm` otherwise — class swap via `set_exclusive_class`,
the same helper `cpu_usage` uses. The labels also get
`font-feature-settings: "tnum"` (tabular numerals) so the pill width stays
steady as digits change.

## Config changes

Append `NetTraffic` after `Power` in the `right` zone of both profiles (and add
it to the `use` import list in each file, matching the other widgets):

- `src/relm4_bar/config/macbook.rs`:
  `right: widgets!(Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power, NetTraffic)`
- `src/relm4_bar/config/intel.rs`:
  `right: widgets!(Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power, NetTraffic)`

## Error handling

- `/proc/net/dev` unreadable → empty parse → `0/0` sample.
- A counter that decreases (driver reset / interface re-init) → `saturating_sub`
  clamps that interface's delta to 0 for that tick.
- Interface appears mid-run → no prev entry → skipped one tick, counted from the
  next.
- Interface disappears → simply absent from the current readings; its stale prev
  entry is dropped when the map is replaced.
- No physical interfaces at all → sample stays `0/0`; the widget shows `0 KB/s`.

## Testing

The repo currently has no test suite; this adds the first `#[cfg(test)]` tests,
covering the two pure functions:

- `format_rate`: boundaries — `0.0`, just under/over 1024 KB/s, the MB/s range,
  the GB/s range; verifies the decimal-place rules.
- the `/proc/net/dev` parser: a sample multi-interface buffer → expected
  per-interface `(rx, tx)`; confirms data lines parse and the header lines are
  ignored.

The physical-interface filter reads `/sys`, so it is exercised at runtime rather
than unit-tested.

## Out of scope

- No click popover and no per-interface breakdown — display only.
- No bits/sec (Mbps) mode — bytes/sec only.
- `gpui_bar` is untouched.
