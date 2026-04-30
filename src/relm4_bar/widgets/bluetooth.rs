//! Bluetooth widget. Subscribes to `hub::bluetooth` and renders one of three
//! icons (off / on-no-device / connected). Click → popover with the paired
//! device list and connect/disconnect buttons + a power toggle.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::relm4_bar::config;
use crate::relm4_bar::hub;
use crate::relm4_bar::hub::bluetooth::{BluetoothState, DeviceInfo};

use super::{NamedWidget, WidgetInit, capsule, set_exclusive_class};

const ICON_OFF: &str = "bluetooth-off-symbolic";
const ICON_ON: &str = "bluetooth-on-symbolic";
const ICON_CONNECTED: &str = "bluetooth-connected-symbolic";

/// CSS classes mirroring the three states; `set_exclusive_class` swaps between
/// them so a stale class can't accumulate.
const STATE_CLASSES: &[&str] = &["bluetooth-off", "bluetooth-on", "bluetooth-connected"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BtIconState {
    Off,
    On,
    Connected,
}

impl BtIconState {
    fn from_state(s: &BluetoothState) -> Self {
        if !s.powered {
            BtIconState::Off
        } else if !s.connected_devices.is_empty() {
            BtIconState::Connected
        } else {
            BtIconState::On
        }
    }

    fn class(self) -> &'static str {
        match self {
            BtIconState::Off => "bluetooth-off",
            BtIconState::On => "bluetooth-on",
            BtIconState::Connected => "bluetooth-connected",
        }
    }

    fn icon_name(self) -> &'static str {
        match self {
            BtIconState::Off => ICON_OFF,
            BtIconState::On => ICON_ON,
            BtIconState::Connected => ICON_CONNECTED,
        }
    }
}

pub struct Bluetooth {
    grouped: bool,
    /// Last-seen icon state; the displayed-value coalescing check skips GTK
    /// writes when the icon would be unchanged.
    icon_state: BtIconState,
    /// Held so `update` can swap textures and re-style on state changes.
    icon: gtk::Image,
    /// Held so `update` can rebuild the popover device list and toggle the
    /// power switch.
    list_box: gtk::Box,
    power_switch: gtk::Switch,
    /// Set during `init`; the power-switch handler reads it to suppress the
    /// `notify::active` action while we are programmatically syncing the
    /// switch position to a new BluetoothState.
    suppress_switch: Rc<RefCell<bool>>,
}

#[derive(Debug)]
pub enum BluetoothMsg {
    Update(BluetoothState),
}

#[relm4::component(pub)]
impl SimpleComponent for Bluetooth {
    type Init = WidgetInit;
    type Input = BluetoothMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_icon_name: Some(BtIconState::Off.icon_name()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let widgets = view_output!();

        // ── Popover scaffolding ────────────────────────────────────────
        let popover = gtk::Popover::builder().autohide(true).build();
        popover.add_css_class("bluetooth-popover");
        popover.set_parent(&root);

        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 6);

        // Header row: title + power switch.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Bluetooth"));
        title.add_css_class("bluetooth-popover-title");
        title.set_hexpand(true);
        title.set_halign(gtk::Align::Start);
        header.append(&title);

        let power_switch = gtk::Switch::new();
        power_switch.set_valign(gtk::Align::Center);
        header.append(&power_switch);
        popover_box.append(&header);

        // Vertical list of devices, rebuilt on every BluetoothState change.
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list_box.add_css_class("bluetooth-device-list");
        popover_box.append(&list_box);

        popover.set_child(Some(&popover_box));

        // Click → open popover.
        let click = gtk::GestureClick::new();
        let popover_for_click = popover.clone();
        click.connect_pressed(move |_, _, _, _| popover_for_click.popup());
        root.add_controller(click);

        // Power switch handler. We have to both react to user toggles and
        // sync the switch when external state changes (e.g. someone runs
        // `bluetoothctl power off`). The `suppress_switch` flag lets the
        // `update` handler set the position without triggering `power_on/off`.
        let suppress_switch = Rc::new(RefCell::new(false));
        {
            let suppress = suppress_switch.clone();
            power_switch.connect_active_notify(move |sw| {
                if *suppress.borrow() {
                    return;
                }
                if sw.is_active() {
                    hub::bluetooth::power_on();
                } else {
                    hub::bluetooth::power_off();
                }
            });
        }

        let model = Bluetooth {
            grouped: init.grouped,
            icon_state: BtIconState::Off,
            icon: widgets.icon.clone(),
            list_box: list_box.clone(),
            power_switch: power_switch.clone(),
            suppress_switch,
        };

        capsule(&root, model.grouped);
        set_exclusive_class(&model.icon, model.icon_state.class(), STATE_CLASSES);

        // Subscription: bridge the watch::Receiver<BluetoothState> into messages.
        let mut rx = hub::bluetooth::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            // Push the initial value, then react to every change. The
            // explicit `(*…).clone()` clones the inner value rather than the
            // `Ref`-style guard.
            let initial = (*rx.borrow_and_update()).clone();
            s.input(BluetoothMsg::Update(initial));
            while rx.changed().await.is_ok() {
                let v = (*rx.borrow_and_update()).clone();
                s.input(BluetoothMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BluetoothMsg::Update(state) => {
                // Icon coalescing — only touch GTK when the visible icon would
                // change. Device-list rebuild always runs because the popover
                // contents may have changed even when the icon hasn't.
                let new_icon_state = BtIconState::from_state(&state);
                if new_icon_state != self.icon_state {
                    self.icon_state = new_icon_state;
                    self.icon.set_icon_name(Some(new_icon_state.icon_name()));
                    set_exclusive_class(&self.icon, new_icon_state.class(), STATE_CLASSES);
                }

                // Sync the power switch without triggering its handler.
                {
                    *self.suppress_switch.borrow_mut() = true;
                    self.power_switch.set_active(state.powered);
                    *self.suppress_switch.borrow_mut() = false;
                }

                rebuild_device_list(&self.list_box, &state);
            }
        }
    }
}

impl NamedWidget for Bluetooth {
    const NAME: &'static str = "bluetooth";
}

/// Tear down the previous device rows and rebuild them from the new state.
/// Cheap — paired-device counts are tiny (single digits).
fn rebuild_device_list(list_box: &gtk::Box, state: &BluetoothState) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    if !state.powered {
        let placeholder = gtk::Label::new(Some("Bluetooth is off"));
        placeholder.add_css_class("bluetooth-placeholder");
        placeholder.set_halign(gtk::Align::Start);
        list_box.append(&placeholder);
        return;
    }

    if state.paired_devices.is_empty() {
        let placeholder = gtk::Label::new(Some("No paired devices"));
        placeholder.add_css_class("bluetooth-placeholder");
        placeholder.set_halign(gtk::Align::Start);
        list_box.append(&placeholder);
        return;
    }

    // Connected devices first, then the rest, alphabetically by display name.
    let mut sorted = state.paired_devices.clone();
    sorted.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    for dev in sorted {
        list_box.append(&device_row(&dev));
    }
}

/// One row: device name (+ MAC tooltip) + connect/disconnect button.
fn device_row(dev: &DeviceInfo) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("bluetooth-device-row");

    let name = if dev.name.is_empty() { dev.mac.clone() } else { dev.name.clone() };
    let label = gtk::Label::new(Some(&name));
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Start);
    label.set_tooltip_text(Some(&dev.mac));
    if dev.connected {
        label.add_css_class("bluetooth-device-connected");
    }
    row.append(&label);

    let button = gtk::Button::new();
    button.set_label(if dev.connected { "Disconnect" } else { "Connect" });
    button.add_css_class("bluetooth-device-button");

    let mac = dev.mac.clone();
    let connected = dev.connected;
    button.connect_clicked(move |_| {
        if connected {
            hub::bluetooth::disconnect(&mac);
        } else {
            hub::bluetooth::connect(&mac);
        }
    });
    row.append(&button);

    row
}
