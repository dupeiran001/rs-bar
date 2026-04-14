use gpui::{
    Context, IntoElement, ParentElement, Styled, Window, div, px, rgb,
};
use uuid::Uuid;

use super::{BarWidget, impl_render};

pub struct Minimap {
    windows: Vec<niri_ipc::Window>,
    workspaces: Vec<niri_ipc::Workspace>,
    outputs: Vec<niri_ipc::Output>,
}

impl Minimap {
    fn output_for_display(&self, window: &Window, cx: &Context<Self>) -> Option<String> {
        let display = window.display(cx)?;
        let display_uuid = display.uuid().ok()?;
        self.outputs
            .iter()
            .find(|o| Uuid::new_v5(&Uuid::NAMESPACE_DNS, o.name.as_bytes()) == display_uuid)
            .map(|o| o.name.clone())
    }

    fn active_workspace_id(&self, output_name: &Option<String>) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| ws.is_active && ws.output.as_ref() == output_name.as_ref())
            .map(|ws| ws.id)
    }
}

impl BarWidget for Minimap {
    const NAME: &str = "minimap";

    fn new(cx: &mut Context<Self>) -> Self {
        let sub = crate::gpui_bar::niri::broadcast().subscribe();
        cx.spawn(async move |this, cx| {
            while let Some(snap) = sub.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.windows = snap.windows;
                        this.workspaces = snap.workspaces;
                        this.outputs = snap.outputs;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            windows: Vec::new(),
            workspaces: Vec::new(),
            outputs: Vec::new(),
        }
    }

    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let output_name = self.output_for_display(window, cx);
        let active_ws_id = self.active_workspace_id(&output_name);

        // The active window on this workspace (even if global focus is elsewhere)
        let active_window_id = self
            .workspaces
            .iter()
            .find(|ws| Some(ws.id) == active_ws_id)
            .and_then(|ws| ws.active_window_id);

        // Get output dimensions for scaling
        let output_size = output_name.as_ref().and_then(|name| {
            self.outputs.iter().find(|o| &o.name == name).and_then(|o| {
                o.logical.as_ref().map(|l| (l.width as f64, l.height as f64))
            })
        });

        // Filter windows to active workspace with layout info
        let mut ws_windows: Vec<_> = self
            .windows
            .iter()
            .filter(|w| {
                w.workspace_id.is_some()
                    && w.workspace_id == active_ws_id
                    && w.layout.pos_in_scrolling_layout.is_some()
            })
            .collect();

        // Sort by (column, tile) for consistent layout
        ws_windows.sort_by_key(|w| w.layout.pos_in_scrolling_layout);

        // Reconstruct tile positions from column/tile indices and sizes.
        // Group by column, stack tiles vertically within each column,
        // columns go left to right.
        struct Tile {
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
                // New column: advance x by the previous column's width
                if let Some(last) = tiles.last() {
                    col_x = last.x + last.w;
                }
                col_y = 0.0;
                prev_col = col;
            }

            tiles.push(Tile {
                x: col_x,
                y: col_y,
                w: tw,
                h: th,
                focused: win.is_focused,
                active_on_ws: active_window_id == Some(win.id),
            });

            col_y += th;
        }

        // Compute total bounds for scaling
        let total_w = tiles
            .iter()
            .map(|t| t.x + t.w)
            .fold(0.0_f64, f64::max);
        let total_h = tiles
            .iter()
            .map(|t| t.y + t.h)
            .fold(0.0_f64, f64::max);

        // Use actual tile bounds or output size, whichever is wider
        let (out_w, out_h) = output_size.unwrap_or((total_w.max(1.0), total_h.max(1.0)));
        let view_w = total_w.max(out_w);
        let view_h = out_h;

        let map_h = content_h - 4.0; // 2px top + 2px bottom
        let scale = map_h as f64 / view_h;
        let map_w = (view_w * scale) as f32;

        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(content_h))
            .child(
                div()
                    .relative()
                    .w(px(map_w))
                    .h(px(map_h))
                    .children(tiles.into_iter().map(|tile| {
                        let x = (tile.x * scale) as f32;
                        let y = (tile.y * scale) as f32;
                        let w = (tile.w * scale).max(2.0) as f32;
                        let h = (tile.h * scale).max(2.0) as f32;
                        let color = if tile.focused {
                            t.accent
                        } else if tile.active_on_ws {
                            t.accent_dim
                        } else {
                            t.border
                        };

                        div()
                            .absolute()
                            .left(px(x))
                            .top(px(y))
                            .w(px(w))
                            .h(px(h))
                            .border_r_2()
                            .border_b_2()
                            .border_color(rgb(t.bg))
                            .bg(rgb(color))
                    })),
            )
    }
}

impl_render!(Minimap);
