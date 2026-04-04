use std::sync::{Arc, Mutex, OnceLock};

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AppContext as _, Context, ElementId, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, img, px,
    rgb, svg,
};

use super::{BarWidget, impl_render};

#[derive(Clone)]
pub(super) struct TrayItem {
    pub id: String,
    pub title: Option<String>,
    pub icon_name: Option<String>,
    pub icon_theme_path: Option<String>,
    pub icon_pixmap_path: Option<std::path::PathBuf>,
}

pub(super) struct TrayServer {
    pub(super) items: Arc<Mutex<Vec<TrayItem>>>,
    /// Subscriber list — each consumer gets its own sender for event-driven updates.
    subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>>,
}

/// Save ARGB32 pixmap as a PNG temp file for GPUI to render.
fn save_pixmap_to_temp(
    id: &str,
    pixmaps: &[system_tray::item::IconPixmap],
) -> Option<std::path::PathBuf> {
    let pixmap = pixmaps.iter().max_by_key(|p| p.width)?;
    let w = pixmap.width as u32;
    let h = pixmap.height as u32;
    if w == 0 || h == 0 {
        return None;
    }

    let mut rgba = Vec::with_capacity(pixmap.pixels.len());
    for chunk in pixmap.pixels.chunks(4) {
        if chunk.len() == 4 {
            rgba.push(chunk[1]);
            rgba.push(chunk[2]);
            rgba.push(chunk[3]);
            rgba.push(chunk[0]);
        }
    }

    let dir = std::env::temp_dir().join("rs-bar-tray");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{id}.png"));

    let file = std::fs::File::create(&path).ok()?;
    let writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&rgba).ok()?;

    Some(path)
}

pub(super) fn tray_server() -> &'static TrayServer {
    static SERVER: OnceLock<TrayServer> = OnceLock::new();
    SERVER.get_or_init(|| {
        let items: Arc<Mutex<Vec<TrayItem>>> = Arc::new(Mutex::new(Vec::new()));
        let subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let ev_items = items.clone();
        let ev_subs = subscribers.clone();

        std::thread::Builder::new()
            .name("tray-server".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create tokio runtime for tray");

                rt.block_on(async {
                    let client = match system_tray::client::Client::new().await {
                        Ok(c) => c,
                        Err(e) => {
                            log::error!("tray: failed to create client: {e:?}");
                            return;
                        }
                    };

                    let mut rx = client.subscribe();
                    let items_map = client.items();

                    let rebuild = |map: &system_tray::data::BaseMap| -> Vec<TrayItem> {
                        map.values()
                            .map(|(item, _menu)| {
                                let pixmap_path = item
                                    .icon_pixmap
                                    .as_ref()
                                    .and_then(|p| save_pixmap_to_temp(&item.id, p));

                                TrayItem {
                                    id: item.id.clone(),
                                    title: item.title.clone(),
                                    icon_name: item.icon_name.clone(),
                                    icon_theme_path: item.icon_theme_path.clone(),
                                    icon_pixmap_path: pixmap_path,
                                }
                            })
                            .collect()
                    };

                    {
                        let map = items_map.lock().unwrap();
                        *ev_items.lock().unwrap() = rebuild(&map);
                        {
                                        let mut subs = ev_subs.lock().unwrap();
                                        subs.retain(|tx| !tx.is_closed());
                                        for tx in subs.iter() {
                                            let _ = tx.try_send(());
                                        }
                                    }
                    }

                    loop {
                        match rx.recv().await {
                            Ok(_event) => {
                                let map = items_map.lock().unwrap();
                                *ev_items.lock().unwrap() = rebuild(&map);
                                {
                                        let mut subs = ev_subs.lock().unwrap();
                                        subs.retain(|tx| !tx.is_closed());
                                        for tx in subs.iter() {
                                            let _ = tx.try_send(());
                                        }
                                    }
                            }
                            Err(e) => {
                                log::error!("tray: event error: {e}");
                                break;
                            }
                        }
                    }
                });
            })
            .expect("failed to spawn tray thread");

        TrayServer { items, subscribers }
    })
}

/// Create a new receiver for tray update notifications (event-driven).
pub(super) fn subscribe_tray() -> async_channel::Receiver<()> {
    let server = tray_server();
    let (tx, rx) = async_channel::bounded(1);
    server.subscribers.lock().unwrap().push(tx);
    rx
}

struct TrayTooltip {
    text: String,
}

impl Render for TrayTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        div()
            .bg(rgb(t.surface))
            .border_1()
            .border_color(rgb(t.border))
            .rounded_md()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(t.fg))
            .child(self.text.clone())
    }
}

pub struct Tray {
    items: Vec<TrayItem>,
    expanded: bool,
    ever_expanded: bool,
}

impl BarWidget for Tray {
    const NAME: &str = "tray";

    fn new(cx: &mut Context<Self>) -> Self {
        let server_items = tray_server().items.clone();
        let rx = subscribe_tray();

        // Load current items immediately (don't wait for first event)
        let initial_items = server_items.lock().unwrap().clone();

        // Then await future events
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let new_items = server_items.lock().unwrap().clone();

                if this
                    .update(cx, |this, cx| {
                        this.items = new_items;
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
            items: initial_items,
            expanded: false,
            ever_expanded: false,
        }
    }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let icon_size = crate::config::ICON_SIZE;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let tray_icon_size: f32 = 12.0; // smaller to fit inside capsule
        let expanded = self.expanded;
        let animate = self.ever_expanded;

        // Filter out Fcitx — it has its own dedicated widget
        let items: Vec<_> = self
            .items
            .iter()
            .filter(|item| !item.id.eq_ignore_ascii_case("fcitx"))
            .collect();

        let n_items = items.len();

        // Match Volume capsule sizing: collapsed = icon + padding
        let collapsed_w = icon_size + 8.0;
        let expanded_w =
            collapsed_w + (n_items as f32) * (tray_icon_size + 4.0) + 4.0;

        // Build icon row
        let icons = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .pl(px(4.0))
            .children(items.into_iter().map(|item| {
                let id = ElementId::Name(format!("tray-{}", item.id).into());
                let resolved = resolve_icon(item);

                let icon_el: gpui::AnyElement = if let Some(icon_path) = resolved {
                    let path_str = icon_path.to_string_lossy().to_string();
                    if path_str.ends_with(".svg") {
                        svg()
                            .external_path(path_str)
                            .size(px(tray_icon_size))
                            .text_color(rgb(t.fg))
                            .into_any_element()
                    } else {
                        img(icon_path)
                            .w(px(tray_icon_size))
                            .h(px(tray_icon_size))
                            .into_any_element()
                    }
                } else {
                    div()
                        .w(px(tray_icon_size * 0.5))
                        .h(px(tray_icon_size * 0.5))
                        .rounded_full()
                        .bg(rgb(t.accent))
                        .into_any_element()
                };

                let tooltip_text = item
                    .title
                    .clone()
                    .unwrap_or_else(|| item.id.clone());

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .tooltip(move |_window, cx| {
                        cx.new(|_| TrayTooltip {
                            text: tooltip_text.clone(),
                        })
                        .into()
                    })
                    .child(icon_el)
            }));

        let arrow_path = if expanded {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tray-arrow.svg")
        } else {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tray-arrow-left.svg")
        };

        let content = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .whitespace_nowrap()
            // Center the arrow in the collapsed_w area
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(collapsed_w))
                    .flex_shrink_0()
                    .child(
                        svg()
                            .external_path(arrow_path.to_string())
                            .size(px(icon_size))
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0(),
                    ),
            )
            .child(icons);

        fn ease_expand(t: f32) -> f32 {
            1.0 - (-(10.0 * t)).exp2()
        }

        fn ease_collapse(t: f32) -> f32 {
            let t2 = t * t;
            t2 / (2.0 * (t2 - t) + 1.0)
        }

        let entity = cx.weak_entity();

        div()
            .id("tray")
            .flex()
            .items_center()
            .h(px(button_h))
            .overflow_hidden()
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_dark)))
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                let _ = entity.update(cx, |this, cx| {
                    this.expanded = !this.expanded;
                    this.ever_expanded = true;
                    cx.notify();
                });
            })
            .child(content)
            .with_animation(
                if expanded { "tray-expand" } else { "tray-collapse" },
                Animation::new(Duration::from_millis(if expanded { 400 } else { 300 }))
                    .with_easing(if expanded { ease_expand } else { ease_collapse }),
                move |el, progress| {
                    let target = if expanded { expanded_w } else { collapsed_w };
                    let from = if !animate {
                        target
                    } else if expanded {
                        collapsed_w
                    } else {
                        expanded_w
                    };
                    let w = from + (target - from) * progress;

                    let border_from: gpui::Hsla = rgb(t.border).into();
                    let border_to: gpui::Hsla = rgb(t.accent_dim).into();
                    let p = if expanded { progress } else { 1.0 - progress };
                    let mut blended = border_to;
                    blended.a = if animate { p } else { 0.0 };
                    let border = border_from.blend(blended);

                    el.w(px(w)).border_color(border)
                },
            )
    }
}

fn resolve_icon(item: &TrayItem) -> Option<std::path::PathBuf> {
    if let Some(path) = &item.icon_pixmap_path {
        if path.exists() {
            return Some(path.clone());
        }
    }

    let theme = crate::config::ICON_THEME;

    if let Some(name) = &item.icon_name {
        if !name.is_empty() {
            if name.starts_with('/') {
                let p = std::path::PathBuf::from(name);
                if p.exists() {
                    return Some(p);
                }
            }

            if let Some(theme_path) = &item.icon_theme_path {
                if !theme_path.is_empty() {
                    for ext in ["svg", "png"] {
                        for category in ["apps", "status"] {
                            let p = std::path::PathBuf::from(format!(
                                "{theme_path}/hicolor/scalable/{category}/{name}.{ext}"
                            ));
                            if p.exists() {
                                return Some(p);
                            }
                            for size in ["48x48", "32x32", "24x24", "22x22", "16x16"] {
                                let p = std::path::PathBuf::from(format!(
                                    "{theme_path}/hicolor/{size}/{category}/{name}.{ext}"
                                ));
                                if p.exists() {
                                    return Some(p);
                                }
                            }
                        }
                    }
                }
            }

            if let Some(icon) = linicon::lookup_icon(name)
                .from_theme(theme)
                .use_fallback_themes(true)
                .next()
            {
                if let Ok(icon) = icon {
                    return Some(icon.path);
                }
            }

            for category in ["apps", "status"] {
                let p = std::path::PathBuf::from(format!(
                    "/usr/share/icons/hicolor/scalable/{category}/{name}.svg"
                ));
                if p.exists() {
                    return Some(p);
                }
            }

            for ext in ["png", "svg"] {
                let p = std::path::PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    None
}

impl_render!(Tray);
