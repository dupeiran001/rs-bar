use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    Context, IntoElement, ParentElement, Styled, Window, div, img, px, rgb, svg,
    prelude::FluentBuilder,
};
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Request, Response};

use super::{BarWidget, impl_render};

pub struct WindowTitle {
    title: String,
    app_id: String,
    icon_path: Option<PathBuf>,
}

struct SharedState {
    title: String,
    app_id: String,
}

/// Look up an app icon via linicon (theme + fallback), .desktop files, and pixmaps.
fn lookup_icon(app_id: &str) -> Option<PathBuf> {
    let theme = crate::config::ICON_THEME();
    let short = app_id.rsplit('.').next().unwrap_or(app_id);
    let candidates = [
        app_id.to_string(),
        short.to_string(),
        app_id.to_lowercase(),
        short.to_lowercase(),
    ];

    // 1. Try icon theme via linicon (handles theme inheritance + hicolor fallback)
    // Don't restrict size — icons scale fine, and many apps only ship large sizes.
    for name in &candidates {
        if let Some(icon) = linicon::lookup_icon(name)
            .from_theme(theme)
            .use_fallback_themes(true)
            .next()
        {
            if let Ok(icon) = icon {
                return Some(icon.path);
            }
        }
    }

    // 2. Try .desktop file → real icon name → linicon + pixmaps
    if let Some(icon_name) = desktop_icon_name(app_id) {
        if icon_name.starts_with('/') {
            let p = PathBuf::from(&icon_name);
            if p.exists() {
                return Some(p);
            }
        }
        if let Some(icon) = linicon::lookup_icon(&icon_name)
            .from_theme(theme)
            .use_fallback_themes(true)
            .next()
        {
            if let Ok(icon) = icon {
                return Some(icon.path);
            }
        }
        for ext in ["png", "svg", "xpm"] {
            let p = PathBuf::from(format!("/usr/share/pixmaps/{icon_name}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
    }

    // 3. Direct hicolor scalable check (linicon may miss these)
    for name in &candidates {
        let p = PathBuf::from(format!("/usr/share/icons/hicolor/scalable/apps/{name}.svg"));
        if p.exists() {
            return Some(p);
        }
    }

    // 4. Pixmaps fallback
    for name in &candidates {
        for ext in ["png", "svg", "xpm"] {
            let p = PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

/// Read the Icon= field from a .desktop file matching the app_id.
fn desktop_icon_name(app_id: &str) -> Option<String> {
    let short = app_id.rsplit('.').next().unwrap_or(app_id);
    let candidates = [
        format!("{app_id}.desktop"),
        format!("{short}.desktop"),
        format!("{}.desktop", app_id.to_lowercase()),
        format!("{}.desktop", short.to_lowercase()),
    ];

    let dirs = [
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    for dir in &dirs {
        for name in &candidates {
            let path = dir.join(name);
            if let Ok(contents) = std::fs::read_to_string(&path) {
                for line in contents.lines() {
                    if let Some(icon) = line.strip_prefix("Icon=") {
                        return Some(icon.trim().to_string());
                    }
                }
            }
        }
    }

    None
}

impl BarWidget for WindowTitle {
    const NAME: &str = "window_title";

    fn new(cx: &mut Context<Self>) -> Self {
        let shared = Arc::new(Mutex::new(SharedState {
            title: String::new(),
            app_id: String::new(),
        }));
        let dirty = Arc::new(AtomicBool::new(false));

        let ev_shared = shared.clone();
        let ev_dirty = dirty.clone();
        std::thread::spawn(move || {
            let Ok(mut socket) = Socket::connect() else {
                log::error!("window_title: failed to connect to niri socket");
                return;
            };

            // Get initial focused window
            if let Ok(Ok(Response::Windows(windows))) = socket.send(Request::Windows) {
                if let Some(win) = windows.iter().find(|w| w.is_focused) {
                    let mut state = ev_shared.lock().unwrap();
                    state.title = win.title.clone().unwrap_or_default();
                    state.app_id = win.app_id.clone().unwrap_or_default();
                    ev_dirty.store(true, Ordering::Release);
                }
            }

            // Event stream
            let Ok(mut socket) = Socket::connect() else { return };
            let Ok(Ok(Response::Handled)) = socket.send(Request::EventStream) else { return };

            let mut read_event = socket.read_events();
            let mut windows: Vec<niri_ipc::Window> = Vec::new();

            loop {
                match read_event() {
                    Ok(event) => {
                        match event {
                            Event::WindowsChanged { windows: ws } => {
                                windows = ws;
                            }
                            Event::WindowOpenedOrChanged { window } => {
                                // If this window is focused, clear focus on all others
                                if window.is_focused {
                                    for w in &mut windows {
                                        w.is_focused = false;
                                    }
                                }
                                windows.retain(|w| w.id != window.id);
                                windows.push(window);
                            }
                            Event::WindowClosed { id } => {
                                windows.retain(|w| w.id != id);
                            }
                            Event::WindowFocusChanged { id } => {
                                for w in &mut windows {
                                    w.is_focused = Some(w.id) == id;
                                }
                            }
                            _ => continue,
                        }

                        let focused = windows.iter().find(|w| w.is_focused);
                        let mut state = ev_shared.lock().unwrap();
                        state.title = focused
                            .and_then(|w| w.title.clone())
                            .unwrap_or_default();
                        state.app_id = focused
                            .and_then(|w| w.app_id.clone())
                            .unwrap_or_default();
                        ev_dirty.store(true, Ordering::Release);
                    }
                    Err(e) => {
                        log::error!("window_title: event stream error: {e}");
                        break;
                    }
                }
            }
        });

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
                    let title = state.title.clone();
                    let app_id = state.app_id.clone();
                    drop(state);

                    // Icon lookup (blocking but fast — cached by OS)
                    let icon_path = lookup_icon(&app_id);

                    if this
                        .update(cx, |this, cx| {
                            this.title = title;
                            this.app_id = app_id;
                            this.icon_path = icon_path;
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
            title: String::new(),
            app_id: String::new(),
            icon_path: None,
        }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let content_h = crate::config::CONTENT_HEIGHT();
        let icon_size = crate::config::ICON_SIZE();

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .h(px(content_h))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_xs()
            .pl_2()
            .when(!self.title.is_empty(), |el| {
                let el = if let Some(path) = &self.icon_path {
                    let path_str: String = path.to_string_lossy().into();
                    if path_str.ends_with(".svg") {
                        el.child(
                            svg()
                                .external_path(path_str)
                                .size(px(icon_size))
                                .flex_shrink_0(),
                        )
                    } else {
                        el.child(
                            img(path.clone())
                                .w(px(icon_size))
                                .h(px(icon_size))
                                .flex_shrink_0(),
                        )
                    }
                } else {
                    el
                };
                el.child(
                    div()
                        .text_color(rgb(t.text_dim))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(self.title.clone()),
                )
            })
    }
}

impl_render!(WindowTitle);
