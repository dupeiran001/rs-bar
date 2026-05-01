//! Volume / audio-sink hub.
//!
//! Architecture: a singleton background OS thread (`"volume"`) spawns
//! `pactl subscribe` and re-queries the full audio state via `pactl -f json
//! list sinks` and `wpctl get-volume @DEFAULT_AUDIO_SINK@` whenever a sink or
//! server event is observed. Subscribers receive a [`VolumeState`] snapshot
//! through [`tokio::sync::watch`].
//!
//! Conforms to the canonical hub pattern: `OnceLock<watch::Sender<_>>`, a
//! single named `std::thread`, lazy spawn on first `subscribe()`. Adds a
//! command API ([`set_volume`], [`toggle_mute`], [`set_default_sink`]) that
//! shells out to `wpctl` / `pactl`; the resulting PipeWire event causes the
//! hub thread to re-publish the new state, so callers do not need to push
//! the change back into the channel themselves.
//!
//! Why pactl + wpctl: PipeWire ships with both pactl (PulseAudio compatibility
//! layer, gives us JSON state listings and the `subscribe` event stream) and
//! wpctl (native, makes volume queries simple). Matching what rs-bar does.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::watch;

// ── Public types ──────────────────────────────────────────────────────────

/// One PipeWire / PulseAudio sink or source. Same shape works for both.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// Stable name (`alsa_output...` / `alsa_input...`). Used as the
    /// argument to [`set_default_sink`] / [`set_default_source`].
    pub name: String,
    /// Human-readable description shown in the dropdown.
    pub description: String,
}

/// Backwards-compat alias — sinks are still `SinkInfo` everywhere they were
/// previously referenced.
pub type SinkInfo = DeviceInfo;

/// Latest snapshot of audio input + output state. One channel covers both
/// sinks (output) and sources (input/microphone).
#[derive(Clone, Debug, Default)]
pub struct VolumeState {
    // ── Output (default sink / speaker) ──────────────────────────
    /// Default sink's volume in percent. May exceed 100 if the user has
    /// boosted past unity gain (we cap reads at 999 for the label).
    pub percent: u32,
    /// Whether the default sink is muted.
    pub muted: bool,
    /// `name` of the current default sink.
    pub default_sink: String,
    /// Available sinks, in pactl order.
    pub sinks: Vec<DeviceInfo>,

    // ── Input (default source / microphone) ──────────────────────
    /// Default source's volume in percent.
    pub mic_percent: u32,
    /// Whether the default source is muted.
    pub mic_muted: bool,
    /// `name` of the current default source.
    pub default_source: String,
    /// Available sources, in pactl order. Filtered to non-monitor inputs
    /// (so we don't show every sink's monitor as a fake mic).
    pub sources: Vec<DeviceInfo>,
}

// ── Public API ────────────────────────────────────────────────────────────

/// Subscribe to volume-state updates. Lazily spawns the hub thread on first
/// call.
pub fn subscribe() -> watch::Receiver<VolumeState> {
    sender().subscribe()
}

/// Set the default sink's volume to `percent` (0..=100, hard-capped at 100).
/// Spawned on a detached thread so the GTK main loop is never blocked by the
/// child process.
pub fn set_volume(percent: u32) {
    let pct = percent.min(100);
    std::thread::Builder::new()
        .name("volume-set".into())
        .spawn(move || {
            // wpctl wants a 0.0..=1.0 fraction; "-l 1.0" caps the limit.
            let frac = format!("{:.2}", pct as f32 / 100.0);
            let _ = Command::new("wpctl")
                .args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", &frac])
                .output();
        })
        .ok();
}

/// Toggle the default sink's mute state.
pub fn toggle_mute() {
    std::thread::Builder::new()
        .name("volume-mute".into())
        .spawn(|| {
            let _ = Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                .output();
        })
        .ok();
}

/// Make the sink with `name` the new default. The hub picks up the change on
/// the next pactl event.
pub fn set_default_sink(name: &str) {
    let name = name.to_string();
    std::thread::Builder::new()
        .name("volume-set-sink".into())
        .spawn(move || {
            let _ = Command::new("pactl")
                .args(["set-default-sink", &name])
                .output();
        })
        .ok();
}

/// Set the default source's (microphone) volume to `percent`.
pub fn set_mic_volume(percent: u32) {
    let pct = percent.min(100);
    std::thread::Builder::new()
        .name("mic-set".into())
        .spawn(move || {
            let frac = format!("{:.2}", pct as f32 / 100.0);
            let _ = Command::new("wpctl")
                .args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SOURCE@", &frac])
                .output();
        })
        .ok();
}

/// Toggle the default source's mute state.
pub fn toggle_mic_mute() {
    std::thread::Builder::new()
        .name("mic-mute".into())
        .spawn(|| {
            let _ = Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"])
                .output();
        })
        .ok();
}

/// Make the source with `name` the new default input.
pub fn set_default_source(name: &str) {
    let name = name.to_string();
    std::thread::Builder::new()
        .name("volume-set-source".into())
        .spawn(move || {
            let _ = Command::new("pactl")
                .args(["set-default-source", &name])
                .output();
        })
        .ok();
}

// ── Internals ─────────────────────────────────────────────────────────────

/// Run `wpctl get-volume <target>` and parse `Volume: 0.42 [MUTED]`.
fn query_wpctl_volume(target: &str) -> (u32, bool) {
    let Ok(out) = Command::new("wpctl").args(["get-volume", target]).output() else {
        return (0, false);
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let muted = s.contains("[MUTED]");
    let frac = s
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0);
    let pct = (frac * 100.0).round().clamp(0.0, 999.0) as u32;
    (pct, muted)
}

/// `pactl get-default-{sink,source}` → bare device name.
fn query_default(kind: &str) -> String {
    Command::new("pactl")
        .args([&format!("get-default-{kind}")])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// `pactl -f json list <sinks|sources>` → `Vec<DeviceInfo>`. For sources,
/// filter out monitor devices (PipeWire's loopback of every sink) so the mic
/// dropdown only shows real input hardware.
fn query_devices(kind: &str) -> Vec<DeviceInfo> {
    let Ok(out) = Command::new("pactl").args(["-f", "json", "list", kind]).output() else {
        return Vec::new();
    };
    let Ok(arr) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return Vec::new();
    };
    arr.into_iter()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let description = s.get("description")?.as_str()?.to_string();
            // Drop sink-monitor pseudo-sources from the mic dropdown.
            if kind == "sources" && name.ends_with(".monitor") {
                return None;
            }
            Some(DeviceInfo { name, description })
        })
        .collect()
}

fn query_full_state() -> VolumeState {
    let (percent, muted) = query_wpctl_volume("@DEFAULT_AUDIO_SINK@");
    let (mic_percent, mic_muted) = query_wpctl_volume("@DEFAULT_AUDIO_SOURCE@");
    VolumeState {
        percent,
        muted,
        default_sink: query_default("sink"),
        sinks: query_devices("sinks"),
        mic_percent,
        mic_muted,
        default_source: query_default("source"),
        sources: query_devices("sources"),
    }
}

/// Lazily spawn the hub thread on first call. The thread runs `pactl
/// subscribe` and re-publishes a fresh state each time a sink-related event
/// fires; if the child dies (e.g. PipeWire restart), it reconnects after a
/// short delay.
fn sender() -> &'static watch::Sender<VolumeState> {
    static S: OnceLock<watch::Sender<VolumeState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(query_full_state());
        let producer = tx.clone();

        std::thread::Builder::new()
            .name("volume".into())
            .spawn(move || {
                loop {
                    // Spawn `pactl subscribe`. SIGTERM-on-parent-death so the
                    // child doesn't outlive us if the bar process exits.
                    use std::os::unix::process::CommandExt;
                    let mut cmd = Command::new("pactl");
                    cmd.arg("subscribe").stdout(Stdio::piped());
                    unsafe {
                        cmd.pre_exec(|| {
                            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                            Ok(())
                        });
                    }
                    let Ok(mut child) = cmd.spawn() else {
                        std::thread::sleep(Duration::from_secs(5));
                        continue;
                    };

                    let stdout = match child.stdout.take() {
                        Some(s) => s,
                        None => {
                            let _ = child.wait();
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        }
                    };

                    for line in std::io::BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        // Filter to events that can affect the published
                        // state. `server` covers default-sink changes.
                        if line.contains("sink") || line.contains("source") || line.contains("server") {
                            let new_state = query_full_state();
                            producer.send_if_modified(|cur| {
                                if state_changed(cur, &new_state) {
                                    *cur = new_state;
                                    true
                                } else {
                                    false
                                }
                            });
                        }
                    }
                    let _ = child.wait();
                    // pactl exited (e.g. PipeWire bounced). Wait briefly
                    // before respawning so we don't busy-loop.
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .ok();

        tx
    })
}

/// Cheap structural inequality used by `send_if_modified` to coalesce
/// no-op updates (pactl emits a lot of unrelated events).
fn state_changed(a: &VolumeState, b: &VolumeState) -> bool {
    if a.percent != b.percent
        || a.muted != b.muted
        || a.default_sink != b.default_sink
        || a.mic_percent != b.mic_percent
        || a.mic_muted != b.mic_muted
        || a.default_source != b.default_source
    {
        return true;
    }
    devices_changed(&a.sinks, &b.sinks) || devices_changed(&a.sources, &b.sources)
}

fn devices_changed(a: &[DeviceInfo], b: &[DeviceInfo]) -> bool {
    if a.len() != b.len() {
        return true;
    }
    a.iter()
        .zip(b.iter())
        .any(|(x, y)| x.name != y.name || x.description != y.description)
}
