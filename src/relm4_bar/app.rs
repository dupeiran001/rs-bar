//! Root App component. On init, opens one Bar per GdkMonitor and subscribes
//! to monitors `items-changed` for hot-plug. The App owns Controller<Bar>
//! handles in a HashMap keyed by monitor.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::bar::{Bar, BarInit};
use crate::relm4_bar::config;
use crate::relm4_bar::style;

/// One open bar plus the connector string we used to filter its niri data.
/// We track `used_connector` so we can detect a stale-connector case (bar
/// opened before wl_output's `name` event arrived) and rebuild the bar with
/// the correct connector once it's available.
struct BarSlot {
    controller: Controller<Bar>,
    used_connector: String,
}

pub struct App {
    bars: HashMap<gdk::Monitor, BarSlot>,
    /// Monitors with a `notify::connector` listener attached. Tracked so we
    /// only attach once per monitor (the listener stays alive for the
    /// monitor's lifetime; GDK drops it when the monitor is destroyed).
    notify_attached: HashSet<gdk::Monitor>,
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

    fn init(
        _: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        style::load();

        // Suppress the management window. relm4's RelmApp::run calls present()
        // on this root after init returns, which goes through the realize →
        // map → show sequence. Hiding on `connect_show` (which fires *after*
        // map completes) is clean — hiding earlier in `connect_realize` trips
        // GTK's `gtk_widget_map: assertion '_gtk_widget_get_visible (widget)'`
        // because GTK's map step expects the widget to still be visible.
        root.connect_show(|w| {
            w.set_visible(false);
        });

        let display = gdk::Display::default().expect("no default GdkDisplay");
        let monitors = display.monitors();

        // Enumerate after a short delay (matches rs-bar's GPUI workaround for
        // late display enumeration; harmless if monitors are already known).
        let s = sender.input_sender().clone();
        glib::timeout_add_local_once(Duration::from_millis(100), move || {
            let _ = s.send(AppMsg::EnumerateMonitors);
        });

        // Hot-plug: subscribe to items-changed on the monitors list. Re-enumerate
        // immediately and again after a short delay — `items-changed` can fire
        // before wl_output's `name` event arrives, leaving the connector empty
        // on the first pass; the delayed pass picks up the late connector and
        // rebuilds the bar via EnumerateMonitors' connector-mismatch path.
        let s = sender.input_sender().clone();
        let monitors_clone = monitors.clone();
        monitors.connect_items_changed(move |list, position, removed, added| {
            let _ = (list, position, removed, added);
            let _ = s.send(AppMsg::EnumerateMonitors);
            let s2 = s.clone();
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                let _ = s2.send(AppMsg::EnumerateMonitors);
            });
            let _ = &monitors_clone; // keep alive
        });

        ComponentParts {
            model: App {
                bars: HashMap::new(),
                notify_attached: HashSet::new(),
            },
            widgets: (),
        }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            AppMsg::EnumerateMonitors => {
                let display = gdk::Display::default().expect("no default GdkDisplay");
                let list = display.monitors();
                let n = list.n_items();
                let current: Vec<gdk::Monitor> = (0..n)
                    .filter_map(|i| list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
                    .collect();

                if current.is_empty() {
                    log::warn!("No monitors detected; nothing to do.");
                    return;
                }

                for m in &current {
                    let new_connector = m.connector().map(|s| s.to_string()).unwrap_or_default();

                    // If the bar already exists with the same connector, leave
                    // it alone. If the connector has changed (typically: was
                    // "" at hot-plug time and is now the real name), close the
                    // existing bar so we rebuild it below with the correct
                    // connector — niri-aware widgets capture connector at
                    // their own init() time, so changing it after the fact
                    // requires re-launching the widgets.
                    if let Some(existing) = self.bars.get(m) {
                        if existing.used_connector == new_connector {
                            continue;
                        }
                        log::info!(
                            "connector for monitor changed: {:?} → {:?}; rebuilding bar",
                            existing.used_connector,
                            new_connector,
                        );
                        if let Some(slot) = self.bars.remove(m) {
                            slot.controller.widget().close();
                            drop(slot.controller);
                        }
                    }

                    // Open the bar regardless of whether the connector is set
                    // yet — the user wants to see the bar on every monitor as
                    // soon as it's plugged in. If the connector is still empty
                    // here, niri filters will return empty until the connector
                    // arrives and we rebuild via the path above.
                    log::info!("opening bar on {:?}", new_connector);
                    crate::relm4_bar::widgets::BAR_CTX.with(|c| {
                        *c.borrow_mut() = Some(crate::relm4_bar::widgets::BarContext {
                            connector: new_connector.clone(),
                        });
                    });
                    let layout = config::bar();
                    crate::relm4_bar::widgets::BAR_CTX.with(|c| *c.borrow_mut() = None);
                    let controller = Bar::builder()
                        .launch(BarInit {
                            monitor: m.clone(),
                            layout,
                        })
                        .detach();
                    self.bars.insert(
                        m.clone(),
                        BarSlot {
                            controller,
                            used_connector: new_connector,
                        },
                    );

                    // Attach a `notify::connector` listener once per monitor
                    // so the rebuild path triggers as soon as the late
                    // wl_output `name` event lands. Re-attaching on the
                    // same monitor is harmless but wasteful, so guard with
                    // a HashSet.
                    if self.notify_attached.insert(m.clone()) {
                        let s = sender.input_sender().clone();
                        m.connect_connector_notify(move |_mon| {
                            let _ = s.send(AppMsg::EnumerateMonitors);
                        });
                    }
                }

                // Close and forget bars whose monitors have vanished. Closing
                // the window explicitly (not just dropping the controller)
                // ensures GTK tears down the layer-shell surface; the
                // controller drop then frees the rest of the widget tree.
                let still_present: HashSet<_> = current.into_iter().collect();
                self.bars.retain(|m, slot| {
                    if still_present.contains(m) {
                        true
                    } else {
                        log::info!("closing bar for vanished monitor");
                        slot.controller.widget().close();
                        false
                    }
                });
                self.notify_attached.retain(|m| still_present.contains(m));
            }
            AppMsg::MonitorAdded(_) | AppMsg::MonitorRemoved(_) => {
                // Reserved — currently we always re-enumerate.
            }
        }
    }
}
