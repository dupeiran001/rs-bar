// The widgets!() and group!() macros expand to absolute paths
// (`$crate::relm4_bar::widgets::Foo`), so explicit imports aren't required — but Rust's
// macro hygiene still allows them as documentation of which widgets this
// profile uses.
#[allow(unused_imports)]
use crate::relm4_bar::widgets::{
    Battery, BatteryDraw, Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp,
    CpuUsage, Fcitx, Memory, Minimap, Notch, PkgUpdate, Power, PsysDraw, Tray, Volume, Wifi,
    WindowTitle, Wireguard, Workspaces,
};
#[allow(unused_imports)]
use crate::{group, widgets};

use crate::relm4_bar::bar::BarLayout;
use crate::relm4_bar::theme;

use super::Config;

pub(super) fn config() -> Config {
    let t = &theme::NORD;
    Config {
        theme: t,
        font_family: "CaskaydiaCove Nerd Font",
        icon_theme: "breeze-dark",
        icon_size: 16.0,
        power_command: "~/.config/waybar/scripts/logout-menu.sh",
        brightness_get_cmd: "brightnessctl -m | cut -d, -f4 | tr -d '%'",
        brightness_up_cmd: "brightnessctl set +5%",
        brightness_down_cmd: "brightnessctl set 5%-",
        power_icon: concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/power.svg"),
        wireguard_connection: "wg",
        bar_height: 38.0,
        border_top: t.bg,
        border_bottom: t.bg,
    }
}

pub(super) fn bar() -> BarLayout {
    BarLayout {
        left: widgets!(Workspaces, Minimap, WindowTitle),
        center_left: widgets!(
            group!(CpuFreq),
            group!(CpuUsage, |, CpuTemp),
            Memory
        ),
        center: widgets!(Notch),
        center_right: widgets!(
            Clock,
            Wifi,
            Bluetooth,
            PkgUpdate,
            group!(BatteryDraw, |, CpuDraw, |, PsysDraw)
        ),
        right: widgets!(
            Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
    }
}
