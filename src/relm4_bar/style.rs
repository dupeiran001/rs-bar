//! CSS pipeline: generate `@define-color` from Theme, append embedded default,
//! then user override. Loaded into a single CssProvider on the default Display.

use std::path::PathBuf;

use gdk::Display;

use crate::relm4_bar::config;

const DEFAULT_CSS: &str = include_str!("../../assets/default-theme.css");

/// Stub written to ~/.config/rs-bar/gtk-theme.css on first run. Kept short and
/// purely additive — full default rules ship embedded in the binary, and we
/// load the embedded copy unconditionally so theme-CSS updates land cleanly
/// across rs-bar versions. Users put their personal overrides here.
///
/// CSS does not support nested comments, so each example lives in its own
/// `/* … */` block — uncomment a whole block to activate it.
const USER_CSS_STUB: &str = "/* rs-bar user theme overrides.
   The full default theme is bundled in the binary and always loads first;
   anything you put here is appended on top, so add only the rules you want
   to override or extend. Theme color variables (@rs_bg, @rs_fg, @rs_accent,
   …) are pre-defined from the active profile's Theme struct.
   Examples below — remove the surrounding comment to enable. */

/* window.rs-bar { background-color: alpha(@rs_bg, 0.85); } */

/* @define-color rs_accent #f6c177; */
";

fn theme_color_block() -> String {
    let t = config::THEME();
    let c = |name: &str, v: u32| format!("@define-color {name} #{v:06X};\n");

    let mut out = String::new();
    out.push_str(&c("rs_bg", t.bg));
    out.push_str(&c("rs_bg_dark", t.bg_dark));
    out.push_str(&c("rs_bg_dark1", t.bg_dark1));
    out.push_str(&c("rs_fg", t.fg));
    out.push_str(&c("rs_fg_dark", t.fg_dark));
    out.push_str(&c("rs_fg_gutter", t.fg_gutter));
    out.push_str(&c("rs_surface", t.surface));
    out.push_str(&c("rs_text_dim", t.text_dim));
    out.push_str(&c("rs_accent", t.accent));
    out.push_str(&c("rs_accent_dim", t.accent_dim));
    out.push_str(&c("rs_border", t.border));
    out.push_str(&c("rs_border_highlight", t.border_highlight));
    out.push_str(&c("rs_green", t.green));
    out.push_str(&c("rs_yellow", t.yellow));
    out.push_str(&c("rs_orange", t.orange));
    out.push_str(&c("rs_red", t.red));
    out.push_str(&c("rs_blue", t.blue));
    out.push_str(&c("rs_teal", t.teal));
    out.push_str(&c("rs_purple", t.purple));
    out.push_str(&c("rs_error", t.error));
    out.push_str(&c("rs_warn", t.warn));
    out.push_str(&c("rs_info", t.info));
    out
}

fn user_css_path() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".config").join("rs-bar").join("gtk-theme.css")
}

fn maybe_bootstrap_user_css() {
    let path = user_css_path();
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, USER_CSS_STUB).is_ok() {
        log::info!("wrote user-css stub to {}", path.display());
    }
}

pub fn load() {
    maybe_bootstrap_user_css();

    let mut css = String::new();
    css.push_str(&theme_color_block());
    css.push('\n');
    css.push_str(DEFAULT_CSS);

    if let Ok(user) = std::fs::read_to_string(user_css_path()) {
        css.push('\n');
        css.push_str(&user);
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);

    let display = Display::default().expect("no default GdkDisplay");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Register our SVG icon directory so widgets can use Image::from_icon_name.
    // Symbolic-icon recoloring works with these because the *-symbolic.svg
    // copies use fill="currentColor".
    let icon_theme = gtk::IconTheme::for_display(&display);
    icon_theme.add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons"));
}
