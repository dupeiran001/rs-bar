//! Design tokens for the bar's CSS.
//!
//! GTK4 CSS doesn't support custom variables outside `@define-color`, so
//! non-color tokens (radii, spacings, durations, easings) live here as Rust
//! constants and are spliced into the CSS string at startup by
//! [`apply_tokens`]. Authoring CSS uses `@RS_*` placeholders verbatim, e.g.
//! `border-radius: @RS_RADIUS_MD;` — they live in GTK's at-rule namespace so
//! they parse cleanly even before substitution. Centralizing the values here
//! means changing the design language is one edit, not a sweep.
//!
//! Color tokens are still owned by [`super::Theme`] and emitted as
//! `@define-color rs_*` blocks (see `style.rs`); tokens defined here are the
//! non-color half of the design system.

// ── Radii ──────────────────────────────────────────────────────────────
// Scale matches Noctalia's 4/8/12/16/20 ladder. RADIUS_XL is the panel
// rounding that gives popovers their soft, "curvy" silhouette.
pub const RADIUS_PILL: &str = "9999px";
pub const RADIUS_XL:   &str = "20px";
pub const RADIUS_LG:   &str = "16px";
pub const RADIUS_MD:   &str = "12px";
pub const RADIUS_SM:   &str = "8px";
pub const RADIUS_XS:   &str = "4px";

// ── Spacings ───────────────────────────────────────────────────────────
pub const SPACING_XL: &str = "16px";
pub const SPACING_LG: &str = "12px";
pub const SPACING_MD: &str = "8px";
pub const SPACING_SM: &str = "4px";
pub const SPACING_XS: &str = "2px";

// ── Typography ─────────────────────────────────────────────────────────
pub const FONT_XL:     &str = "14px";
pub const FONT_LG:     &str = "12px";
pub const FONT_MD:     &str = "11px";
pub const FONT_SM:     &str = "10px";
pub const FONT_HEADER: &str = "9px";

// ── Animation ──────────────────────────────────────────────────────────
pub const ANIM_FAST:     &str = "140ms";
pub const ANIM_MED:      &str = "200ms";
pub const ANIM_SLOW:     &str = "300ms";
pub const EASING_SPRING: &str = "cubic-bezier(0.2, 0.8, 0.2, 1)";
// Material Design "standard" curve — slow start, fast settle. Use for
// transitions where the user benefits from a moment of anticipation
// before the motion takes hold (slider thumb hover, fade-ins).
pub const EASING_SMOOTH: &str = "cubic-bezier(0.4, 0, 0.2, 1)";

// ── Component dimensions ───────────────────────────────────────────────
pub const POPOVER_MIN_W: &str = "320px";
pub const SLIDER_W:      &str = "220px";

/// Token table consumed by [`apply_tokens`]. Order matters only when one
/// token's value contains another's name; we don't have any such overlaps,
/// but keeping `@RS_RADIUS_*` strict prefix-distinct from spacing names
/// preserves that property if tokens ever change.
const TOKENS: &[(&str, &str)] = &[
    ("@RS_RADIUS_PILL",   RADIUS_PILL),
    ("@RS_RADIUS_XL",     RADIUS_XL),
    ("@RS_RADIUS_LG",     RADIUS_LG),
    ("@RS_RADIUS_MD",     RADIUS_MD),
    ("@RS_RADIUS_SM",     RADIUS_SM),
    ("@RS_RADIUS_XS",     RADIUS_XS),
    ("@RS_SPACING_XL",    SPACING_XL),
    ("@RS_SPACING_LG",    SPACING_LG),
    ("@RS_SPACING_MD",    SPACING_MD),
    ("@RS_SPACING_SM",    SPACING_SM),
    ("@RS_SPACING_XS",    SPACING_XS),
    ("@RS_FONT_XL",       FONT_XL),
    ("@RS_FONT_LG",       FONT_LG),
    ("@RS_FONT_MD",       FONT_MD),
    ("@RS_FONT_SM",       FONT_SM),
    ("@RS_FONT_HEADER",   FONT_HEADER),
    ("@RS_ANIM_FAST",     ANIM_FAST),
    ("@RS_ANIM_MED",      ANIM_MED),
    ("@RS_ANIM_SLOW",     ANIM_SLOW),
    ("@RS_EASING_SPRING", EASING_SPRING),
    ("@RS_EASING_SMOOTH", EASING_SMOOTH),
    ("@RS_POPOVER_MIN_W", POPOVER_MIN_W),
    ("@RS_SLIDER_W",      SLIDER_W),
];

/// Replace every `@RS_*` placeholder in `css` with its concrete value.
/// Runs once at CSS load time; cost is one pass per token (~20) over the
/// embedded CSS, which is negligible against GTK's own parse step.
pub fn apply_tokens(css: &mut String) {
    for (token, value) in TOKENS {
        if css.contains(token) {
            *css = css.replace(token, value);
        }
    }
}
