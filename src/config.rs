use gpui::App;

use crate::Bar;
use crate::theme::{self, Theme};
use crate::widgets::{Bluetooth, Brightness, CapsLock, Clock, CpuUsage, Date, Fcitx, Minimap, PkgUpdate, Power, PowerDraw, Tray, Volume, Wifi, Widget, WindowTitle, Workspaces};

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

pub(crate) const BAR_HEIGHT: f32 = 24.0;
pub(crate) const BORDER_TOP: u32 = THEME.bg;
pub(crate) const BORDER_BOTTOM: u32 = THEME.border;

/// Usable content height inside the bar, excluding top/bottom border lines (1px each).
pub(crate) const CONTENT_HEIGHT: f32 = BAR_HEIGHT - 2.0;

// macro rule to simplify creation of bar widgets
macro_rules! widgets {
    ($cx:expr, $($w:ty),* $(,)?) => {
        vec![$(Widget::build::<$w>($cx)),*]
    };
}

pub(crate) fn bar(cx: &mut App) -> Bar {
    Bar {
        left: widgets!(cx, Workspaces, Minimap, WindowTitle),
        center_left: widgets!(cx,),
        center: widgets!(cx, CpuUsage, Clock, Date, Wifi, Bluetooth, PkgUpdate, PowerDraw),
        center_right: widgets!(cx,),
        right: widgets!(cx, Volume, Brightness, Tray, Fcitx, CapsLock, Power),
    }
}
