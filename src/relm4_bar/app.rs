//! Root App component. On init, opens one Bar per GdkMonitor and subscribes
//! to monitors `items-changed` for hot-plug. The App owns Controller<Bar>
//! handles in a HashMap keyed by monitor.

use std::collections::HashMap;
use std::time::Duration;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::bar::{Bar, BarInit};
use crate::relm4_bar::config;
use crate::relm4_bar::style;

pub struct App {
    bars: HashMap<gdk::Monitor, Controller<Bar>>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum AppMsg {
    EnumerateMonitors,
    MonitorAdded(gdk::Monitor),
    MonitorRemoved(gdk::Monitor),
}

impl Component for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = ();
    type Root = gtk::Window;
    type Widgets = ();

    fn init_root() -> Self::Root {
        // Management window — relm4 RelmApp will call `present()` on this
        // after init returns. We hide it immediately on realize so it never
        // appears on screen; the actual UI lives in per-monitor Bar windows
        // launched below.
        gtk::Window::builder()
            .default_width(1)
            .default_height(1)
            .decorated(false)
            .resizable(false)
            .build()
    }

    fn init(_: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        style::load();

        // Suppress the management window. `connect_realize` fires before the
        // window's surface is mapped, so calling `set_visible(false)` here
        // prevents it from flashing on screen. We also hide on `connect_show`
        // as a belt-and-braces guard against compositors that map regardless.
        root.connect_realize(|w| {
            w.set_visible(false);
        });
        root.connect_show(|w| {
            w.set_visible(false);
        });

        let display = gdk::Display::default().expect("no default GdkDisplay");
        let monitors = display.monitors();

        // Enumerate after a short delay (matches rs-bar's GPUI workaround for
        // late display enumeration; harmless if monitors are already known).
        let s = sender.clone();
        glib::timeout_add_local_once(Duration::from_millis(100), move || {
            s.input(AppMsg::EnumerateMonitors);
        });

        // Hot-plug: subscribe to items-changed on the monitors list.
        let s = sender.clone();
        let monitors_clone = monitors.clone();
        monitors.connect_items_changed(move |list, position, removed, added| {
            // Removed: indices [position, position+removed) — but the items
            // are already gone from the list. We track existing bars in the
            // App map and reconcile on each event by re-enumerating.
            let _ = (list, position, removed, added);
            s.input(AppMsg::EnumerateMonitors);
            let _ = &monitors_clone; // keep alive
        });

        ComponentParts {
            model: App { bars: HashMap::new() },
            widgets: (),
        }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            AppMsg::EnumerateMonitors => {
                let display = gdk::Display::default().expect("no default GdkDisplay");
                let list = display.monitors();
                let n = list.n_items();
                let mut current: Vec<gdk::Monitor> = (0..n)
                    .filter_map(|i| list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
                    .collect();

                if current.is_empty() {
                    log::warn!("No monitors detected; nothing to do.");
                    return;
                }

                // Add bars for new monitors
                for m in &current {
                    if !self.bars.contains_key(m) {
                        log::info!("opening bar on {}", m.connector().unwrap_or_default());
                        let connector = m.connector().map(|s| s.to_string()).unwrap_or_default();
                        crate::relm4_bar::widgets::BAR_CTX.with(|c| {
                            *c.borrow_mut() = Some(crate::relm4_bar::widgets::BarContext { connector });
                        });
                        let layout = config::bar();
                        crate::relm4_bar::widgets::BAR_CTX.with(|c| *c.borrow_mut() = None);
                        let controller = Bar::builder()
                            .launch(BarInit {
                                monitor: m.clone(),
                                layout,
                            })
                            .detach();
                        self.bars.insert(m.clone(), controller);
                    }
                }

                // Remove bars for vanished monitors
                let still_present: std::collections::HashSet<_> = current.drain(..).collect();
                self.bars.retain(|m, _| still_present.contains(m));
            }
            AppMsg::MonitorAdded(_) | AppMsg::MonitorRemoved(_) => {
                // Reserved — currently we always re-enumerate.
            }
        }
    }
}
