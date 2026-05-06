//! StatusNotifierItem (system tray) widget.
//!
//! Subscribes to [`hub::tray`] and renders one clickable icon per tray item
//! inside a single capsule. Each icon button:
//!
//! * Left-click → calls [`hub::tray::activate`] (the SNI primary action).
//! * Right-click → opens a `gtk::PopoverMenu` built from the item's menu
//!   tree. Menu activation routes back through [`hub::tray::invoke_menu`]
//!   via per-button `gio::SimpleAction`s.
//!
//! On every `TrayState` update we diff against the previous snapshot by
//! `id` so unchanged icons are *not* rebuilt — this matters because the
//! hub re-publishes a fresh snapshot every time any one item changes
//! (icon, title, menu, …) and rebuilding GTK widgets per tick would
//! noticeably hitch the bar.
//!
//! When `state.items` is empty the whole capsule is hidden so the bar
//! collapses cleanly (matches the GPUI rs-bar UX, which animates to
//! arrow-only width — here we just hide outright since GTK doesn't have
//! the same animation harness).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::tray::{TrayItem, TrayMenuEntry, TrayState};

use super::{NamedWidget, WidgetInit, capsule, popover};

/// Per-icon UI state retained across updates so we can reuse GTK widgets
/// when only metadata (e.g. tooltip text) changes.
struct IconSlot {
    button: gtk::Button,
    image: gtk::Image,
    popover: gtk::PopoverMenu,
    /// `gio::SimpleActionGroup` exposed under the `tray` action prefix on
    /// the button. Owned here so it outlives the popover's references.
    action_group: gio::SimpleActionGroup,
    /// Cached fields used for cheap diffing of the next state.
    cached: CachedIcon,
}

#[derive(Clone, PartialEq)]
struct CachedIcon {
    /// Width × height × first/last byte hash — cheap signature for the
    /// RGBA buffer. Avoids storing the whole `Vec<u8>` per icon.
    icon_sig: Option<(i32, i32, usize, u8, u8)>,
    title: Option<String>,
    /// Hash of the menu tree, so we know when to rebuild the popover.
    menu_sig: u64,
}

pub struct Tray {
    grouped: bool,
    /// Container that holds each item's `gtk::Button`. We append/remove
    /// children directly when the tray state changes.
    container: gtk::Box,
    /// Per-item slots keyed by `TrayItem::id` so we can diff updates.
    slots: HashMap<String, IconSlot>,
    /// Stable order of `id`s currently displayed, in insertion order so
    /// the bar layout doesn't reshuffle when no real change happened.
    order: Vec<String>,
}

pub enum TrayMsg {
    Update(TrayState),
}

// `TrayState` doesn't implement `Debug` (it's owned by the hub, which we
// don't modify here). Provide a minimal manual impl so relm4 can format
// the message — only the item count matters for tracing.
impl std::fmt::Debug for TrayMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrayMsg::Update(state) => f
                .debug_struct("Update")
                .field("items", &state.items.len())
                .finish(),
        }
    }
}

#[relm4::component(pub)]
impl SimpleComponent for Tray {
    type Init = WidgetInit;
    type Input = TrayMsg;
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
        let model = Tray {
            grouped: init.grouped,
            container: widgets.container.clone(),
            slots: HashMap::new(),
            order: Vec::new(),
        };

        capsule(&root, model.grouped);

        // Start hidden — once the first non-empty TrayState arrives we
        // re-show the capsule. Avoids a brief empty pill flicker on
        // startup when the SNI bus has zero items.
        root.set_visible(false);

        crate::subscribe_into_msg!(hub::tray::subscribe(), sender, TrayMsg::Update);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            TrayMsg::Update(state) => {
                self.apply(state);
            }
        }
    }
}

impl Tray {
    /// Reconcile `self.slots` and the GTK container against the new state.
    fn apply(&mut self, state: TrayState) {
        // Set capsule visibility against the parent root via container.
        // The capsule is the bar zone parent — we toggle our own
        // container visibility, the relm4 root inherits via display.
        if let Some(parent) = self.container.parent() {
            parent.set_visible(!state.items.is_empty());
        } else {
            self.container.set_visible(!state.items.is_empty());
        }

        // 1. Compute the set of incoming ids in the order they arrive.
        let new_order: Vec<String> = state.items.iter().map(|it| it.id.clone()).collect();

        // 2. Drop slots that are no longer present.
        self.slots.retain(|id, slot| {
            let keep = new_order.iter().any(|n| n == id);
            if !keep {
                self.container.remove(&slot.button);
            }
            keep
        });

        // 3. Walk the incoming list. Either reuse an existing slot
        //    (updating only the changed fields) or build a fresh one.
        for (idx, item) in state.items.iter().enumerate() {
            let new_cached = compute_cached(item);

            let needs_create = !self.slots.contains_key(&item.id);
            if needs_create {
                let slot = build_slot(item);
                self.container.append(&slot.button);
                self.slots.insert(item.id.clone(), slot);
            }

            // Borrow now that the slot is guaranteed to exist.
            let slot = self.slots.get_mut(&item.id).expect("slot present");

            // Reposition if the order changed (cheap: GTK no-op when stable).
            if self.order.get(idx) != Some(&item.id) {
                // Remove and re-append at the end. This isn't perfect
                // ordering, but the hub already produces a stable sort,
                // and re-appending in input order yields identical layout.
                self.container.remove(&slot.button);
                self.container.append(&slot.button);
            }

            // Diff vs cached and apply only the parts that changed.
            if needs_create || slot.cached.icon_sig != new_cached.icon_sig {
                update_image(&slot.image, item);
            }
            if needs_create || slot.cached.title != new_cached.title {
                slot.button
                    .set_tooltip_text(item.tooltip.as_deref().or(item.title.as_deref()));
            }
            if needs_create || slot.cached.menu_sig != new_cached.menu_sig {
                rebuild_menu(slot, item);
            }
            slot.cached = new_cached;
        }

        self.order = new_order;
    }
}

// -----------------------------------------------------------------------------
// Slot construction
// -----------------------------------------------------------------------------

fn build_slot(item: &TrayItem) -> IconSlot {
    let image = gtk::Image::new();
    image.add_css_class("tray-icon");
    image.set_pixel_size(config::ICON_SIZE() as i32);

    let button = gtk::Button::new();
    button.set_child(Some(&image));
    button.add_css_class("tray-icon-button");
    button.add_css_class("flat");
    button.set_focus_on_click(false);

    // Empty placeholder model; rebuild_menu replaces it on first apply.
    let popover = gtk::PopoverMenu::from_model(None::<&gio::Menu>);
    popover.set_parent(&button);
    popover.set_has_arrow(false);
    popover.set_offset(0, 0);
    popover.add_css_class("tray-popover");
    popover::install_motion(&popover);

    let action_group = gio::SimpleActionGroup::new();
    button.insert_action_group("tray", Some(&action_group));

    // Left-click: SNI primary activate. We use Button::connect_clicked
    // for the default left-click behaviour (handles keyboard activation
    // too) and a separate GestureClick for the right-click. The SNI
    // `x`/`y` hints are passed as `(0, 0)`: the spec treats them as
    // hints, most servers ignore them, and computing exact screen-space
    // coords on a Wayland layer-shell surface is unreliable anyway.
    {
        let id = item.id.clone();
        button.connect_clicked(move |_| {
            hub::tray::activate(&id, 0, 0);
        });
    }

    // Right-click: open the popover.
    let right = gtk::GestureClick::new();
    right.set_button(gtk::gdk::BUTTON_SECONDARY);
    let popover_for_right = popover.clone();
    let id_for_right = item.id.clone();
    right.connect_pressed(move |_, _, _, _| {
        // Inform the hub about the secondary press so apps that listen
        // for it (e.g. some volume applets) can react. The SNI spec
        // allows the server to act on either Secondary or the menu —
        // we surface both.
        hub::tray::secondary(&id_for_right, 0, 0);
        popover::toggle(&popover_for_right);
    });
    button.add_controller(right);

    IconSlot {
        button,
        image,
        popover,
        action_group,
        cached: CachedIcon {
            icon_sig: None,
            title: None,
            menu_sig: 0,
        },
    }
}

// -----------------------------------------------------------------------------
// Image construction
// -----------------------------------------------------------------------------

/// Set the button's image from the tray item. Tries the pre-decoded RGBA
/// pixmap first, then the freedesktop icon name (resolved via GTK's icon
/// theme machinery). Logs a warning and clears the image if neither path
/// works rather than panicking — broken icons are common in the wild.
fn update_image(image: &gtk::Image, item: &TrayItem) {
    if let Some(icon) = &item.icon {
        if icon.width > 0
            && icon.height > 0
            && icon.rgba.len() == (icon.width as usize) * (icon.height as usize) * 4
        {
            let bytes = glib::Bytes::from(&icon.rgba);
            let texture = gdk::MemoryTexture::new(
                icon.width,
                icon.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (icon.width * 4) as usize,
            );
            image.set_paintable(Some(&texture));
            return;
        } else {
            log::warn!(
                "tray: malformed pixmap for {}: {}x{}, {} bytes",
                item.id,
                icon.width,
                icon.height,
                icon.rgba.len()
            );
        }
    }

    if let Some(name) = item.icon_name.as_deref().filter(|s| !s.is_empty()) {
        // Absolute path → load directly.
        if name.starts_with('/') {
            match gdk::Texture::from_filename(name) {
                Ok(tex) => {
                    image.set_paintable(Some(&tex));
                    return;
                }
                Err(e) => log::warn!("tray: failed to load {name}: {e}"),
            }
        }

        // Application-supplied theme path: GTK's IconTheme respects extra
        // search paths. We add the path lazily and look up by name.
        if let Some(theme_path) = item.icon_theme_path.as_deref() {
            if !theme_path.is_empty() {
                if let Some(display) = gdk::Display::default() {
                    let theme = gtk::IconTheme::for_display(&display);
                    if !theme
                        .search_path()
                        .iter()
                        .any(|p| p.as_os_str() == theme_path)
                    {
                        theme.add_search_path(theme_path);
                    }
                }
            }
        }

        image.set_icon_name(Some(name));
        return;
    }

    // Nothing usable — clear the image so a stale icon isn't shown.
    image.clear();
}

// -----------------------------------------------------------------------------
// Menu construction
// -----------------------------------------------------------------------------

/// Replace the popover's `gio::MenuModel` and rewire the per-entry
/// `gio::SimpleAction`s under the slot's action group.
fn rebuild_menu(slot: &mut IconSlot, item: &TrayItem) {
    // Clear existing actions. `list_actions` returns the names registered
    // on the group; remove each via ActionMapExt.
    for name in slot.action_group.list_actions() {
        slot.action_group.remove_action(&name);
    }

    if item.menu.is_empty() {
        // No menu → close (if open) and replace with empty model.
        popover::popdown(&slot.popover);
        slot.popover.set_menu_model(Some(&gio::Menu::new()));
        return;
    }

    // Counter ensures unique action names across nested submenus, since
    // dbusmenu ids are not necessarily unique within a tree.
    let counter = Rc::new(RefCell::new(0u32));
    let model = build_menu_model(&item.menu, &item.id, &slot.action_group, &counter);

    slot.popover.set_menu_model(Some(&model));
}

fn build_menu_model(
    entries: &[TrayMenuEntry],
    item_id: &str,
    actions: &gio::SimpleActionGroup,
    counter: &Rc<RefCell<u32>>,
) -> gio::Menu {
    let menu = gio::Menu::new();

    // Buffer separator-delimited "sections" so they render as visual
    // groupings the way a native GIO menu would.
    let mut current = gio::Menu::new();

    for entry in entries {
        match entry {
            TrayMenuEntry::Item { id, label, enabled } => {
                let n = {
                    let mut c = counter.borrow_mut();
                    *c += 1;
                    *c
                };
                let action_name = format!("entry-{n}");
                let action = gio::SimpleAction::new(&action_name, None);
                action.set_enabled(*enabled);

                let item_id_owned = item_id.to_string();
                let menu_entry_id = *id;
                action.connect_activate(move |_, _| {
                    hub::tray::invoke_menu(&item_id_owned, menu_entry_id);
                });
                actions.add_action(&action);

                let menu_item = gio::MenuItem::new(
                    Some(&clean_label(label)),
                    Some(&format!("tray.{action_name}")),
                );
                current.append_item(&menu_item);
            }
            TrayMenuEntry::Submenu { label, children } => {
                let sub = build_menu_model(children, item_id, actions, counter);
                current.append_submenu(Some(&clean_label(label)), &sub);
            }
            TrayMenuEntry::Separator => {
                if current.n_items() > 0 {
                    menu.append_section(None, &current);
                    current = gio::Menu::new();
                }
            }
        }
    }

    if current.n_items() > 0 {
        menu.append_section(None, &current);
    }

    menu
}

/// dbusmenu labels embed `_` characters as keyboard mnemonic markers. GTK
/// menus interpret `_` the same way (the next char becomes a mnemonic),
/// but the SNI client passes them through verbatim, so the label looks
/// like `_File`. Strip a single underscore prefix to match the GPUI bar's
/// rendering. Real underscores are escaped as `__` per the dbusmenu spec.
fn clean_label(label: &str) -> String {
    // Replace `__` with a sentinel, drop remaining `_`, restore.
    let placeholder = '\u{1}';
    label
        .replace("__", &placeholder.to_string())
        .replace('_', "")
        .replace(placeholder, "_")
}

// -----------------------------------------------------------------------------
// Cached signature
// -----------------------------------------------------------------------------

fn compute_cached(item: &TrayItem) -> CachedIcon {
    let icon_sig = item.icon.as_ref().map(|i| {
        let last = i.rgba.last().copied().unwrap_or(0);
        let first = i.rgba.first().copied().unwrap_or(0);
        (i.width, i.height, i.rgba.len(), first, last)
    });
    CachedIcon {
        icon_sig,
        title: item.tooltip.clone().or_else(|| item.title.clone()),
        menu_sig: hash_menu(&item.menu),
    }
}

fn hash_menu(entries: &[TrayMenuEntry]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    fn walk<H: Hasher>(entries: &[TrayMenuEntry], h: &mut H) {
        for e in entries {
            match e {
                TrayMenuEntry::Item { id, label, enabled } => {
                    0u8.hash(h);
                    id.hash(h);
                    label.hash(h);
                    enabled.hash(h);
                }
                TrayMenuEntry::Submenu { label, children } => {
                    1u8.hash(h);
                    label.hash(h);
                    walk(children, h);
                }
                TrayMenuEntry::Separator => {
                    2u8.hash(h);
                }
            }
        }
    }
    walk(entries, &mut h);
    h.finish()
}

impl NamedWidget for Tray {
    const NAME: &'static str = "tray";
}
