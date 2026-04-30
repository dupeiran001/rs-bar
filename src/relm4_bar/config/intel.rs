#[allow(unused_imports)]
use crate::relm4_bar::widgets::{
    Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuFreqGraph, CpuTemp, CpuUsage,
    Fcitx, GpuBusy, GpuDraw, Memory, Minimap, PkgUpdate, Power, Tray, Volume, Wifi, WindowTitle,
    Wireguard, Workspaces,
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
        bar_height: 30.0,
        border_top: t.bg,
        border_bottom: t.bg,
    }
}

pub(super) fn bar() -> BarLayout {
    BarLayout {
        left: widgets!(Workspaces, Minimap, WindowTitle),
        center_left: widgets!(group!(CpuFreqGraph, |, CpuFreq), group!(CpuUsage, |, CpuTemp), Memory),
        center: widgets!(Clock),
        center_right: widgets!(
            Wifi,
            Bluetooth,
            PkgUpdate,
            group!(GpuDraw, |, CpuDraw, |, GpuBusy)
        ),
        right: widgets!(
            Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
    }
}
