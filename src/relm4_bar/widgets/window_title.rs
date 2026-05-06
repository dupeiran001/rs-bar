//! Window title widget. Subscribes to the niri hub and renders an app icon
//! plus the title of the focused window on this bar's monitor.
//!
//! Icon lookup mirrors the GPUI version: try linicon (theme inheritance +
//! hicolor fallback), then read `.desktop`'s `Icon=` field, then probe
//! hicolor/scalable and /usr/share/pixmaps directly. The relevant crate
//! is `linicon` (already a dep) — GTK's own `IconTheme` is also an option,
//! but linicon handles the .desktop-file dance cleanly and we already use
//! it in the GPUI backend.
//!
//! Per-monitor scoping: each bar shows the focused window living on its own
//! monitor's active workspace.

use std::path::PathBuf;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule};

pub struct WindowTitle {
    /// Connector name (e.g. "DP-2") captured from `BAR_CTX` in `init`.
    connector: String,
    /// Last-rendered (app_id, title) pair — coalesces redundant repaints.
    last_app_id: String,
    last_title: String,
    /// Last successfully resolved icon path. Cached so we don't re-probe
    /// linicon on every snapshot — only when app_id changes.
    last_icon_path: Option<PathBuf>,
    /// Root box, held so `update` can hide the entire capsule when there's
    /// no focused window to show.
    root: gtk::Box,
    icon: gtk::Image,
    label: gtk::Label,
}

pub enum WindowTitleMsg {
    Update(hub::niri::NiriSnapshot),
}

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
            set_spacing: 6,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_pixel_size: config::ICON_SIZE() as i32,
                set_visible: false,
            },
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
            last_app_id: String::new(),
            last_title: String::new(),
            last_icon_path: None,
            root: root.clone(),
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, init.grouped);
        // Start hidden; un-hide on the first non-empty title.
        root.set_visible(false);

        crate::subscribe_into_msg!(hub::niri::subscribe(), sender, WindowTitleMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WindowTitleMsg::Update(snapshot) => {
                let active_ws = snapshot
                    .workspaces
                    .iter()
                    .find(|ws| ws.is_active && ws.output.as_deref() == Some(&self.connector));

                let focused = active_ws.and_then(|ws| {
                    if let Some(id) = ws.active_window_id {
                        snapshot.windows.iter().find(|w| w.id == id)
                    } else {
                        snapshot
                            .windows
                            .iter()
                            .find(|w| w.workspace_id == Some(ws.id) && w.is_focused)
                    }
                });

                let new_title = focused.and_then(|w| w.title.clone()).unwrap_or_default();
                let new_app_id = focused.and_then(|w| w.app_id.clone()).unwrap_or_default();

                // Coalesce: nothing changed, do nothing.
                if new_title == self.last_title && new_app_id == self.last_app_id {
                    return;
                }

                // Re-resolve the icon only when the app_id actually changes.
                if new_app_id != self.last_app_id {
                    self.last_icon_path = lookup_icon(&new_app_id);
                    apply_icon(&self.icon, self.last_icon_path.as_deref());
                }
                self.last_app_id = new_app_id;

                if new_title != self.last_title {
                    self.last_title = new_title;
                    self.label.set_label(&self.last_title);
                }

                // Show/hide the whole capsule based on whether there's
                // anything to render. An empty title with no icon means no
                // focused window.
                let has_content = !self.last_title.is_empty() || self.last_icon_path.is_some();
                self.root.set_visible(has_content);
            }
        }
    }
}

/// Apply a resolved icon path to the gtk::Image. Falls back to hidden when
/// `path` is None or load fails.
fn apply_icon(image: &gtk::Image, path: Option<&std::path::Path>) {
    if let Some(path) = path {
        match gdk::Texture::from_filename(path) {
            Ok(tex) => {
                image.set_paintable(Some(&tex));
                image.set_visible(true);
                return;
            }
            Err(e) => {
                log::debug!("window_title: failed to load icon {}: {e}", path.display());
            }
        }
    }
    image.set_paintable(None::<&gdk::Paintable>);
    image.set_visible(false);
}

/// Look up an app icon via linicon (theme + fallback), .desktop files, and
/// pixmaps. Mirrors the GPUI version's resolution order so both backends
/// pick the same icon.
fn lookup_icon(app_id: &str) -> Option<PathBuf> {
    if app_id.is_empty() {
        return None;
    }
    let theme = config::ICON_THEME();
    let short = app_id.rsplit('.').next().unwrap_or(app_id);
    let candidates = [
        app_id.to_string(),
        short.to_string(),
        app_id.to_lowercase(),
        short.to_lowercase(),
    ];

    // 1. linicon (icon-theme inheritance + hicolor fallback)
    for name in &candidates {
        if let Some(Ok(icon)) = linicon::lookup_icon(name)
            .from_theme(theme)
            .use_fallback_themes(true)
            .next()
        {
            return Some(icon.path);
        }
    }

    // 2. .desktop file's Icon= field
    if let Some(icon_name) = desktop_icon_name(app_id) {
        if icon_name.starts_with('/') {
            let p = PathBuf::from(&icon_name);
            if p.exists() {
                return Some(p);
            }
        }
        if let Some(Ok(icon)) = linicon::lookup_icon(&icon_name)
            .from_theme(theme)
            .use_fallback_themes(true)
            .next()
        {
            return Some(icon.path);
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

/// Read the `Icon=` field from a .desktop file matching the app_id.
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

impl NamedWidget for WindowTitle {
    const NAME: &'static str = "window-title";
}
