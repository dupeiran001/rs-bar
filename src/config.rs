use gpui::App;

use crate::Bar;
use crate::theme::{self, Theme};
use crate::widgets::{
    BatteryDraw, Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp, CpuUsage,
    Date, Fcitx, GpuBusy, GpuDraw, Memory, Minimap, PkgUpdate, Power, PsysDraw, Tray, Volume,
    Widget, Wifi, WindowTitle, Wireguard, Workspaces, group,
};

pub(crate) const THEME: &Theme = &theme::NORD;

pub(crate) const FONT_FAMILY: &str = "CaskaydiaCove Nerd Font";
pub(crate) const ICON_THEME: &str = "breeze-dark";
pub(crate) const ICON_SIZE: f32 = 16.0;

pub(crate) const POWER_COMMAND: &str = "~/.config/waybar/scripts/logout-menu.sh";

// Brightness commands (adjust for your system)
pub(crate) const BRIGHTNESS_GET_CMD: &str = "brightnessctl -m | cut -d, -f4 | tr -d '%'";
pub(crate) const BRIGHTNESS_UP_CMD: &str = "brightnessctl set +5%";
pub(crate) const BRIGHTNESS_DOWN_CMD: &str = "brightnessctl set 5%-";
pub(crate) const POWER_ICON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/power.svg");

// WireGuard connection name (as shown in `nmcli con show`)
pub(crate) const WIREGUARD_CONNECTION: &str = "wg";

pub(crate) const BAR_HEIGHT: f32 = 24.0;
pub(crate) const BORDER_TOP: u32 = THEME.bg;
pub(crate) const BORDER_BOTTOM: u32 = THEME.border;

/// Usable content height inside the bar, excluding top/bottom border lines (1px each).
pub(crate) const CONTENT_HEIGHT: f32 = BAR_HEIGHT - 2.0;

// macro rule to simplify creation of bar widgets
// Accepts plain widget types and group!() calls:
//   widgets!(cx, Clock, group!(cx, A, |, B), Date)
macro_rules! widgets {
    // terminal — no more items
    (@acc $cx:expr, [$($out:expr),*]) => {
        vec![$($out),*]
    };
    // match: group!(...) , rest...
    (@acc $cx:expr, [$($out:expr),*] group!($($g:tt)*) , $($rest:tt)*) => {
        widgets!(@acc $cx, [$($out,)* group!($($g)*)] $($rest)*)
    };
    // match: group!(...) at end
    (@acc $cx:expr, [$($out:expr),*] group!($($g:tt)*)) => {
        widgets!(@acc $cx, [$($out,)* group!($($g)*)])
    };
    // match: ident , rest...
    (@acc $cx:expr, [$($out:expr),*] $w:ident , $($rest:tt)*) => {
        widgets!(@acc $cx, [$($out,)* Widget::build::<$w>($cx)] $($rest)*)
    };
    // match: ident at end
    (@acc $cx:expr, [$($out:expr),*] $w:ident) => {
        widgets!(@acc $cx, [$($out,)* Widget::build::<$w>($cx)])
    };
    // entry point
    ($cx:expr, $($items:tt)*) => {
        widgets!(@acc $cx, [] $($items)*)
    };
}

pub(crate) fn bar(cx: &mut App) -> Bar {
    Bar {
        left: widgets!(cx, Workspaces, Minimap, WindowTitle),
        center_left: widgets!(
            cx,
            group!(cx, CpuFreq, |, GpuBusy),
            group!(cx, CpuUsage, |, CpuTemp),
            Memory
        ),
        center: widgets!(cx, Clock),
        center_right: widgets!(
            cx,
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
