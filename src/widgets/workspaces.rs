use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, Window, div, px, rgb,
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

#[derive(Clone, Copy, Debug)]
struct DotMemory {
    /// Last target the dot was animating toward.
    target_w: f32,
    target_color: u32,
    /// Where the current animation starts from.
    baseline_w: f32,
    baseline_color: u32,
}

pub struct Workspaces {
    workspaces: Vec<niri_ipc::Workspace>,
    outputs: Vec<niri_ipc::Output>,
    dot_memory: HashMap<u64, DotMemory>,
    hovered_id: Option<u64>,
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
                            log::debug!(
                                "workspaces: poll picked up update, {} workspaces",
                                ws.len()
                            );
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
            dot_memory: HashMap::new(),
            hovered_id: None,
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
        let dot_size: f32 = 9.0;
        let active_w: f32 = 18.0;
        let hover_bonus: f32 = 5.0;
        let hovered_id = self.hovered_id;

        // Build per-dot render data, updating baseline only on actual transitions.
        struct DotRender {
            ws_id: u64,
            state_tag: &'static str,
            hover_tag: &'static str,
            target_w: f32,
            target_color: u32,
            from_w: f32,
            from_color: u32,
        }

        // Reap memory for workspaces that no longer exist
        let alive: std::collections::HashSet<u64> = filtered.iter().map(|w| w.id).collect();
        let before_count = self.dot_memory.len();
        self.dot_memory.retain(|id, _| alive.contains(id));
        if self.dot_memory.len() != before_count {
            log::debug!(
                "workspaces: reaped {} dot_memory entries",
                before_count - self.dot_memory.len()
            );
        }

        let mut dots: Vec<DotRender> = Vec::with_capacity(filtered.len());
        for ws in &filtered {
            let has_windows = ws.active_window_id.is_some();
            let is_hovered = hovered_id == Some(ws.id);
            let hover_tag: &'static str = if is_hovered { "h" } else { "n" };
            let extra_w = if is_hovered { hover_bonus } else { 0.0 };
            let (target_w, target_color, state_tag) = if ws.is_active {
                (active_w + extra_w, t.accent, "a")
            } else if has_windows {
                (dot_size + extra_w, t.text_dim, "o")
            } else {
                (dot_size + extra_w, t.fg_gutter, "e")
            };

            let mem = match self.dot_memory.get(&ws.id).copied() {
                Some(prev) => {
                    if prev.target_w != target_w || prev.target_color != target_color {
                        // Transition: rebase onto the previous target.
                        let new_mem = DotMemory {
                            target_w,
                            target_color,
                            baseline_w: prev.target_w,
                            baseline_color: prev.target_color,
                        };
                        log::debug!(
                            "workspaces: ws {} transition {:?} -> {:?} (baseline w={}, color=#{:06x})",
                            ws.id,
                            (prev.target_w, format!("#{:06x}", prev.target_color)),
                            (target_w, format!("#{:06x}", target_color)),
                            new_mem.baseline_w,
                            new_mem.baseline_color
                        );
                        self.dot_memory.insert(ws.id, new_mem);
                        new_mem
                    } else {
                        // No change: keep baseline stable so animation closure
                        // captures the same from/to on every re-render.
                        prev
                    }
                }
                None => {
                    // New workspace: grow from nothing.
                    let new_mem = DotMemory {
                        target_w,
                        target_color,
                        baseline_w: 0.0,
                        baseline_color: t.surface,
                    };
                    log::debug!(
                        "workspaces: ws {} appearing (target w={}, color=#{:06x})",
                        ws.id,
                        target_w,
                        target_color
                    );
                    self.dot_memory.insert(ws.id, new_mem);
                    new_mem
                }
            };

            dots.push(DotRender {
                ws_id: ws.id,
                state_tag,
                hover_tag,
                target_w: mem.target_w,
                target_color: mem.target_color,
                from_w: mem.baseline_w,
                from_color: mem.baseline_color,
            });
        }

        log::trace!("workspaces: rendering {} dots", dots.len());

        let entity = cx.weak_entity();

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .gap(px(4.0))
            .px(px(6.0))
            .children(dots.into_iter().map(move |d| {
                let id = ElementId::Name(format!("ws-{}", d.ws_id).into());
                let ws_id = d.ws_id;
                let anim_key = ElementId::Name(
                    format!("ws-anim-{}-{}{}", d.ws_id, d.state_tag, d.hover_tag).into(),
                );

                let from_w = d.from_w;
                let target_w = d.target_w;
                let from_bg: Hsla = rgb(d.from_color).into();
                let target_bg: Hsla = rgb(d.target_color).into();

                let hover_entity = entity.clone();
                let inner = div()
                    .id(id)
                    .flex_shrink_0()
                    .h(px(dot_size))
                    .rounded(px(dot_size / 2.0))
                    .cursor_pointer()
                    .on_hover(move |is_hovered, _window, cx| {
                        let hovered = *is_hovered;
                        let _ = hover_entity.update(cx, |this, cx| {
                            if hovered {
                                if this.hovered_id != Some(ws_id) {
                                    this.hovered_id = Some(ws_id);
                                    cx.notify();
                                }
                            } else if this.hovered_id == Some(ws_id) {
                                this.hovered_id = None;
                                cx.notify();
                            }
                        });
                    })
                    .on_click(move |_, _, _| {
                        let id = ws_id;
                        log::debug!("workspaces: click ws {}", id);
                        std::thread::spawn(move || {
                            if let Ok(mut socket) = Socket::connect() {
                                let _ = socket.send(Request::Action(
                                    Action::FocusWorkspace {
                                        reference: WorkspaceReferenceArg::Id(id),
                                    },
                                ));
                            }
                        });
                    });

                inner.with_animation(
                    anim_key,
                    Animation::new(Duration::from_millis(220))
                        .with_easing(gpui::ease_out_quint()),
                    move |el, progress| {
                        let w = from_w + (target_w - from_w) * progress;

                        // Crossfade colors via alpha blending (matches volume.rs pattern).
                        let mut blended = target_bg;
                        blended.a = progress;
                        let bg = from_bg.blend(blended);

                        el.w(px(w)).bg(bg)
                    },
                )
            }))
    }
}

impl_render!(Workspaces);
