use std::sync::OnceLock;

use crate::relm4_bar::theme::Theme;

mod intel;
mod macbook;

#[allow(dead_code)]
pub struct Config {
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
    #[allow(dead_code)]
    pub fn content_height(&self) -> f32 {
        self.bar_height - 2.0
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static PROFILE: OnceLock<String> = OnceLock::new();

const PROFILES: &[&str] = &["macbook", "intel"];

pub fn init() {
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

pub fn get() -> &'static Config {
    CONFIG
        .get()
        .expect("config::init() must be called before get()")
}

pub fn profile() -> &'static str {
    PROFILE
        .get()
        .expect("config::init() must be called")
        .as_str()
}

#[allow(non_snake_case)]
pub fn THEME() -> &'static Theme {
    get().theme
}
#[allow(non_snake_case, dead_code)]
pub fn FONT_FAMILY() -> &'static str {
    get().font_family
}
#[allow(non_snake_case, dead_code)]
pub fn ICON_THEME() -> &'static str {
    get().icon_theme
}
#[allow(non_snake_case, dead_code)]
pub fn ICON_SIZE() -> f32 {
    get().icon_size
}
#[allow(non_snake_case, dead_code)]
pub fn POWER_COMMAND() -> &'static str {
    get().power_command
}
#[allow(non_snake_case, dead_code)]
pub fn BRIGHTNESS_GET_CMD() -> &'static str {
    get().brightness_get_cmd
}
#[allow(non_snake_case, dead_code)]
pub fn BRIGHTNESS_UP_CMD() -> &'static str {
    get().brightness_up_cmd
}
#[allow(non_snake_case, dead_code)]
pub fn BRIGHTNESS_DOWN_CMD() -> &'static str {
    get().brightness_down_cmd
}
#[allow(non_snake_case, dead_code)]
pub fn POWER_ICON() -> &'static str {
    get().power_icon
}
#[allow(non_snake_case, dead_code)]
pub fn WIREGUARD_CONNECTION() -> &'static str {
    get().wireguard_connection
}
#[allow(non_snake_case)]
pub fn BAR_HEIGHT() -> f32 {
    get().bar_height
}
#[allow(non_snake_case, dead_code)]
pub fn CONTENT_HEIGHT() -> f32 {
    get().content_height()
}
#[allow(non_snake_case, dead_code)]
pub fn BORDER_TOP() -> u32 {
    get().border_top
}
#[allow(non_snake_case, dead_code)]
pub fn BORDER_BOTTOM() -> u32 {
    get().border_bottom
}

// Bar layout — five zones, each a Vec<Widget>. The widgets!() and group!()
// macros (defined in widgets/mod.rs) build these vectors.
pub fn bar() -> crate::relm4_bar::bar::BarLayout {
    match profile() {
        "macbook" => macbook::bar(),
        "intel" => intel::bar(),
        _ => unreachable!(),
    }
}
