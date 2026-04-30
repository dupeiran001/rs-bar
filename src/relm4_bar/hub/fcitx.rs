//! Fcitx5 input-method hub. Watches tray events, queries `fcitx5-remote -n`.
//!
//! Mirrors the GPUI rs-bar approach: rather than talking to fcitx5 over D-Bus
//! directly, the hub piggybacks on the SNI (system tray) bus events that fcitx5
//! emits when the active input method changes, and re-runs `fcitx5-remote -n`
//! to read the current method name. This keeps the hub small while remaining
//! event-driven (no polling).
//!
//! Singleton background thread (`"fcitx-hub"`):
//!
//! 1. Subscribe to [`crate::relm4_bar::hub::tray`] (which already runs a current-thread
//!    tokio runtime + zbus listener for SNI).
//! 2. Each time the tray state changes, spawn a `tokio::process::Command` for
//!    `fcitx5-remote -n` and parse the stdout into a [`FcitxIm`].
//! 3. Publish the resulting [`FcitxState`] over a `tokio::sync::watch` channel.
//!
//! Conforms to the canonical hub pattern (`OnceLock<watch::Sender<_>>`,
//! a single named `std::thread`, lazy spawn on first `subscribe()`).

use std::sync::OnceLock;

use tokio::sync::watch;

/// Coarse classification of the active fcitx5 input method.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FcitxIm {
    /// `pinyin`, `rime`, … — Chinese input.
    Chinese,
    /// `keyboard-us`, `keyboard-…` — passthrough/Latin.
    English,
    /// fcitx5-remote unavailable, daemon not running, or unknown method id.
    #[default]
    Unknown,
}

/// Hub-published fcitx state. Wrapping in a struct (rather than re-exporting
/// the bare enum) leaves room for future fields (e.g. the raw method id)
/// without breaking subscribers.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FcitxState {
    pub im: FcitxIm,
}

/// Subscribe to fcitx state updates. Lazily spawns the hub thread on first call.
#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<FcitxState> {
    sender().subscribe()
}

fn sender() -> &'static watch::Sender<FcitxState> {
    static S: OnceLock<watch::Sender<FcitxState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(FcitxState::default());
        let producer = tx.clone();

        std::thread::Builder::new()
            .name("fcitx-hub".into())
            .spawn(move || listener(producer))
            .ok();

        tx
    })
}

/// Hub thread entry point. Builds a current-thread tokio runtime (mirroring
/// `hub::tray`) so async tray-watch reception and `tokio::process` are in scope.
fn listener(tx: watch::Sender<FcitxState>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("fcitx-hub: failed to build runtime: {e}");
            return;
        }
    };

    rt.block_on(async move {
        let mut tray_rx = crate::relm4_bar::hub::tray::subscribe();

        // Initial query so the bar shows something before the first tray event.
        let initial = query_fcitx().await;
        let _ = tx.send(FcitxState { im: initial });

        loop {
            // Wake on every tray-state change. fcitx5 publishes itself as an
            // SNI item, so any input-method switch produces a tray update.
            if tray_rx.changed().await.is_err() {
                log::error!("fcitx-hub: tray channel closed");
                break;
            }
            // Drain any further changes; we only care about the latest state.
            tray_rx.borrow_and_update();

            let im = query_fcitx().await;
            let new = FcitxState { im };
            if *tx.borrow() != new {
                let _ = tx.send(new);
            }
        }
    });
}

/// Run `fcitx5-remote -n` and classify its output.
///
/// `fcitx5-remote -n` prints the unique name of the current input method, e.g.
/// `keyboard-us`, `pinyin`, `rime`. Empty output / non-zero exit / missing
/// binary all collapse into [`FcitxIm::Unknown`].
async fn query_fcitx() -> FcitxIm {
    let output = tokio::process::Command::new("fcitx5-remote")
        .arg("-n")
        .output()
        .await;

    let Ok(out) = output else {
        return FcitxIm::Unknown;
    };
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    classify(&name)
}

fn classify(name: &str) -> FcitxIm {
    if name.starts_with("pinyin") || name.starts_with("rime") {
        FcitxIm::Chinese
    } else if name.starts_with("keyboard") {
        FcitxIm::English
    } else {
        FcitxIm::Unknown
    }
}
