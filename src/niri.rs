//! Unified niri compositor event hub.
//!
//! One background thread opens **one** niri event stream, maintains the
//! complete state (outputs, workspaces, windows, overview) and publishes a
//! [`NiriSnapshot`] through a [`Broadcast`] every time anything changes.
//!
//! Every niri-aware widget (workspaces, minimap, window_title) and the
//! overview-triggered global repaint subscribe to this single source, rather
//! than each opening its own socket + event stream + polling loop. On a
//! dual-monitor setup that collapses 6 niri event-stream connections into 1
//! and eliminates 4 × 50 ms dirty-flag polling loops.
//!
//! The existing [`NiriState`] GPUI Global is kept so the rest of the app can
//! still call `cx.observe_global::<NiriState>()` — it is fed from a
//! subscription to the same hub.

use std::sync::OnceLock;

use gpui::{App, Global};
use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamStatePart, WindowsState, WorkspacesState};
use niri_ipc::{Event, Request, Response};

use crate::hub::Broadcast;

/// Complete niri state snapshot. Cloned per subscriber on publish, so keep it
/// lean — the Vec fields reuse niri_ipc types that already implement Clone.
#[derive(Clone, Default)]
pub struct NiriSnapshot {
    pub workspaces: Vec<niri_ipc::Workspace>,
    pub windows: Vec<niri_ipc::Window>,
    pub outputs: Vec<niri_ipc::Output>,
    pub overview_open: bool,
}

/// Global niri compositor state, observable by all bar windows via GPUI
/// `cx.observe_global::<NiriState>()`. Populated from the hub.
pub struct NiriState {
    pub overview_open: bool,
}

impl Global for NiriState {}

/// Broadcast handle. First call spawns the single listener thread; later
/// calls reuse it.
pub fn broadcast() -> &'static Broadcast<NiriSnapshot> {
    static BC: OnceLock<Broadcast<NiriSnapshot>> = OnceLock::new();
    BC.get_or_init(|| {
        let bc = Broadcast::<NiriSnapshot>::new();
        let producer = bc.clone();
        std::thread::Builder::new()
            .name("niri-hub".into())
            .spawn(move || listener(producer))
            .ok();
        bc
    })
}

/// Initialise the GPUI global + subscribe to the hub so overview state is
/// kept in sync and observers get notified on change. Call once at startup.
pub fn start_listener(cx: &mut App) {
    cx.set_global(NiriState {
        overview_open: false,
    });

    // Force the hub thread to spawn now rather than lazily on first widget.
    let sub = broadcast().subscribe();
    cx.spawn(async move |cx| {
        let mut last_overview = false;
        while let Some(snap) = sub.next().await {
            if snap.overview_open != last_overview {
                last_overview = snap.overview_open;
                let _ = cx.update(|cx| {
                    cx.global_mut::<NiriState>().overview_open = last_overview;
                });
            }
        }
    })
    .detach();
}

/// Background listener. Owns the single niri event-stream socket and
/// publishes a snapshot of the complete state every time anything changes.
fn listener(bc: Broadcast<NiriSnapshot>) {
    // One-shot socket for the initial outputs/windows/workspaces dump.
    let mut outputs: Vec<niri_ipc::Output> = Vec::new();
    let mut initial_windows: Vec<niri_ipc::Window> = Vec::new();

    if let Ok(mut s) = Socket::connect() {
        if let Ok(Ok(Response::Outputs(o))) = s.send(Request::Outputs) {
            outputs = o.into_values().collect();
        }
    }
    if let Ok(mut s) = Socket::connect() {
        if let Ok(Ok(Response::Windows(w))) = s.send(Request::Windows) {
            initial_windows = w;
        }
    }

    // Main event-stream socket.
    let Ok(mut socket) = Socket::connect() else {
        log::error!("niri-hub: failed to connect to socket");
        return;
    };
    let Ok(Ok(Response::Handled)) = socket.send(Request::EventStream) else {
        log::error!("niri-hub: failed to start event stream");
        return;
    };

    let mut read_event = socket.read_events();

    // niri_ipc's state helpers keep incremental state up to date from the
    // delta events. We apply each event to both layers; the first that
    // returns None (handled) consumes the event.
    let mut ws_state = WorkspacesState::default();
    let mut win_state = WindowsState::default();

    // Track overview separately since niri_ipc::state doesn't cover it.
    let mut overview_open = false;

    // Seed windows from the initial dump by synthesising a WindowsChanged event.
    if !initial_windows.is_empty() {
        let _ = win_state.apply(Event::WindowsChanged {
            windows: initial_windows,
        });
    }

    // Publish the initial snapshot so subscribers show real data immediately.
    publish(&bc, &ws_state, &win_state, &outputs, overview_open);

    loop {
        let event = match read_event() {
            Ok(e) => e,
            Err(e) => {
                log::error!("niri-hub: event stream error: {e}");
                break;
            }
        };

        // Handle overview events before delegating to state apply.
        if let Event::OverviewOpenedOrClosed { is_open } = event {
            overview_open = is_open;
            publish(&bc, &ws_state, &win_state, &outputs, overview_open);
            continue;
        }

        // Feed the event through the workspaces state first, then windows.
        // The state machines return None when they've consumed the event and
        // Some(event) when they haven't — pass the leftover through.
        let event = match ws_state.apply(event) {
            None => {
                publish(&bc, &ws_state, &win_state, &outputs, overview_open);
                continue;
            }
            Some(e) => e,
        };
        if win_state.apply(event).is_none() {
            publish(&bc, &ws_state, &win_state, &outputs, overview_open);
        }
    }
}

fn publish(
    bc: &Broadcast<NiriSnapshot>,
    ws_state: &WorkspacesState,
    win_state: &WindowsState,
    outputs: &[niri_ipc::Output],
    overview_open: bool,
) {
    let mut workspaces: Vec<_> = ws_state.workspaces.values().cloned().collect();
    workspaces.sort_by(|a, b| a.output.cmp(&b.output).then(a.idx.cmp(&b.idx)));
    let windows: Vec<_> = win_state.windows.values().cloned().collect();
    bc.publish(NiriSnapshot {
        workspaces,
        windows,
        outputs: outputs.to_vec(),
        overview_open,
    });
}
