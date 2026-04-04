use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{App, Global};
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Request, Response};

/// Global niri compositor state, observable by all bar windows.
pub struct NiriState {
    pub overview_open: bool,
}

impl Global for NiriState {}

/// Start a background thread that listens to niri events and updates the global state.
pub fn start_listener(cx: &mut App) {
    cx.set_global(NiriState {
        overview_open: false,
    });

    let dirty = Arc::new(AtomicBool::new(false));
    let overview = Arc::new(AtomicBool::new(false));

    let ev_dirty = dirty.clone();
    let ev_overview = overview.clone();
    std::thread::spawn(move || {
        let Ok(mut socket) = Socket::connect() else {
            log::error!("niri: failed to connect to socket");
            return;
        };
        let Ok(Ok(Response::Handled)) = socket.send(Request::EventStream) else {
            log::error!("niri: failed to start event stream");
            return;
        };

        let mut read_event = socket.read_events();
        loop {
            match read_event() {
                Ok(Event::OverviewOpenedOrClosed { is_open }) => {
                    ev_overview.store(is_open, Ordering::Release);
                    ev_dirty.store(true, Ordering::Release);
                }
                Ok(_) => {}
                Err(e) => {
                    log::error!("niri: event stream error: {e}");
                    break;
                }
            }
        }
    });

    let poll_dirty = dirty.clone();
    let poll_overview = overview.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;

            if poll_dirty.load(Ordering::Acquire) {
                poll_dirty.store(false, Ordering::Release);
                let is_open = poll_overview.load(Ordering::Acquire);
                let _ = cx.update(|cx| {
                    cx.global_mut::<NiriState>().overview_open = is_open;
                });
            }
        }
    })
    .detach();
}
