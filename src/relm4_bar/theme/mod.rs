mod nord;
pub mod tokens;

pub use nord::NORD;

#[allow(dead_code)]
pub struct Theme {
    pub bg: u32,
    pub bg_dark: u32,
    pub bg_dark1: u32,
    pub fg: u32,
    pub fg_dark: u32,
    pub fg_gutter: u32,
    pub surface: u32,
    pub text_dim: u32,
    pub accent: u32,
    pub accent_dim: u32,
    pub border: u32,
    pub border_highlight: u32,

    pub green: u32,
    pub yellow: u32,
    pub orange: u32,
    pub red: u32,
    pub blue: u32,
    pub teal: u32,
    pub purple: u32,

    pub error: u32,
    pub warn: u32,
    pub info: u32,
}
