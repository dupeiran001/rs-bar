use gpui::App;

use crate::Bar;
use crate::theme;
use crate::widgets::{
    BatteryDraw, Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp, CpuUsage, Date,
    Fcitx, GpuBusy, GpuDraw, Memory, Minimap, Notch, PkgUpdate, Power, PsysDraw, Tray, Volume,
    Widget, Wifi, WindowTitle, Wireguard, Workspaces, group,
};

use super::{Config, widgets};

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
        border_bottom: t.border,
    }
}

pub(super) fn bar(cx: &mut App) -> Bar {
    Bar {
        left: widgets!(cx, Workspaces, Minimap, WindowTitle),
        center_left: widgets!(
            cx,
            group!(cx, CpuFreq, |, GpuBusy),
            group!(cx, CpuUsage, |, CpuTemp),
            Memory
        ),
        center: widgets!(cx, Notch),
        center_right: widgets!(
            cx,
            Clock,
            Date,
            Wifi,
            Bluetooth,
            PkgUpdate,
            group!(cx, BatteryDraw, |, GpuDraw, |, CpuDraw, |, PsysDraw)
        ),
        right: widgets!(
            cx, Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
    }
}
