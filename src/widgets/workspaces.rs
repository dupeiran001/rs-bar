use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    BoxShadow, Context, ElementId, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, point, px, rgb,
};
use uuid::Uuid;
use niri_ipc::socket::Socket;
use niri_ipc::state::EventStreamStatePart;
use niri_ipc::{Action, Request, Response, WorkspaceReferenceArg};

use super::{BarWidget, impl_render};

struct SharedState {
    workspaces: Vec<niri_ipc::Workspace>,
    outputs: Vec<niri_ipc::Output>,
}

pub struct Workspaces {
    workspaces: Vec<niri_ipc::Workspace>,
    outputs: Vec<niri_ipc::Output>,
}

impl Workspaces {
    /// Match GPUI display to a niri output by UUID.
    /// GPUI's WaylandDisplay computes uuid as `Uuid::new_v5(DNS, name)`.
    fn output_for_display(&self, window: &Window, cx: &Context<Self>) -> Option<String> {
        let display = window.display(cx)?;
        let display_uuid = display.uuid().ok()?;

        self.outputs
            .iter()
            .find(|o| {
                Uuid::new_v5(&Uuid::NAMESPACE_DNS, o.name.as_bytes()) == display_uuid
            })
            .map(|o| o.name.clone())
    }
}

impl BarWidget for Workspaces {
    const NAME: &str = "workspaces";

    fn new(cx: &mut Context<Self>) -> Self {
        let shared = Arc::new(Mutex::new(SharedState {
            workspaces: Vec::new(),
            outputs: Vec::new(),
        }));
        let dirty = Arc::new(AtomicBool::new(false));

        // Background thread: fetch outputs, then listen to workspace events
        let ws_shared = shared.clone();
        let ws_dirty = dirty.clone();
        std::thread::spawn(move || {
            // Fetch outputs on a one-shot connection
            if let Ok(mut socket) = Socket::connect() {
                if let Ok(Ok(Response::Outputs(outputs))) = socket.send(Request::Outputs) {
                    ws_shared.lock().unwrap().outputs =
                        outputs.into_values().collect();
                    ws_dirty.store(true, Ordering::Release);
                }
            }

            // Start event stream on a new connection
            let Ok(mut socket) = Socket::connect() else {
                log::error!("workspaces: failed to connect to niri socket");
                return;
            };
            let Ok(Ok(Response::Handled)) = socket.send(Request::EventStream) else {
                log::error!("workspaces: failed to start event stream");
                return;
            };

            let mut read_event = socket.read_events();
            let mut state = niri_ipc::state::WorkspacesState::default();

            loop {
                match read_event() {
                    Ok(event) => {
                        if state.apply(event).is_none() {
                            let mut ws: Vec<_> =
                                state.workspaces.values().cloned().collect();
                            ws.sort_by(|a, b| {
                                a.output.cmp(&b.output).then(a.idx.cmp(&b.idx))
                            });
                            ws_shared.lock().unwrap().workspaces = ws;
                            ws_dirty.store(true, Ordering::Release);
                        }
                    }
                    Err(e) => {
                        log::error!("workspaces: event stream error: {e}");
                        break;
                    }
                }
            }
        });

        // GPUI poller: pick up changes from the background thread
        let poll_shared = shared.clone();
        let poll_dirty = dirty.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;

                if poll_dirty.load(Ordering::Acquire) {
                    poll_dirty.store(false, Ordering::Release);
                    let state = poll_shared.lock().unwrap();
                    let ws = state.workspaces.clone();
                    let outputs = state.outputs.clone();
                    drop(state);

                    if this
                        .update(cx, |this, cx| {
                            this.workspaces = ws;
                            this.outputs = outputs;
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
        .detach();

        Self {
            workspaces: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let content_h = crate::config::CONTENT_HEIGHT();
        let output_name = self.output_for_display(window, cx);

        let filtered: Vec<_> = self
            .workspaces
            .iter()
            .filter(|ws| match (&output_name, &ws.output) {
                (Some(bar_out), Some(ws_out)) => bar_out == ws_out,
                _ => true,
            })
            .collect();

        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let ws_btn_h = button_h - 4.0;
        let ws_btn_radius = ws_btn_h / 2.0;

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .gap(px(2.0))
            .px(px(4.0))
            .children(filtered.into_iter().map(|ws| {
                let label = ws
                    .name
                    .as_deref()
                    .map(String::from)
                    .unwrap_or_else(|| ws.idx.to_string());

                let id = ElementId::Name(format!("ws-{}", ws.id).into());
                let ws_id = ws.id;

                let mut button = div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(ws_btn_h))
                    .px(px(6.0))
                    .text_xs()
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .rounded(px(ws_btn_radius))
                    .cursor_pointer()
                    .on_click(move |_, _, _| {
                        let id = ws_id;
                        std::thread::spawn(move || {
                            if let Ok(mut socket) = Socket::connect() {
                                let _ =
                                    socket.send(Request::Action(Action::FocusWorkspace {
                                        reference: WorkspaceReferenceArg::Id(id),
                                    }));
                            }
                        });
                    })
                    .child(label);

                if ws.is_active {
                    let shadow_color = rgb(t.bg).into();
                    button = button
                        .bg(rgb(t.accent))
                        .text_color(rgb(t.bg))
                        .shadow(vec![BoxShadow {
                            color: shadow_color,
                            offset: point(px(0.), px(1.)),
                            blur_radius: px(3.),
                            spread_radius: px(1.),
                        }]);
                } else {
                    button = button
                        .bg(rgb(t.surface))
                        .text_color(rgb(t.fg_dark))
                        .hover(|s| {
                            s.border_color(rgb(t.teal)).text_color(rgb(t.fg))
                        });
                }

                button
            }))
    }
}

impl_render!(Workspaces);
