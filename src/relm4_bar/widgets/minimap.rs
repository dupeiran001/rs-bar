//! Minimap widget. Subscribes to the niri hub and renders one rectangle per
//! window in this monitor's currently-active workspace, scaled proportionally
//! to the output dimensions.
//!
//! Like Workspaces, this is a dynamic list — children are cleared and rebuilt
//! on each Update. The container is a `gtk::Fixed` so each tile can be placed
//! at an arbitrary (x, y) computed from niri's scrolling-layout positions.

use gtk::prelude::*;
use relm4::prelude::*;

use niri_ipc::socket::Socket;
use niri_ipc::{Action, Request};

use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

pub struct Minimap {
    /// Connector name (e.g. "DP-2") captured from `BAR_CTX` in `init`. Used
    /// to find this bar's monitor in the snapshot's workspaces/outputs.
    connector: String,
    /// Root box, held so `update` can hide the entire capsule when this
    /// monitor's active workspace has no windows.
    root: gtk::Box,
    /// Dynamic absolute-positioned container — children are rebuilt on every
    /// `Update`.
    container: gtk::Fixed,
    /// Outer fixed-height row that wraps the Fixed; we resize it on each
    /// update so the bar layout claims the right amount of horizontal space.
    row: gtk::Box,
    /// Coalescing key: (window_count, focused_window_id, active_window_on_ws,
    /// active_workspace_id, overview_open, layout_fingerprint). The fingerprint
    /// hashes each window's (id, pos_in_scrolling_layout, tile_size) so layout
    /// changes (e.g. vertically stacking a window in niri overview, which
    /// halves tile heights) invalidate the cached rebuild.
    last_key: Option<(usize, Option<u64>, Option<u64>, Option<u64>, bool, u64)>,
}

pub enum MinimapMsg {
    Update(hub::niri::NiriSnapshot),
}

// `NiriSnapshot` doesn't implement `Debug`. Provide a minimal manual impl so
// relm4's internals can format the message.
impl std::fmt::Debug for MinimapMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MinimapMsg::Update(snap) => f
                .debug_struct("Update")
                .field("windows", &snap.windows.len())
                .field("workspaces", &snap.workspaces.len())
                .field("overview_open", &snap.overview_open)
                .finish(),
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for Minimap {
    type Init = WidgetInit;
    type Input = MinimapMsg;
    type Output = ();

    view! {
        #[name = "row"]
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            // Hard-clip overflow: niri's overview mode can report tiles
            // stacked beyond the bar's content height (vertical columns,
            // workspace previews). Without clipping, the inner Fixed grows
            // to contain them and pushes the bar window taller.
            set_overflow: gtk::Overflow::Hidden,
            #[name = "container"]
            gtk::Fixed {
                set_valign: gtk::Align::Center,
                set_overflow: gtk::Overflow::Hidden,
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
        let model = Minimap {
            connector,
            root: root.clone(),
            container: widgets.container.clone(),
            row: widgets.row.clone(),
            last_key: None,
        };

        capsule(&root, init.grouped);
        // Start hidden — we'll un-hide as soon as any windows are placed.
        root.set_visible(false);

        crate::subscribe_into_msg!(hub::niri::subscribe(), sender, MinimapMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            MinimapMsg::Update(snapshot) => {
                // Find the active workspace on this monitor.
                let active_ws = snapshot
                    .workspaces
                    .iter()
                    .find(|ws| ws.is_active && ws.output.as_deref() == Some(&self.connector));
                let active_ws_id = active_ws.map(|ws| ws.id);
                let active_window_on_ws = active_ws.and_then(|ws| ws.active_window_id);

                // Filter windows to that workspace and require layout info.
                let mut ws_windows: Vec<&niri_ipc::Window> = snapshot
                    .windows
                    .iter()
                    .filter(|w| {
                        w.workspace_id.is_some()
                            && w.workspace_id == active_ws_id
                            && w.layout.pos_in_scrolling_layout.is_some()
                    })
                    .collect();
                ws_windows.sort_by_key(|w| w.layout.pos_in_scrolling_layout);

                // Coalesce: skip rebuild when the meaningful key is unchanged.
                let focused_id = ws_windows.iter().find(|w| w.is_focused).map(|w| w.id);
                // Fingerprint window layout (id + position + tile_size) so a
                // pure size change (e.g. vertical stacking that halves
                // neighbouring tile heights) invalidates the cache. Without
                // this the minimap kept rendering with stale tile_sizes until
                // a counted-quantity changed (window add/remove/focus).
                let mut layout_fp: u64 = 0;
                for w in &ws_windows {
                    let (col, row) = w.layout.pos_in_scrolling_layout.unwrap_or((0, 0));
                    let (tw, th) = w.layout.tile_size;
                    layout_fp = layout_fp.wrapping_mul(1_000_003).wrapping_add(w.id);
                    layout_fp = layout_fp.wrapping_mul(1_000_003).wrapping_add(col as u64);
                    layout_fp = layout_fp.wrapping_mul(1_000_003).wrapping_add(row as u64);
                    layout_fp = layout_fp
                        .wrapping_mul(1_000_003)
                        .wrapping_add((tw * 1000.0) as u64);
                    layout_fp = layout_fp
                        .wrapping_mul(1_000_003)
                        .wrapping_add((th * 1000.0) as u64);
                }
                let key = (
                    ws_windows.len(),
                    focused_id,
                    active_window_on_ws,
                    active_ws_id,
                    snapshot.overview_open,
                    layout_fp,
                );
                if self.last_key == Some(key) {
                    return;
                }
                self.last_key = Some(key);

                // Output logical size for this monitor (used to scale).
                let output_size = snapshot
                    .outputs
                    .iter()
                    .find(|o| o.name == self.connector)
                    .and_then(|o| o.logical.as_ref())
                    .map(|l| (l.width as f64, l.height as f64));

                // Reconstruct tile positions: stack tiles vertically within
                // each scrolling-layout column, columns laid out left to right.
                struct Tile {
                    id: u64,
                    x: f64,
                    y: f64,
                    w: f64,
                    h: f64,
                    focused: bool,
                    active_on_ws: bool,
                }

                let mut tiles: Vec<Tile> = Vec::new();
                let mut col_x: f64 = 0.0;
                let mut prev_col: usize = 0;
                let mut col_y: f64 = 0.0;

                for win in &ws_windows {
                    let (col, _tile) = win.layout.pos_in_scrolling_layout.unwrap();
                    let (tw, th) = win.layout.tile_size;

                    if col != prev_col {
                        if let Some(last) = tiles.last() {
                            col_x = last.x + last.w;
                        }
                        col_y = 0.0;
                        prev_col = col;
                    }

                    tiles.push(Tile {
                        id: win.id,
                        x: col_x,
                        y: col_y,
                        w: tw,
                        h: th,
                        focused: win.is_focused,
                        active_on_ws: active_window_on_ws == Some(win.id),
                    });
                    col_y += th;
                }

                let total_w = tiles.iter().map(|t| t.x + t.w).fold(0.0_f64, f64::max);
                let total_h = tiles.iter().map(|t| t.y + t.h).fold(0.0_f64, f64::max);

                let (out_w, out_h) = output_size.unwrap_or((total_w.max(1.0), total_h.max(1.0)));
                let view_w = total_w.max(out_w);
                // Include total_h in the vertical viewport so vertically-
                // stacked windows (niri overview, vertical columns) scale
                // down to fit map_h. Without this, total_h could exceed
                // view_h and tiles would overflow past map_h, dragging the
                // bar window taller via Fixed's natural-size propagation.
                let view_h = out_h.max(total_h).max(1.0);

                // Map height: 22px fills the 24px-min capsule (border 1+1)
                // wall-to-wall. We still vertically-center the tiles within
                // map_h below, so any slack between scaled content and
                // map_h distributes evenly above/below.
                let map_h: f64 = 22.0;
                let scale = map_h / view_h;
                let map_w = (view_w * scale).max(1.0);
                let scaled_total_h = total_h * scale;
                let y_offset = ((map_h - scaled_total_h) * 0.5).max(0.0);

                // Tear down all existing children and rebuild from scratch.
                while let Some(child) = self.container.first_child() {
                    self.container.remove(&child);
                }

                // Hide the entire capsule when there are no windows on this
                // monitor's active workspace, so we don't render an empty pill.
                if tiles.is_empty() {
                    self.root.set_visible(false);
                    return;
                }
                self.root.set_visible(true);

                self.container.set_size_request(map_w as i32, map_h as i32);
                self.row.set_size_request(map_w as i32, -1);

                for tile in tiles {
                    let x = tile.x * scale;
                    let y = tile.y * scale + y_offset;
                    let w = (tile.w * scale).max(2.0);
                    let h = (tile.h * scale).max(2.0);

                    let r = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    r.set_size_request(w as i32, h as i32);
                    r.add_css_class("minimap-window");
                    if tile.focused {
                        r.add_css_class("minimap-window-focused");
                    } else if tile.active_on_ws {
                        r.add_css_class("minimap-window-active");
                    }
                    if snapshot.overview_open {
                        r.add_css_class("minimap-window-overview");
                    }

                    // Click → focus that window in niri.
                    let win_id = tile.id;
                    let click = gtk::GestureClick::new();
                    click.connect_pressed(move |_, _, _, _| focus_window(win_id));
                    r.add_controller(click);
                    r.set_cursor_from_name(Some("pointer"));

                    self.container.put(&r, x, y);
                }
            }
        }
    }
}

/// Open a fresh niri socket and dispatch `FocusWindow { id }`. Errors are
/// swallowed — failing to focus must not panic the bar.
fn focus_window(id: u64) {
    std::thread::spawn(move || {
        if let Ok(mut socket) = Socket::connect() {
            let _ = socket.send(Request::Action(Action::FocusWindow { id }));
        }
    });
}

impl NamedWidget for Minimap {
    const NAME: &'static str = "minimap";
}
