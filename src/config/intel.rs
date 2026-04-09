use gpui::App;

use crate::Bar;
use crate::theme;
use crate::widgets::GpuBusy;
use crate::widgets::GpuDraw;
use crate::widgets::{
    Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp, CpuUsage, Fcitx, Memory,
    Minimap, PkgUpdate, Power, PsysDraw, Tray, Volume, Widget, Wifi, WindowTitle, Wireguard,
    Workspaces, group,
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
        bar_height: 30.0,
        border_top: t.bg,
        border_bottom: t.bg,
    }
}

pub(super) fn bar(cx: &mut App) -> Bar {
    Bar {
        left: widgets!(cx, Workspaces, Minimap, WindowTitle),
        center_left: widgets!(cx, CpuFreq, group!(cx, CpuUsage, |, CpuTemp), Memory),
        center: widgets!(cx, Clock),
        center_right: widgets!(
            cx,
            Wifi,
            Bluetooth,
            PkgUpdate,
            group!(cx, GpuDraw, |, CpuDraw, |, GpuBusy)
        ),
        right: widgets!(
            cx, Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
    }
}
