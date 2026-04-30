//! WireGuard VPN toggle widget. Subscribes to `hub::wireguard` and renders
//! a VPN icon tinted green when the tunnel is up, dim otherwise.
//!
//! Left-click toggles the configured nmcli WireGuard connection via
//! `nmcli con up id <name>` / `nmcli con down id <name>`. The hub's
//! 1 s poll picks up the state change, so there is no need to push the new
//! value back into the channel from the widget.
//!
//! Mirrors the canonical relm4 widget pattern documented in `cpu_usage.rs`:
//! cached SVG textures via `OnceLock`, model holds the GTK widgets, watch
//! receiver bridged into component messages on the GTK main context, and
//! `update` short-circuits when the displayed value is unchanged.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;

use super::{NamedWidget, WidgetInit, capsule_icon, set_exclusive_class};

const ICON_ON: &str = "vpn-on-symbolic";
const ICON_OFF: &str = "vpn-off-symbolic";

/// CSS classes for on/off color states. `set_exclusive_class` strips the
/// other before adding the chosen one, so stale classes can't accumulate.
const COLOR_CLASSES: &[&str] = &["wireguard-on", "wireguard-off"];

/// Install a process-wide CssProvider once that defines the on/off color
/// classes. Mounted at `STYLE_PROVIDER_PRIORITY_APPLICATION + 1` so it sits
/// just above the global theme provider but below user overrides.
///
/// Done locally (rather than in `assets/default-theme.css`) so the widget is
/// self-contained and does not require the shared theme CSS to know about it.
fn ensure_css() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let css = "\
            .wireguard-on  { color: @rs_green; }\n\
            .wireguard-off { color: @rs_fg_dark; }\n\
        ";
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
        }
    });
}

pub struct Wireguard {
    /// Last-seen tunnel state, kept for the displayed-value coalescing check.
    active: bool,
    /// Whether the first hub Update has been applied. On the very first
    /// message we *must* render even when `active` matches the seed value
    /// (otherwise an actually-up tunnel renders as off because both the
    /// model default and the hub-seeded false-default were `false`).
    initialized: bool,
    grouped: bool,
    /// Held so `update` can swap the paintable + class when the state flips.
    icon: gtk::Image,
}

#[derive(Debug)]
pub enum WireguardMsg {
    Update(bool),
    Click,
}

#[relm4::component(pub)]
impl SimpleComponent for Wireguard {
    type Init = WidgetInit;
    type Input = WireguardMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(ICON_OFF),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        ensure_css();
        let widgets = view_output!();
        let model = Wireguard {
            active: false,
            initialized: false,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
        };

        capsule_icon(&root, model.grouped);

        // Cursor → pointer over the clickable area, mirroring rs-bar's
        // `.cursor_pointer()` GPUI styling.
        root.set_cursor_from_name(Some("pointer"));

        // Subscription: bridge the watch::Receiver<bool> into component messages.
        let mut rx = hub::wireguard::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Apply the current value immediately, then await further changes.
            let initial = *rx.borrow_and_update();
            s.input(WireguardMsg::Update(initial));
            while rx.changed().await.is_ok() {
                let v = *rx.borrow_and_update();
                s.input(WireguardMsg::Update(v));
            }
        });

        // Click → toggle the connection via nmcli on a detached thread so we
        // never block the GTK main loop on the subprocess.
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        let s = sender.clone();
        click.connect_pressed(move |_, _, _, _| s.input(WireguardMsg::Click));
        root.add_controller(click);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WireguardMsg::Update(active) => {
                if self.initialized && active == self.active {
                    return;
                }
                self.initialized = true;
                self.active = active;
                let (name, class) = if active {
                    (ICON_ON, "wireguard-on")
                } else {
                    (ICON_OFF, "wireguard-off")
                };
                self.icon.set_icon_name(Some(name));
                set_exclusive_class(&self.icon, class, COLOR_CLASSES);
            }
            WireguardMsg::Click => {
                let active = self.active;
                let conn = config::WIREGUARD_CONNECTION().to_string();
                std::thread::Builder::new()
                    .name("wireguard-toggle".into())
                    .spawn(move || {
                        let action = if active { "down" } else { "up" };
                        let _ = std::process::Command::new("nmcli")
                            .args(["con", action, "id", &conn])
                            .output();
                    })
                    .ok();
                // The hub's 1 s poll picks up the new state; no need to push
                // back into the channel from here.
            }
        }
    }
}

impl NamedWidget for Wireguard {
    const NAME: &'static str = "wireguard";
}
