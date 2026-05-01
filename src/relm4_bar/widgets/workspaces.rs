//! Workspaces widget. Subscribes to the niri hub and renders one dot per
//! workspace belonging to this bar's monitor (filtered by the connector
//! captured from `BAR_CTX` at init time).
//!
//! Dots are persistent across updates, keyed by workspace id, so CSS
//! transitions on background-color, border-color, and min-width animate
//! smoothly when:
//!   - focus shifts (active dot widens, neighbouring dots restore)
//!   - a workspace is added (new dot fades in from opacity 0)
//!   - a workspace becomes occupied or empty (color transitions)
//!   - the user hovers a dot (`:hover` rule widens it briefly)

use std::collections::HashMap;
use std::time::Duration;

use gtk::prelude::*;
use relm4::prelude::*;

use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, WorkspaceReferenceArg};

use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

/// State classes a dot can carry. Mutually exclusive — kept in lockstep with
/// CSS so we can mass-strip them on transition.
const STATE_CLASSES: &[&str] = &[
    "workspace-dot-active",
    "workspace-dot-windows",
    "workspace-dot-empty",
];

/// Cached per-dot state so we only touch GTK on actual transitions.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DotState {
    state: DotKind,
    urgent: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DotKind {
    Active,
    Windows,
    Empty,
}

impl DotKind {
    fn css_class(self) -> &'static str {
        match self {
            DotKind::Active => "workspace-dot-active",
            DotKind::Windows => "workspace-dot-windows",
            DotKind::Empty => "workspace-dot-empty",
        }
    }
}

pub struct Workspaces {
    /// Connector name (e.g. "DP-2") captured from `BAR_CTX` in `init`. Used
    /// to filter the niri hub's workspace list down to this bar's monitor.
    connector: String,
    /// Container that holds the dots in workspace order.
    container: gtk::Box,
    /// Persistent dots keyed by workspace id. Re-used across updates so CSS
    /// transitions can fire — rebuilding from scratch every update would
    /// always start with the new state already applied (no transition).
    dots: HashMap<u64, (gtk::Box, DotState)>,
}

pub enum WorkspacesMsg {
    Update(hub::niri::NiriSnapshot),
}

impl std::fmt::Debug for WorkspacesMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspacesMsg::Update(snap) => f
                .debug_struct("Update")
                .field("workspaces", &snap.workspaces.len())
                .field("overview_open", &snap.overview_open)
                .finish(),
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for Workspaces {
    type Init = WidgetInit;
    type Input = WorkspacesMsg;
    type Output = ();

    view! {
        #[name = "container"]
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let connector = super::current_connector().unwrap_or_default();
        let model = Workspaces {
            connector,
            container: widgets.container.clone(),
            dots: HashMap::new(),
        };

        capsule(&root, init.grouped);

        let mut rx = hub::niri::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            let snap = rx.borrow_and_update().clone();
            s.input(WorkspacesMsg::Update(snap));
            while rx.changed().await.is_ok() {
                let snap = rx.borrow_and_update().clone();
                s.input(WorkspacesMsg::Update(snap));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WorkspacesMsg::Update(snapshot) => {
                let mut workspaces: Vec<&niri_ipc::Workspace> = snapshot
                    .workspaces
                    .iter()
                    .filter(|ws| ws.output.as_deref() == Some(&self.connector))
                    .collect();
                workspaces.sort_by_key(|ws| ws.idx);

                // ── 1. Remove dots whose workspace vanished ─────────────
                //
                // We fade them out by setting opacity to 0 (the CSS
                // `transition: opacity` makes this animate), then drop the
                // widget after the transition completes via a short timeout.
                let alive: std::collections::HashSet<u64> =
                    workspaces.iter().map(|w| w.id).collect();
                let removed: Vec<u64> = self
                    .dots
                    .keys()
                    .copied()
                    .filter(|id| !alive.contains(id))
                    .collect();
                for id in removed {
                    if let Some((dot, _)) = self.dots.remove(&id) {
                        dot.set_opacity(0.0);
                        let container = self.container.clone();
                        glib::timeout_add_local_once(Duration::from_millis(220), move || {
                            container.remove(&dot);
                        });
                    }
                }

                // ── 2. Add or update dots for live workspaces ──────────
                //
                // We rebuild the container's child order on each update so
                // newly-inserted workspaces land in the right slot, but the
                // *widgets* are the same objects across updates so CSS
                // transitions fire on state changes.
                for (slot, ws) in workspaces.iter().enumerate() {
                    let kind = if ws.is_active {
                        DotKind::Active
                    } else if ws.active_window_id.is_some() {
                        DotKind::Windows
                    } else {
                        DotKind::Empty
                    };
                    let new_state = DotState {
                        state: kind,
                        urgent: ws.is_urgent,
                    };

                    let (dot, prev_state, is_new) = match self.dots.remove(&ws.id) {
                        Some((dot, prev)) => (dot, Some(prev), false),
                        None => (build_dot(ws.id), None, true),
                    };

                    if is_new {
                        // Fade in: start invisible, schedule the visible
                        // transition on the next idle so GTK's CSS
                        // transition catches the change.
                        dot.set_opacity(0.0);
                        let dot_for_idle = dot.clone();
                        glib::idle_add_local_once(move || {
                            dot_for_idle.set_opacity(1.0);
                        });
                    }

                    if prev_state != Some(new_state) {
                        for c in STATE_CLASSES {
                            if *c != new_state.state.css_class() {
                                dot.remove_css_class(c);
                            }
                        }
                        dot.add_css_class(new_state.state.css_class());
                        if new_state.urgent {
                            dot.add_css_class("workspace-dot-urgent");
                        } else {
                            dot.remove_css_class("workspace-dot-urgent");
                        }
                    }

                    // Position in the container at `slot`. If the dot is
                    // already at the right index we don't need to move it;
                    // GTK's reorder is cheap regardless.
                    if dot.parent().is_none() {
                        self.container.append(&dot);
                    }
                    let target_idx = slot as i32;
                    let current_idx = child_index(&self.container, &dot);
                    if current_idx != target_idx {
                        self.container.reorder_child_after(
                            &dot,
                            nth_child(&self.container, target_idx - 1).as_ref(),
                        );
                    }

                    self.dots.insert(ws.id, (dot, new_state));
                }
            }
        }
    }
}

/// Build a fresh dot widget with click handler. State classes are applied by
/// the caller.
fn build_dot(id: u64) -> gtk::Box {
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("workspace-dot");
    dot.set_halign(gtk::Align::Center);
    dot.set_valign(gtk::Align::Center);
    // Start at the "windows" baseline size (9x9). The `.workspace-dot-active`
    // CSS class bumps min-width to 18 with a transition.
    dot.set_size_request(9, 9);

    let click = gtk::GestureClick::new();
    click.connect_pressed(move |_, _, _, _| focus_workspace(id));
    dot.add_controller(click);
    dot
}

/// Return the index of `child` inside `parent`, or -1 if not found.
fn child_index(parent: &gtk::Box, child: &gtk::Box) -> i32 {
    let mut i = 0;
    let mut sibling = parent.first_child();
    while let Some(s) = sibling {
        if s.eq(child.upcast_ref::<gtk::Widget>()) {
            return i;
        }
        sibling = s.next_sibling();
        i += 1;
    }
    -1
}

/// Return the n-th child widget of `parent`, or None if out of bounds.
/// `nth_child(parent, -1)` returns None — used as the "insert at start" anchor
/// for `reorder_child_after`.
fn nth_child(parent: &gtk::Box, n: i32) -> Option<gtk::Widget> {
    if n < 0 {
        return None;
    }
    let mut i = 0;
    let mut sibling = parent.first_child();
    while let Some(s) = sibling {
        if i == n {
            return Some(s);
        }
        sibling = s.next_sibling();
        i += 1;
    }
    None
}

/// Open a fresh niri socket and dispatch `FocusWorkspace { id }`. Errors are
/// swallowed — failing to focus must not panic the bar.
fn focus_workspace(id: u64) {
    std::thread::spawn(move || {
        if let Ok(mut socket) = Socket::connect() {
            let _ = socket.send(Request::Action(Action::FocusWorkspace {
                reference: WorkspaceReferenceArg::Id(id),
            }));
        }
    });
}

impl NamedWidget for Workspaces {
    const NAME: &'static str = "workspaces";
}
