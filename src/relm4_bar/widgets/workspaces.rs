//! Workspaces widget. Subscribes to the niri hub and renders one pill per
//! workspace belonging to this bar's monitor (filtered by the connector
//! captured from `BAR_CTX` at init time).
//!
//! The number of workspaces is dynamic, so unlike fixed-layout widgets this
//! one uses a plain `gtk::Box` as root and rebuilds its children on every
//! `Update`. For typical workspace counts (<=10) this is cheap and avoids
//! pulling in `FactoryVecDeque`.

use gtk::prelude::*;
use relm4::prelude::*;

use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request, WorkspaceReferenceArg};

use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

pub struct Workspaces {
    /// Connector name (e.g. "DP-2") captured from `BAR_CTX` in `init`. Used
    /// to filter the niri hub's workspace list down to this bar's monitor.
    connector: String,
    /// Dynamic container — children are rebuilt on every `Update`.
    container: gtk::Box,
}

pub enum WorkspacesMsg {
    Update(hub::niri::NiriSnapshot),
}

// `NiriSnapshot` doesn't implement `Debug` (it's defined in this crate's hub
// module which we don't modify here). Provide a minimal manual impl so relm4's
// internals can format the message.
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
        };

        capsule(&root, init.grouped);

        // Subscription: forward NiriSnapshot updates as component messages.
        // Send the current value first so the widget renders immediately.
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
                // Defensive sort — the hub already sorts, but be safe.
                workspaces.sort_by_key(|ws| ws.idx);

                // Tear down all existing children and rebuild from scratch.
                while let Some(child) = self.container.first_child() {
                    self.container.remove(&child);
                }

                for ws in &workspaces {
                    // Render each workspace as a fixed-size dot rather than a
                    // numeric pill — matches the GPUI bar's three-state dot:
                    //   active        : 18×9 accent-coloured pill
                    //   has windows   : 9×9 dim dot
                    //   empty         : 9×9 gutter-coloured dot
                    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    dot.add_css_class("workspace-dot");
                    if ws.is_active {
                        dot.add_css_class("workspace-dot-active");
                        dot.set_size_request(18, 9);
                    } else if ws.active_window_id.is_some() {
                        dot.add_css_class("workspace-dot-windows");
                        dot.set_size_request(9, 9);
                    } else {
                        dot.add_css_class("workspace-dot-empty");
                        dot.set_size_request(9, 9);
                    }
                    if ws.is_urgent {
                        dot.add_css_class("workspace-dot-urgent");
                    }

                    let id = ws.id;
                    let click = gtk::GestureClick::new();
                    click.connect_pressed(move |_, _, _, _| focus_workspace(id));
                    dot.add_controller(click);

                    self.container.append(&dot);
                }
            }
        }
    }
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
