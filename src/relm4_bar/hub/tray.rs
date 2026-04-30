//! StatusNotifierItem (system tray) hub.
//!
//! Architecture:
//!
//! - One singleton background OS thread (`"tray-hub"`) owns the
//!   [`system_tray::client::Client`], which transitively owns a `zbus`
//!   connection. The thread spins up a current-thread tokio runtime
//!   internally because the `system-tray` crate is async; this isolates
//!   that runtime from the app-level tokio runtime in `main.rs`.
//!
//! - Whenever the `system-tray` client emits an event, the hub thread
//!   rebuilds a complete [`TrayState`] snapshot and publishes it through a
//!   [`tokio::sync::watch`] channel. Widgets call [`subscribe`] to receive
//!   updates.
//!
//! - All data crossing the channel is `Send + Sync`. Icons are decoded
//!   from the SNI ARGB32 wire format into RGBA bytes on the hub thread,
//!   so the widget receives a [`TrayIcon`] (`Vec<u8>`) and can build a
//!   `gdk::MemoryTexture` / `gdk::Pixbuf` lazily on the GTK main thread.
//!   The menu tree is shipped as plain owned [`TrayMenuEntry`] values —
//!   the widget side rebuilds GIO `MenuModel`s on demand.
//!
//! - Widgets invoke menu items / activate the primary action by calling
//!   [`activate`] / [`invoke_menu`]. These send an internal
//!   [`TrayCommand`] over an `async_channel` to the hub thread, where the
//!   `Client` actually performs the dbus call.
//!
//! Conforms to the canonical hub pattern (`OnceLock<watch::Sender<_>>`,
//! a single named `std::thread`, lazy spawn on first `subscribe()`).

use std::sync::OnceLock;

use tokio::sync::watch;

use system_tray::client::{ActivateRequest, Client, Event, UpdateEvent};
use system_tray::item::{IconPixmap, StatusNotifierItem};
use system_tray::menu::{MenuItem, MenuType, TrayMenu};

// -----------------------------------------------------------------------------
// Public types
// -----------------------------------------------------------------------------

/// Snapshot of all currently-known StatusNotifierItem entries.
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct TrayState {
    pub items: Vec<TrayItem>,
}

/// A single tray item. Mirrors the parts of
/// [`system_tray::item::StatusNotifierItem`] the widget actually consumes,
/// but with all icon data pre-decoded into RGBA `Vec<u8>` so the type is
/// `Send + Sync`.
#[allow(dead_code)]
#[derive(Clone)]
pub struct TrayItem {
    /// Stable ID — the SNI bus name (e.g. `:1.42`). This is also the
    /// `address` parameter of `ActivateRequest`.
    pub id: String,
    /// Application-supplied id (e.g. `"fcitx"`). Matches the GPUI rs-bar's
    /// `TrayItem.id` and is the value widgets typically filter on.
    pub app_id: String,
    /// Optional human-readable title from the SNI properties.
    pub title: Option<String>,
    /// Freedesktop-compliant icon name, if the application provides one.
    pub icon_name: Option<String>,
    /// Optional theme-search path supplied by the application.
    pub icon_theme_path: Option<String>,
    /// Largest pixmap (ARGB32 → RGBA) supplied by the application, if any.
    pub icon: Option<TrayIcon>,
    /// Pre-decoded "attention" icon (used for `NeedsAttention` status).
    pub attention_icon: Option<TrayIcon>,
    /// Optional tooltip text (`tool_tip.title` falls back to `tool_tip.description`).
    pub tooltip: Option<String>,
    /// Path of the `com.canonical.dbusmenu` object, if the item has a menu.
    /// Required to invoke menu items via [`invoke_menu`].
    pub menu_path: Option<String>,
    /// Flattened menu tree (without invisible entries).
    pub menu: Vec<TrayMenuEntry>,
}

/// Pre-decoded RGBA icon. Width × height × 4 == `rgba.len()`.
#[allow(dead_code)]
#[derive(Clone)]
pub struct TrayIcon {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

/// A node in a tray item's menu tree.
#[allow(dead_code)]
#[derive(Clone)]
pub enum TrayMenuEntry {
    Item {
        /// dbus-menu numeric id, used as `submenu_id` in `ActivateRequest::MenuItem`.
        id: i32,
        label: String,
        enabled: bool,
    },
    Submenu {
        label: String,
        children: Vec<TrayMenuEntry>,
    },
    Separator,
}

// -----------------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------------

/// Subscribe to tray state updates. Lazily spawns the hub thread on first call.
#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<TrayState> {
    sender().subscribe()
}

/// Send an `Activate` request (the SNI primary action) for an item by id.
///
/// `id` is the [`TrayItem::id`] (SNI bus name). `x`/`y` are screen-space
/// coordinate hints; pass `(0, 0)` if not relevant.
#[allow(dead_code)]
pub fn activate(id: &str, x: i32, y: i32) {
    let _ = sender(); // ensure the listener is running
    if let Some(tx) = command_sender().get() {
        let _ = tx.try_send(TrayCommand::Activate {
            address: id.to_string(),
            x,
            y,
        });
    }
}

/// Send a `Secondary` request for an item — typically right-click.
#[allow(dead_code)]
pub fn secondary(id: &str, x: i32, y: i32) {
    let _ = sender();
    if let Some(tx) = command_sender().get() {
        let _ = tx.try_send(TrayCommand::Secondary {
            address: id.to_string(),
            x,
            y,
        });
    }
}

/// Click a menu item belonging to the tray item with the given `id`.
///
/// `menu_entry_id` is the [`TrayMenuEntry::Item::id`] (the dbus-menu numeric id).
#[allow(dead_code)]
pub fn invoke_menu(id: &str, menu_entry_id: i32) {
    let _ = sender();
    if let Some(tx) = command_sender().get() {
        let _ = tx.try_send(TrayCommand::InvokeMenu {
            address: id.to_string(),
            menu_entry_id,
        });
    }
}

// -----------------------------------------------------------------------------
// Internals
// -----------------------------------------------------------------------------

/// Commands sent from widget threads to the hub thread.
enum TrayCommand {
    Activate {
        address: String,
        x: i32,
        y: i32,
    },
    Secondary {
        address: String,
        x: i32,
        y: i32,
    },
    InvokeMenu {
        address: String,
        menu_entry_id: i32,
    },
}

fn command_sender() -> &'static OnceLock<async_channel::Sender<TrayCommand>> {
    static C: OnceLock<async_channel::Sender<TrayCommand>> = OnceLock::new();
    &C
}

fn sender() -> &'static watch::Sender<TrayState> {
    static S: OnceLock<watch::Sender<TrayState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(TrayState::default());
        let producer = tx.clone();

        // Set up the command channel up front so [`activate`] /
        // [`invoke_menu`] called *during* hub startup don't get dropped.
        let (cmd_tx, cmd_rx) = async_channel::unbounded::<TrayCommand>();
        let _ = command_sender().set(cmd_tx);

        std::thread::Builder::new()
            .name("tray-hub".into())
            .spawn(move || listener(producer, cmd_rx))
            .ok();

        tx
    })
}

/// Hub thread entry point. Builds a current-thread tokio runtime (the
/// `system-tray` crate requires tokio) and drives the event/command loop.
fn listener(tx: watch::Sender<TrayState>, cmd_rx: async_channel::Receiver<TrayCommand>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!("tray-hub: failed to build runtime: {e}");
            return;
        }
    };

    rt.block_on(async move {
        let client = match Client::new().await {
            Ok(c) => c,
            Err(e) => {
                log::error!("tray-hub: failed to create system-tray client: {e}");
                return;
            }
        };

        let mut events = client.subscribe();
        let items_map = client.items();

        // Push the initial (likely-empty) snapshot so subscribers don't
        // sit on `Default::default()` if no items exist.
        publish(&tx, &items_map);

        loop {
            tokio::select! {
                ev = events.recv() => {
                    match ev {
                        Ok(Event::Add(_, _))
                        | Ok(Event::Remove(_))
                        | Ok(Event::Update(_, UpdateEvent::Icon { .. }))
                        | Ok(Event::Update(_, UpdateEvent::AttentionIcon(_)))
                        | Ok(Event::Update(_, UpdateEvent::Title(_)))
                        | Ok(Event::Update(_, UpdateEvent::Tooltip(_)))
                        | Ok(Event::Update(_, UpdateEvent::Status(_)))
                        | Ok(Event::Update(_, UpdateEvent::OverlayIcon(_)))
                        | Ok(Event::Update(_, UpdateEvent::Menu(_)))
                        | Ok(Event::Update(_, UpdateEvent::MenuDiff(_)))
                        | Ok(Event::Update(_, UpdateEvent::MenuConnect(_))) => {
                            publish(&tx, &items_map);
                        }
                        Err(e) => {
                            log::error!("tray-hub: event channel error: {e}");
                            break;
                        }
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Ok(TrayCommand::Activate { address, x, y }) => {
                            if let Err(e) = client.activate(ActivateRequest::Default {
                                address,
                                x,
                                y,
                            }).await {
                                log::warn!("tray-hub: activate failed: {e}");
                            }
                        }
                        Ok(TrayCommand::Secondary { address, x, y }) => {
                            if let Err(e) = client.activate(ActivateRequest::Secondary {
                                address,
                                x,
                                y,
                            }).await {
                                log::warn!("tray-hub: secondary failed: {e}");
                            }
                        }
                        Ok(TrayCommand::InvokeMenu { address, menu_entry_id }) => {
                            // Look up the menu_path from the cached items.
                            let menu_path = items_map
                                .lock()
                                .ok()
                                .and_then(|m| m.get(&address).and_then(|(item, _)| item.menu.clone()));
                            let Some(menu_path) = menu_path else {
                                log::warn!(
                                    "tray-hub: invoke_menu({address}, {menu_entry_id}) — no menu path"
                                );
                                continue;
                            };
                            if let Err(e) = client.activate(ActivateRequest::MenuItem {
                                address,
                                menu_path,
                                submenu_id: menu_entry_id,
                            }).await {
                                log::warn!("tray-hub: invoke_menu failed: {e}");
                            }
                        }
                        Err(e) => {
                            log::error!("tray-hub: command channel closed: {e}");
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Re-snapshot the cached items map and broadcast a new [`TrayState`].
fn publish(
    tx: &watch::Sender<TrayState>,
    items_map: &std::sync::Arc<std::sync::Mutex<system_tray::data::BaseMap>>,
) {
    let map = match items_map.lock() {
        Ok(m) => m,
        Err(e) => {
            log::error!("tray-hub: items map poisoned: {e}");
            return;
        }
    };

    let mut items: Vec<TrayItem> = map
        .iter()
        .map(|(address, (sni, menu))| build_item(address, sni, menu.as_ref()))
        .collect();

    // Stable ordering so subscribers see deterministic widget order.
    items.sort_by(|a, b| a.app_id.cmp(&b.app_id).then(a.id.cmp(&b.id)));

    let _ = tx.send(TrayState { items });
}

fn build_item(address: &str, sni: &StatusNotifierItem, menu: Option<&TrayMenu>) -> TrayItem {
    let icon = sni.icon_pixmap.as_deref().and_then(decode_largest);
    let attention_icon = sni.attention_icon_pixmap.as_deref().and_then(decode_largest);

    let tooltip = sni.tool_tip.as_ref().and_then(|t| {
        if !t.title.is_empty() {
            Some(t.title.clone())
        } else if !t.description.is_empty() {
            Some(t.description.clone())
        } else {
            None
        }
    });

    let menu_entries = menu
        .map(|m| convert_menu_entries(&m.submenus))
        .unwrap_or_default();

    TrayItem {
        id: address.to_string(),
        app_id: sni.id.clone(),
        title: sni.title.clone(),
        icon_name: sni.icon_name.clone(),
        icon_theme_path: sni.icon_theme_path.clone(),
        icon,
        attention_icon,
        tooltip,
        menu_path: sni.menu.clone(),
        menu: menu_entries,
    }
}

/// SNI ships icons in ARGB32 (network byte order, i.e. big-endian: A, R, G, B).
/// GTK / GDK expect RGBA. Reorder bytes here on the hub thread.
fn decode_largest(pixmaps: &[IconPixmap]) -> Option<TrayIcon> {
    let pixmap = pixmaps
        .iter()
        .filter(|p| p.width > 0 && p.height > 0)
        .max_by_key(|p| p.width * p.height)?;

    let expected = (pixmap.width as usize) * (pixmap.height as usize) * 4;
    if pixmap.pixels.len() < expected {
        log::warn!(
            "tray-hub: pixmap {}x{} has {} bytes, expected {}",
            pixmap.width,
            pixmap.height,
            pixmap.pixels.len(),
            expected
        );
        return None;
    }

    let mut rgba = Vec::with_capacity(expected);
    for chunk in pixmap.pixels[..expected].chunks_exact(4) {
        // chunk = [A, R, G, B] → RGBA = [R, G, B, A]
        rgba.push(chunk[1]);
        rgba.push(chunk[2]);
        rgba.push(chunk[3]);
        rgba.push(chunk[0]);
    }

    Some(TrayIcon {
        width: pixmap.width,
        height: pixmap.height,
        rgba,
    })
}

fn convert_menu_entries(items: &[MenuItem]) -> Vec<TrayMenuEntry> {
    items
        .iter()
        .filter(|m| m.visible)
        .map(convert_menu_entry)
        .collect()
}

fn convert_menu_entry(item: &MenuItem) -> TrayMenuEntry {
    if matches!(item.menu_type, MenuType::Separator) {
        return TrayMenuEntry::Separator;
    }

    let label = item.label.clone().unwrap_or_default();

    if !item.submenu.is_empty()
        || item
            .children_display
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("submenu"))
            .unwrap_or(false)
    {
        TrayMenuEntry::Submenu {
            label,
            children: convert_menu_entries(&item.submenu),
        }
    } else {
        TrayMenuEntry::Item {
            id: item.id,
            label,
            enabled: item.enabled,
        }
    }
}
