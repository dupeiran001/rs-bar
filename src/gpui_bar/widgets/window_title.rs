use std::path::PathBuf;

use gpui::{
    Context, IntoElement, ParentElement, Styled, Window, div, img, px, rgb, svg,
    prelude::FluentBuilder,
};

use super::{BarWidget, impl_render};

pub struct WindowTitle {
    title: String,
    app_id: String,
    icon_path: Option<PathBuf>,
}

/// Look up an app icon via linicon (theme + fallback), .desktop files, and pixmaps.
fn lookup_icon(app_id: &str) -> Option<PathBuf> {
    let theme = crate::gpui_bar::config::ICON_THEME();
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
        // Subscribe to the shared niri hub: extract focused window from each
        // snapshot. No own thread, no polling.
        let sub = crate::gpui_bar::niri::broadcast().subscribe();
        cx.spawn(async move |this, cx| {
            let mut last_app_id = String::new();
            let mut last_icon: Option<PathBuf> = None;
            while let Some(snap) = sub.next().await {
                let focused = snap.windows.iter().find(|w| w.is_focused);
                let title = focused.and_then(|w| w.title.clone()).unwrap_or_default();
                let app_id = focused.and_then(|w| w.app_id.clone()).unwrap_or_default();

                // Only re-lookup the icon when the app_id actually changes.
                let icon_path = if app_id != last_app_id {
                    last_app_id = app_id.clone();
                    last_icon = lookup_icon(&app_id);
                    last_icon.clone()
                } else {
                    last_icon.clone()
                };

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
        })
        .detach();

        Self {
            title: String::new(),
            app_id: String::new(),
            icon_path: None,
        }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let icon_size = crate::gpui_bar::config::ICON_SIZE();

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
