use std::sync::OnceLock;

use gpui::App;

use crate::gpui_bar::Bar;
use crate::gpui_bar::theme::Theme;
mod intel;
mod macbook;

pub(crate) struct Config {
    pub theme: &'static Theme,
    pub font_family: &'static str,
    pub icon_theme: &'static str,
    pub icon_size: f32,
    pub power_command: &'static str,
    pub brightness_get_cmd: &'static str,
    pub brightness_up_cmd: &'static str,
    pub brightness_down_cmd: &'static str,
    pub power_icon: &'static str,
    pub wireguard_connection: &'static str,
    pub bar_height: f32,
    pub border_top: u32,
    pub border_bottom: u32,
}

impl Config {
    /// Usable content height inside the bar, excluding top/bottom border lines (1px each).
    pub fn content_height(&self) -> f32 {
        self.bar_height - 2.0
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static PROFILE: OnceLock<String> = OnceLock::new();

const PROFILES: &[&str] = &["macbook", "intel"];

/// Parse `--config <profile>` from CLI args and initialise the global config.
pub(crate) fn init() {
    let args: Vec<String> = std::env::args().collect();
    let profile = args
        .windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "macbook".into());

    let config = match profile.as_str() {
        "macbook" => macbook::config(),
        "intel" => intel::config(),
        other => {
            eprintln!(
                "Unknown config profile '{}'. Available: {}",
                other,
                PROFILES.join(", ")
            );
            std::process::exit(1);
        }
    };

    if CONFIG.set(config).is_err() {
        panic!("config::init() called twice");
    }
    let _ = PROFILE.set(profile.clone());
    log::info!("Using config profile: {profile}");
}

/// Access the active configuration. Panics if `init()` has not been called.
pub(crate) fn get() -> &'static Config {
    CONFIG
        .get()
        .expect("config::init() must be called before get()")
}

/// Build the bar widget layout for the active profile.
pub(crate) fn bar(cx: &mut App) -> Bar {
    match PROFILE
        .get()
        .expect("config::init() must be called before bar()")
        .as_str()
    {
        "macbook" => macbook::bar(cx),
        "intel" => intel::bar(cx),
        _ => unreachable!(), // init() already validated
    }
}

// Convenience re-exports so call sites can use short paths like `config::THEME`.
// These read from the global config at runtime.
#[allow(non_snake_case)]
pub(crate) fn THEME() -> &'static Theme {
    get().theme
}
#[allow(non_snake_case)]
pub(crate) fn FONT_FAMILY() -> &'static str {
    get().font_family
}
#[allow(non_snake_case)]
pub(crate) fn ICON_THEME() -> &'static str {
    get().icon_theme
}
#[allow(non_snake_case)]
pub(crate) fn ICON_SIZE() -> f32 {
    get().icon_size
}
#[allow(non_snake_case)]
pub(crate) fn POWER_COMMAND() -> &'static str {
    get().power_command
}
#[allow(non_snake_case)]
pub(crate) fn BRIGHTNESS_GET_CMD() -> &'static str {
    get().brightness_get_cmd
}
#[allow(non_snake_case)]
pub(crate) fn BRIGHTNESS_UP_CMD() -> &'static str {
    get().brightness_up_cmd
}
#[allow(non_snake_case)]
pub(crate) fn BRIGHTNESS_DOWN_CMD() -> &'static str {
    get().brightness_down_cmd
}
#[allow(non_snake_case)]
pub(crate) fn POWER_ICON() -> &'static str {
    get().power_icon
}
#[allow(non_snake_case)]
pub(crate) fn WIREGUARD_CONNECTION() -> &'static str {
    get().wireguard_connection
}
#[allow(non_snake_case)]
pub(crate) fn BAR_HEIGHT() -> f32 {
    get().bar_height
}
#[allow(non_snake_case)]
pub(crate) fn CONTENT_HEIGHT() -> f32 {
    get().content_height()
}
#[allow(non_snake_case)]
pub(crate) fn BORDER_TOP() -> u32 {
    get().border_top
}
#[allow(non_snake_case)]
pub(crate) fn BORDER_BOTTOM() -> u32 {
    get().border_bottom
}

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
pub(crate) use widgets;
