//! Window title widget. Subscribes to the niri hub and renders the title of
//! the focused window on this bar's monitor.
//!
//! Per-monitor scoping: each bar shows the title of the focused window living
//! on the workspace currently active on *its* monitor. When the user switches
//! focus to a window on another monitor, this bar keeps showing whatever was
//! last visible on its own monitor — so each bar acts independently.
//!
//! Mirrors rs-bar's GPUI version: rs-bar doesn't truncate text in code,
//! relying on overflow-hidden styling. Here we use GTK's ellipsization on the
//! label which gives the same effect inside a flex container.

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

pub struct WindowTitle {
    /// Connector name (e.g. "DP-2") captured from `BAR_CTX` in `init`. Used
    /// to scope the focused-window lookup to this bar's monitor.
    connector: String,
    /// Last-rendered title text, kept for the displayed-value coalescing
    /// check in `update`.
    title: String,
    /// Root box, held so `update` can hide the entire capsule when there's
    /// no focused window to show.
    root: gtk::Box,
    /// Held so `update` can rewrite the label text.
    label: gtk::Label,
}

pub enum WindowTitleMsg {
    Update(hub::niri::NiriSnapshot),
}

// `NiriSnapshot` doesn't implement `Debug` (it's defined in this crate's hub
// module). Provide a minimal manual impl so relm4's internals can format the
// message — same pattern as `workspaces.rs`.
impl std::fmt::Debug for WindowTitleMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowTitleMsg::Update(snap) => f
                .debug_struct("Update")
                .field("workspaces", &snap.workspaces.len())
                .field("windows", &snap.windows.len())
                .finish(),
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for WindowTitle {
    type Init = WidgetInit;
    type Input = WindowTitleMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            #[name = "label"]
            gtk::Label {
                set_label: "",
                set_ellipsize: gtk::pango::EllipsizeMode::End,
                set_xalign: 0.0,
                add_css_class: "window-title",
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();
        let connector = super::current_connector().unwrap_or_default();
        let model = WindowTitle {
            connector,
            title: String::new(),
            root: root.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, init.grouped);
        // Start hidden — we'll un-hide on the first non-empty title.
        root.set_visible(false);

        // Subscription: forward NiriSnapshot updates as component messages.
        // Send the current value first so the widget renders immediately.
        let mut rx = hub::niri::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            let snap = rx.borrow_and_update().clone();
            s.input(WindowTitleMsg::Update(snap));
            while rx.changed().await.is_ok() {
                let snap = rx.borrow_and_update().clone();
                s.input(WindowTitleMsg::Update(snap));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WindowTitleMsg::Update(snapshot) => {
                // Locate the workspace currently active on this monitor. Niri
                // guarantees exactly one active workspace per output.
                let active_ws = snapshot
                    .workspaces
                    .iter()
                    .find(|ws| {
                        ws.is_active && ws.output.as_deref() == Some(&self.connector)
                    });

                // Resolve the focused window for that workspace. Prefer the
                // workspace's `active_window_id` (set by niri) and fall back
                // to scanning windows with matching `workspace_id` for the
                // `is_focused` flag.
                let new_title = active_ws
                    .and_then(|ws| {
                        if let Some(id) = ws.active_window_id {
                            snapshot.windows.iter().find(|w| w.id == id)
                        } else {
                            snapshot
                                .windows
                                .iter()
                                .find(|w| w.workspace_id == Some(ws.id) && w.is_focused)
                        }
                    })
                    .and_then(|w| w.title.clone())
                    .unwrap_or_default();

                // Coalescing optimisation: skip the GTK property write when
                // the displayed text hasn't changed.
                if new_title == self.title {
                    return;
                }
                self.title = new_title;
                self.label.set_label(&self.title);
                // Hide the entire capsule when there's no title to show, so
                // we don't render an empty pill.
                self.root.set_visible(!self.title.is_empty());
            }
        }
    }
}

impl NamedWidget for WindowTitle {
    const NAME: &'static str = "window-title";
}
