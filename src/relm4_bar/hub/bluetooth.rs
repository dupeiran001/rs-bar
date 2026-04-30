//! Bluetooth hub. Tracks adapter power and paired/connected devices via the
//! `bluetoothctl` command-line tool.
//!
//! Architecture:
//!   - One singleton background thread (`"bluetooth"`) spawns a long-lived
//!     `bluetoothctl` child process. Its stdout is parsed for `[CHG]` event
//!     lines, which trigger a refresh of the device list.
//!   - Power state is sourced from `/sys/class/rfkill/*` (cheap, kernel-side
//!     truth) on every refresh.
//!   - The device list is rebuilt by running `bluetoothctl devices Paired` and
//!     `bluetoothctl devices Connected`; the intersection determines per-device
//!     connection state.
//!   - The thread is also notified by a small `async-channel` whenever a
//!     command (`power_on`, `connect`, …) is issued so the snapshot is
//!     refreshed promptly after user actions.
//!
//! Subscribers receive `BluetoothState` via `tokio::sync::watch`.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BluetoothState {
    /// Adapter power state from rfkill.
    pub powered: bool,
    /// Devices currently connected.
    pub connected_devices: Vec<DeviceInfo>,
    /// All paired devices, including the connected ones.
    pub paired_devices: Vec<DeviceInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub mac: String,
    pub name: String,
    pub connected: bool,
    pub paired: bool,
}

// ── refresh-trigger channel ────────────────────────────────────────────

/// Singleton channel pair used to wake the worker thread on demand. Both the
/// internal stdout reader (parsing `[CHG]` lines) and the public command API
/// publish to the sender; the worker awaits on the receiver.
fn refresh_chan() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static S: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> = OnceLock::new();
    S.get_or_init(|| async_channel::bounded(4))
}

fn nudge_refresh() {
    let _ = refresh_chan().0.try_send(());
}

// ── rfkill / sysfs ─────────────────────────────────────────────────────

fn is_bt_powered() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/rfkill") else {
        return false;
    };
    for entry in dir.filter_map(Result::ok) {
        let p = entry.path();
        let ty = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        if ty.trim() != "bluetooth" {
            continue;
        }
        let state = std::fs::read_to_string(p.join("state")).unwrap_or_default();
        let hard = std::fs::read_to_string(p.join("hard")).unwrap_or_default();
        return state.trim() == "1" && hard.trim() != "1";
    }
    false
}

// ── bluetoothctl helpers ───────────────────────────────────────────────

/// Parse `bluetoothctl devices [filter]` output into a list of (mac, name).
/// Each line has the form `Device AA:BB:CC:DD:EE:FF Name With Spaces`.
fn parse_devices(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("Device ")?;
            let mut parts = rest.splitn(2, ' ');
            let mac = parts.next()?.to_string();
            let name = parts.next().unwrap_or("").to_string();
            Some((mac, name))
        })
        .collect()
}

fn run_bluetoothctl(args: &[&str]) -> Option<String> {
    let out = Command::new("bluetoothctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn query_paired() -> Vec<(String, String)> {
    run_bluetoothctl(&["devices", "Paired"])
        .map(|s| parse_devices(&s))
        .unwrap_or_default()
}

fn query_connected() -> Vec<(String, String)> {
    run_bluetoothctl(&["devices", "Connected"])
        .map(|s| parse_devices(&s))
        .unwrap_or_default()
}

fn snapshot() -> BluetoothState {
    let powered = is_bt_powered();
    if !powered {
        return BluetoothState {
            powered: false,
            connected_devices: Vec::new(),
            paired_devices: Vec::new(),
        };
    }

    let paired_raw = query_paired();
    let connected_raw = query_connected();

    let connected_macs: std::collections::HashSet<String> =
        connected_raw.iter().map(|(m, _)| m.clone()).collect();

    let connected_devices: Vec<DeviceInfo> = connected_raw
        .into_iter()
        .map(|(mac, name)| DeviceInfo {
            mac,
            name,
            connected: true,
            paired: true,
        })
        .collect();

    // Build the paired list, marking which are connected. If a device shows up
    // only in Connected (rare — adapter quirk), include it too.
    let mut paired_devices: Vec<DeviceInfo> = paired_raw
        .into_iter()
        .map(|(mac, name)| {
            let connected = connected_macs.contains(&mac);
            DeviceInfo {
                mac,
                name,
                connected,
                paired: true,
            }
        })
        .collect();

    let paired_macs: std::collections::HashSet<String> =
        paired_devices.iter().map(|d| d.mac.clone()).collect();
    for d in &connected_devices {
        if !paired_macs.contains(&d.mac) {
            paired_devices.push(d.clone());
        }
    }

    BluetoothState {
        powered: true,
        connected_devices,
        paired_devices,
    }
}

// ── event monitor: long-lived `bluetoothctl` child ─────────────────────

/// Spawn `bluetoothctl` with stdin kept open (so it doesn't exit on EOF) and
/// return its child + stdout reader. Caller is responsible for `wait()`ing.
fn spawn_monitor() -> Option<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("bluetoothctl");
    cmd.stdout(Stdio::piped())
        .stdin(Stdio::piped()) // keep pipe open so bluetoothctl doesn't exit on EOF
        .stderr(Stdio::null());
    // Kill this long-lived child when the parent dies instead of orphaning.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    cmd.spawn().ok()
}

fn monitor_thread(producer: watch::Sender<BluetoothState>) {
    use std::io::{BufRead, BufReader};

    // Push an initial snapshot before doing anything else so subscribers don't
    // have to wait for the first event.
    let _ = producer.send(snapshot());

    let refresh = refresh_chan().1.clone();
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        let Some(mut child) = spawn_monitor() else {
            log::warn!("bluetooth: bluetoothctl not found, retrying in {backoff:?}");
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(max_backoff);
            continue;
        };

        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            std::thread::sleep(backoff);
            continue;
        };

        log::info!("bluetooth: monitor started");
        let started = std::time::Instant::now();

        // Drive the reader on a helper thread; it pushes "refresh" tokens onto
        // the same async-channel used by the public command API. We then fold
        // both event sources into a single coalescing loop here.
        let trigger_tx = refresh_chan().0.clone();
        let reader_handle = std::thread::Builder::new()
            .name("bt-reader".into())
            .spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    // Any `[CHG]` line is a state change worth re-snapshotting.
                    // `Connected: yes/no`, `Powered: yes/no`, `Paired:`, etc.
                    if line.contains("[CHG]")
                        || line.contains("[NEW]")
                        || line.contains("[DEL]")
                    {
                        let _ = trigger_tx.try_send(());
                    }
                }
            });

        // Coalesce refresh tokens: drain the channel each iteration so a burst
        // of events produces a single snapshot read.
        loop {
            // Block on at least one token.
            if refresh.recv_blocking().is_err() {
                break;
            }
            // Drain any extras non-blockingly to coalesce.
            while refresh.try_recv().is_ok() {}

            let new = snapshot();
            // `send_if_modified` would also be fine; `send` is cheap when no
            // receivers are awaiting and the watch channel deduplicates via
            // marking only if the value differs.
            producer.send_if_modified(|cur| {
                if *cur != new {
                    *cur = new;
                    true
                } else {
                    false
                }
            });
        }

        // Reader exited (child died) or the channel closed. Clean up and back
        // off before respawning bluetoothctl.
        let _ = child.kill();
        let _ = child.wait();
        if let Ok(h) = reader_handle {
            let _ = h.join();
        }

        if started.elapsed() > Duration::from_secs(5) {
            backoff = Duration::from_secs(1);
        }
        log::debug!(
            "bluetooth: bluetoothctl exited after {:?}, retrying in {backoff:?}",
            started.elapsed()
        );
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(max_backoff);
    }
}

fn sender() -> &'static watch::Sender<BluetoothState> {
    static S: OnceLock<watch::Sender<BluetoothState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(BluetoothState::default());
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("bluetooth".into())
            .spawn(move || monitor_thread(producer))
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<BluetoothState> {
    sender().subscribe()
}

// ── command API ────────────────────────────────────────────────────────

/// Run a one-shot `bluetoothctl` command on a detached background thread so
/// the caller (often a GTK click handler) returns immediately.
fn fire_and_forget(args: Vec<String>) {
    std::thread::Builder::new()
        .name("bt-cmd".into())
        .spawn(move || {
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            let _ = Command::new("bluetoothctl")
                .args(&argv)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            // Nudge the monitor to re-snapshot now (don't wait for events).
            nudge_refresh();
        })
        .ok();
}

#[allow(dead_code)]
pub fn power_on() {
    fire_and_forget(vec!["power".into(), "on".into()]);
}

#[allow(dead_code)]
pub fn power_off() {
    fire_and_forget(vec!["power".into(), "off".into()]);
}

#[allow(dead_code)]
pub fn connect(mac: &str) {
    fire_and_forget(vec!["connect".into(), mac.to_string()]);
}

#[allow(dead_code)]
pub fn disconnect(mac: &str) {
    fire_and_forget(vec!["disconnect".into(), mac.to_string()]);
}
