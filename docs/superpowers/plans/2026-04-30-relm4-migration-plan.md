# rs-bar-relm4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port rs-bar (GPUI/Wayland status bar for niri) from GPUI to relm4/GTK4 in a fully self-contained sibling project at `~/Develop/rs-bar-relm4`, preserving all 23 widgets, both profiles (`--config macbook`, `--config intel`), all popups, all fallbacks, and visual fidelity.

**Architecture:** GTK4 + relm4 components on `gtk4-layer-shell` for Wayland layer-shell surfaces. Shared system-reading code in `src/hub/` modules, each owning a `tokio::sync::watch::Sender<T>` fed by a singleton OS thread. Widgets in `src/widgets/<name>.rs` (one widget per file) subscribe via `watch::Receiver` and translate updates to relm4 `Component` messages on the GTK main loop via `relm4::spawn_local`. CSS theming via embedded default + optional `~/.config/rs-bar/gtk-theme.css` overlay.

**Tech Stack:** Rust 2024, relm4 0.10, gtk4 0.10, gtk4-layer-shell 0.6, tokio (runtime + sync), niri-ipc, system-tray, libc, chrono.

**Source of truth for behavior:** `~/Develop/rs-bar/src/gpui_bar/` — every widget and hub module in rs-bar-relm4 must reproduce the behavior of the corresponding file in rs-bar. Where a relm4 widget task says "port from rs-bar", read that file first.

**Spec:** `docs/superpowers/specs/2026-04-30-relm4-migration-design.md`

**No tests.** rs-bar has no test suite; widgets exercise live system state and GTK rendering. Verification is `cargo build` + `cargo run` + visual check, per-widget. This is consistent with the spec's non-goal of introducing tests.

---

## Parallelization Map

```
Phase 0 (sequential)         — bootstrap repo, all 23 widget stubs registered, bar opens empty
   │
Phase 1 (sequential)         — reference pattern: hub/cpu_usage.rs + widgets/cpu_usage.rs end-to-end
   │
Phase 2 (PARALLEL, 7 tasks)  — port hub/* modules: cpu_temp, cpu_freq, memory, gpu_busy,
   │                            power_draw, niri, tray. Independent files.
   │
Phase 3 (PARALLEL, 22 tasks) — port widgets/* (excluding cpu_usage from P1 and tray which
   │                            depends on a special hub from P2). Each task touches exactly
   │                            one widgets/<name>.rs file. Independent.
   │
Phase 4 (sequential)         — final CSS, top/bottom borders, ~/.config bootstrap, README
   │
Phase 5 (sequential)         — visual acceptance verification on both profiles
```

Phases 2 and 3 are the parallel-friendly batches. Phase 0 makes this possible by stubbing every widget and hub module so that `widgets/mod.rs`, `hub/mod.rs`, and both config profile files are *not touched* in subsequent phases.

---

## Reference paths

- New project root: `~/Develop/rs-bar-relm4/`
- Source-of-truth project: `~/Develop/rs-bar/`
- Spec: `~/Develop/rs-bar-relm4/docs/superpowers/specs/2026-04-30-relm4-migration-design.md`
- This plan: `~/Develop/rs-bar-relm4/docs/superpowers/plans/2026-04-30-relm4-migration-plan.md`

All commits land in `~/Develop/rs-bar-relm4/`. The `~/Develop/rs-bar/` repo is read-only reference and must not be modified.

---

# Phase 0 — Bootstrap (sequential)

Phase 0 produces a buildable, runnable bar with all 23 widget stubs and all hub stubs registered. The bar opens (empty zones rendering nothing visible) on every monitor on both profiles. Subsequent phases fill in the stubs without touching shared registration files.

## Task 0.1: Cargo.toml + .gitignore + README skeleton

**Files:**
- Create: `~/Develop/rs-bar-relm4/Cargo.toml`
- Create: `~/Develop/rs-bar-relm4/.gitignore`
- Create: `~/Develop/rs-bar-relm4/README.md`

- [ ] **Step 1: Write Cargo.toml**

```toml
[package]
name = "rs-bar-relm4"
version = "0.1.0"
edition = "2024"

[dependencies]
relm4 = "0.10"
relm4-components = "0.10"
gtk = { package = "gtk4", version = "0.10", features = ["v4_18"] }
gtk4-layer-shell = "0.6"
glib = "0.22"
gio = "0.22"
gdk = { package = "gdk4", version = "0.10" }

tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros", "process"] }

chrono = "0.4"
niri-ipc = "25.11"
system-tray = "0.8"
async-channel = "2"
libc = "0.2"
serde_json = "1"
anyhow = "1"
log = "0.4"
env_logger = "0.11"
linicon = "2.3"
png = "0.17"
uuid = "1"
```

- [ ] **Step 2: Write .gitignore**

```
/target
**/*.rs.bk
Cargo.lock.bak
```

- [ ] **Step 3: Write README.md (skeleton)**

```markdown
# rs-bar-relm4

A Wayland status bar for the [niri](https://github.com/YaLTeR/niri) compositor. relm4/GTK4 port of [rs-bar](../rs-bar) (originally GPUI).

## Build & Run

    cargo build --release
    rs-bar-relm4 --config macbook    # default profile
    rs-bar-relm4 --config intel

## Theming

A default theme is bundled. On first run a copy is written to `~/.config/rs-bar/gtk-theme.css` — edit that file to customize colors, fonts, spacing.

## Status

Migration in progress. Tracking spec: `docs/superpowers/specs/2026-04-30-relm4-migration-design.md`.
```

- [ ] **Step 4: Commit**

```bash
cd ~/Develop/rs-bar-relm4
git add Cargo.toml .gitignore README.md
git commit -m "build: initial Cargo manifest and gitignore"
```

## Task 0.2: Theme module

**Files:**
- Create: `src/theme/mod.rs`
- Create: `src/theme/nord.rs`

- [ ] **Step 1: Write src/theme/mod.rs**

```rust
mod nord;

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
```

- [ ] **Step 2: Write src/theme/nord.rs**

Copy verbatim from `~/Develop/rs-bar/src/gpui_bar/theme/nord.rs` (only the `use super::Theme; pub const NORD: Theme = Theme { ... };` block — no GPUI imports needed).

- [ ] **Step 3: Commit**

```bash
git add src/theme/
git commit -m "theme: add Theme struct and Nord palette"
```

## Task 0.3: Config module + profile stubs

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/macbook.rs`
- Create: `src/config/intel.rs`

- [ ] **Step 1: Write src/config/mod.rs**

```rust
use std::sync::OnceLock;

use crate::theme::Theme;

mod macbook;
mod intel;

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
    CONFIG.get().expect("config::init() must be called before get()")
}

pub fn profile() -> &'static str {
    PROFILE.get().expect("config::init() must be called").as_str()
}

#[allow(non_snake_case)]
pub fn THEME() -> &'static Theme { get().theme }
#[allow(non_snake_case)]
pub fn FONT_FAMILY() -> &'static str { get().font_family }
#[allow(non_snake_case)]
pub fn ICON_THEME() -> &'static str { get().icon_theme }
#[allow(non_snake_case)]
pub fn ICON_SIZE() -> f32 { get().icon_size }
#[allow(non_snake_case)]
pub fn POWER_COMMAND() -> &'static str { get().power_command }
#[allow(non_snake_case)]
pub fn BRIGHTNESS_GET_CMD() -> &'static str { get().brightness_get_cmd }
#[allow(non_snake_case)]
pub fn BRIGHTNESS_UP_CMD() -> &'static str { get().brightness_up_cmd }
#[allow(non_snake_case)]
pub fn BRIGHTNESS_DOWN_CMD() -> &'static str { get().brightness_down_cmd }
#[allow(non_snake_case)]
pub fn POWER_ICON() -> &'static str { get().power_icon }
#[allow(non_snake_case)]
pub fn WIREGUARD_CONNECTION() -> &'static str { get().wireguard_connection }
#[allow(non_snake_case)]
pub fn BAR_HEIGHT() -> f32 { get().bar_height }
#[allow(non_snake_case)]
pub fn CONTENT_HEIGHT() -> f32 { get().content_height() }
#[allow(non_snake_case)]
pub fn BORDER_TOP() -> u32 { get().border_top }
#[allow(non_snake_case)]
pub fn BORDER_BOTTOM() -> u32 { get().border_bottom }

// Bar layout — five zones, each a Vec<Widget>. The widgets!() and group!()
// macros (defined in widgets/mod.rs) build these vectors.
pub fn bar() -> crate::bar::BarLayout {
    match profile() {
        "macbook" => macbook::bar(),
        "intel" => intel::bar(),
        _ => unreachable!(),
    }
}
```

- [ ] **Step 2: Write src/config/macbook.rs**

```rust
use crate::bar::BarLayout;
use crate::theme;
use crate::widgets::{
    Battery, BatteryDraw, Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp,
    CpuUsage, Fcitx, Memory, Minimap, Notch, PkgUpdate, Power, PsysDraw, Tray, Volume, Wifi,
    WindowTitle, Wireguard, Workspaces, group, widgets,
};

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
        bar_height: 38.0,
        border_top: t.bg,
        border_bottom: t.bg,
    }
}

pub(super) fn bar() -> BarLayout {
    BarLayout {
        left: widgets!(Workspaces, Minimap, WindowTitle),
        center_left: widgets!(
            group!(CpuFreq),
            group!(CpuUsage, |, CpuTemp),
            Memory
        ),
        center: widgets!(Notch),
        center_right: widgets!(
            Clock,
            Wifi,
            Bluetooth,
            PkgUpdate,
            group!(BatteryDraw, |, CpuDraw, |, PsysDraw)
        ),
        right: widgets!(
            Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power
        ),
    }
}
```

- [ ] **Step 3: Write src/config/intel.rs**

```rust
use crate::bar::BarLayout;
use crate::theme;
use crate::widgets::{
    Bluetooth, Brightness, CapsLock, Clock, CpuDraw, CpuFreq, CpuTemp, CpuUsage, Fcitx,
    GpuBusy, GpuDraw, Memory, Minimap, PkgUpdate, Power, Tray, Volume, Wifi, WindowTitle,
    Wireguard, Workspaces, group, widgets,
};

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
        center_left: widgets!(CpuFreq, group!(CpuUsage, |, CpuTemp), Memory),
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
```

(Intel does NOT include `Battery`, `BatteryDraw`, `PsysDraw`, or `Notch` — workstation, no battery, has notch-replacement Clock in center.)

- [ ] **Step 4: Commit**

```bash
git add src/config/
git commit -m "config: profile system with macbook + intel layouts"
```

## Task 0.4: Assets — copy icons + initial CSS

**Files:**
- Create: `assets/icons/` — copy entire dir from rs-bar
- Create: `assets/default-theme.css`

- [ ] **Step 1: Copy icons**

```bash
cp -r ~/Develop/rs-bar/assets/icons ~/Develop/rs-bar-relm4/assets/icons
```

- [ ] **Step 2: Write minimal initial CSS**

This is the minimum to get a working, recognizable bar. Phase 4 expands it.

```css
/* assets/default-theme.css — bundled default. Reference @rs_* colors that are
   defined at runtime from the active Theme struct. Users can override by
   editing ~/.config/rs-bar/gtk-theme.css. */

window.rs-bar {
    background-color: @rs_bg;
    color: @rs_fg;
    font-family: "CaskaydiaCove Nerd Font", monospace;
    font-weight: 600;
    font-size: 12px;
}

.bar-capsule, .bar-group {
    background-color: @rs_surface;
    border: 1px solid @rs_border;
    border-radius: 9999px;
    padding: 2px 4px;
    min-height: 0;
}

.bar-group separator {
    background-color: @rs_fg_gutter;
    min-width: 1px;
    margin: 4px 2px;
}

.bar-zone {
    /* horizontal, gap, padding */
    padding: 0 8px;
}

.bar-border-top, .bar-border-bottom {
    background-color: @rs_bg;
    min-height: 1px;
}
```

- [ ] **Step 3: Commit**

```bash
git add assets/
git commit -m "assets: copy icons and initial default-theme.css"
```

## Task 0.5: Style module

**Files:**
- Create: `src/style.rs`

- [ ] **Step 1: Write src/style.rs**

```rust
//! CSS pipeline: generate `@define-color` from Theme, append embedded default,
//! then user override. Loaded into a single CssProvider on the default Display.

use std::path::PathBuf;

use gdk::Display;
use gtk::prelude::*;

use crate::config;

const DEFAULT_CSS: &str = include_str!("../assets/default-theme.css");

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
    if std::fs::write(&path, DEFAULT_CSS).is_ok() {
        log::info!("wrote default theme to {}", path.display());
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
}
```

- [ ] **Step 2: Commit**

```bash
git add src/style.rs
git commit -m "style: CSS pipeline with theme color generation and user override"
```

## Task 0.6: Hub infrastructure + stubs

**Files:**
- Create: `src/hub/mod.rs`
- Create: `src/hub/sys.rs` — shared sysfs/timerfd helpers (lifted from rs-bar's power_draw.rs top section)
- Create stub files for each hub module

- [ ] **Step 1: Write src/hub/mod.rs**

```rust
//! Shared system-data hub. Each submodule owns a singleton OS thread that
//! reads from sysfs/procfs/IPC and publishes via a `tokio::sync::watch::Sender`.
//! Widgets subscribe with `<module>::subscribe()`.
//!
//! First call to `subscribe()` lazily spawns the OS thread. Multi-monitor
//! setups still get exactly one poller per data source — `tx.subscribe()`
//! returns a fresh receiver cheaply.

pub mod sys;

pub mod cpu_usage;
pub mod cpu_temp;
pub mod cpu_freq;
pub mod memory;
pub mod gpu_busy;
pub mod power_draw;
pub mod niri;
pub mod tray;
```

- [ ] **Step 2: Write src/hub/sys.rs**

Lift the helper section from `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs` (the top-of-file `sysfs_u64`, `sysfs_i64`, `sysfs_str`, `timerfd_loop` helpers). Make all functions `pub`. Drop the `pub(crate)` qualifiers and `super`/`crate` imports. Keep the libc imports.

```rust
//! Shared low-level helpers used by hub poller threads:
//! - sysfs file readers (u64 / i64 / String)
//! - timerfd + epoll loop for periodic polling
//!
//! No dependency on hub channels; pure libc + std::fs.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

pub fn sysfs_u64(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn sysfs_i64(path: &Path) -> Option<i64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn sysfs_str(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn sysfs_readable(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok()
}

/// Run `tick` every `interval_secs` on a timerfd + epoll loop.
/// Returns when `tick` returns `false`.
pub fn timerfd_loop(interval_secs: i64, fire_immediately: bool, mut tick: impl FnMut() -> bool) {
    let tfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_CLOEXEC) };
    if tfd < 0 {
        return;
    }
    let tfd = unsafe { OwnedFd::from_raw_fd(tfd) };

    let spec = libc::itimerspec {
        it_interval: libc::timespec { tv_sec: interval_secs, tv_nsec: 0 },
        it_value: libc::timespec {
            tv_sec: if fire_immediately { 0 } else { interval_secs },
            tv_nsec: if fire_immediately { 1 } else { 0 },
        },
    };
    unsafe { libc::timerfd_settime(tfd.as_raw_fd(), 0, &spec, std::ptr::null_mut()) };

    let epfd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epfd < 0 {
        return;
    }
    let epfd = unsafe { OwnedFd::from_raw_fd(epfd) };

    let mut ev = libc::epoll_event { events: libc::EPOLLIN as u32, u64: 0 };
    unsafe {
        libc::epoll_ctl(epfd.as_raw_fd(), libc::EPOLL_CTL_ADD, tfd.as_raw_fd(), &mut ev);
    }

    loop {
        let mut out = [libc::epoll_event { events: 0, u64: 0 }; 1];
        let n = unsafe { libc::epoll_wait(epfd.as_raw_fd(), out.as_mut_ptr(), 1, -1) };
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        let mut buf = [0u8; 8];
        unsafe { libc::read(tfd.as_raw_fd(), buf.as_mut_ptr().cast(), 8) };

        if !tick() {
            break;
        }
    }
}
```

- [ ] **Step 3: Stub each hub module**

For each of: `cpu_usage`, `cpu_temp`, `cpu_freq`, `memory`, `gpu_busy`, `power_draw`, `niri`, `tray` — create `src/hub/<name>.rs` with the minimum to compile:

```rust
//! STUB. Will be filled in by Phase 1 (cpu_usage) or Phase 2.

use std::sync::OnceLock;
use tokio::sync::watch;

// For most modules the value type is f32; tray and niri define their own.
fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        // No producer thread spawned in the stub.
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<f32> {
    sender().subscribe()
}
```

For `hub/niri.rs` — initial stub with the snapshot type defined (so widgets can name it):

```rust
//! STUB. Will be replaced in Phase 2.

use std::sync::OnceLock;
use tokio::sync::watch;

#[derive(Clone, Default)]
pub struct NiriSnapshot {
    pub workspaces: Vec<niri_ipc::Workspace>,
    pub windows: Vec<niri_ipc::Window>,
    pub outputs: Vec<niri_ipc::Output>,
    pub overview_open: bool,
}

fn sender() -> &'static watch::Sender<NiriSnapshot> {
    static S: OnceLock<watch::Sender<NiriSnapshot>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(NiriSnapshot::default());
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<NiriSnapshot> {
    sender().subscribe()
}
```

For `hub/tray.rs` — initial stub with a placeholder `TrayState` type:

```rust
//! STUB. Will be replaced in Phase 2.

use std::sync::OnceLock;
use tokio::sync::watch;

#[derive(Clone, Default)]
pub struct TrayState {
    pub items: Vec<TrayItem>,
}

#[derive(Clone)]
pub struct TrayItem {
    pub id: String,
    pub title: String,
    pub icon_pixbuf: Option<gdk::gdk_pixbuf::Pixbuf>,
    pub menu: Option<gio::MenuModel>,
}

fn sender() -> &'static watch::Sender<TrayState> {
    static S: OnceLock<watch::Sender<TrayState>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(TrayState::default());
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<TrayState> {
    sender().subscribe()
}
```

For `hub/power_draw.rs` — initial stub with the four-channel struct:

```rust
//! STUB. Will be replaced in Phase 2.

use std::sync::OnceLock;
use tokio::sync::watch;

#[derive(Clone, Copy, Default)]
pub struct PowerDrawSample {
    pub battery_w: Option<f64>,
    pub cpu_w: Option<f64>,
    pub psys_w: Option<f64>,
    pub gpu_w: Option<f64>,
}

fn sender() -> &'static watch::Sender<PowerDrawSample> {
    static S: OnceLock<watch::Sender<PowerDrawSample>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(PowerDrawSample::default());
        tx
    })
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<PowerDrawSample> {
    sender().subscribe()
}
```

(Phase 2 may replace this with four separate channels — TBD by the agent that implements `hub/power_draw.rs`. The widgets in Phase 3 read `PowerDrawSample` regardless and pull out their field.)

- [ ] **Step 4: Commit**

```bash
git add src/hub/
git commit -m "hub: scaffolding + stubs for all data sources"
```

## Task 0.7: Widget abstraction + stubs (one file per widget)

**Files:**
- Create: `src/widgets/mod.rs`
- Create: `src/widgets/<name>.rs` (×23, all stubs)

- [ ] **Step 1: Write src/widgets/mod.rs**

```rust
//! Widget framework. Each widget is a relm4 Component with `const NAME` and
//! `Init = WidgetInit`. The `widgets!()` and `group!()` macros build a
//! `Vec<Widget>` for a bar zone, type-erasing each Component's controller.

use gtk::prelude::*;
use relm4::prelude::*;

mod battery;
mod battery_draw;
mod bluetooth;
mod brightness;
mod capslock;
mod clock;
mod cpu_draw;
mod cpu_freq;
mod cpu_temp;
mod cpu_usage;
mod date;
mod fcitx;
mod gpu_busy;
mod gpu_draw;
mod memory;
mod minimap;
mod notch;
mod pkg_update;
mod power;
mod psys_draw;
mod tray;
mod volume;
mod wifi;
mod window_title;
mod wireguard;
mod workspaces;

pub use battery::Battery;
pub use battery_draw::BatteryDraw;
pub use bluetooth::Bluetooth;
pub use brightness::Brightness;
pub use capslock::CapsLock;
pub use clock::Clock;
pub use cpu_draw::CpuDraw;
pub use cpu_freq::CpuFreq;
pub use cpu_temp::CpuTemp;
pub use cpu_usage::CpuUsage;
pub use date::Date;
pub use fcitx::Fcitx;
pub use gpu_busy::GpuBusy;
pub use gpu_draw::GpuDraw;
pub use memory::Memory;
pub use minimap::Minimap;
pub use notch::Notch;
pub use pkg_update::PkgUpdate;
pub use power::Power;
pub use psys_draw::PsysDraw;
pub use tray::Tray;
pub use volume::Volume;
pub use wifi::Wifi;
pub use window_title::WindowTitle;
pub use wireguard::Wireguard;
pub use workspaces::Workspaces;

/// Init payload all widgets accept. `grouped` true means the widget skips its
/// own capsule wrapper because a parent Group provides one.
#[derive(Clone, Copy, Default)]
pub struct WidgetInit {
    pub grouped: bool,
}

/// Type-erased handle to a launched widget Component. `root` is the GTK widget
/// to attach into the bar layout; `_controller` keeps the Controller alive.
pub struct Widget {
    pub name: &'static str,
    pub root: gtk::Widget,
    _controller: Box<dyn std::any::Any>,
}

/// Trait implemented by all widget Components for stable name access.
pub trait NamedWidget: Component<Init = WidgetInit> {
    const NAME: &'static str;
}

/// Build a widget by launching its Component. Returns a type-erased Widget.
pub fn build<C>(grouped: bool) -> Widget
where
    C: NamedWidget + 'static,
    C::Root: glib::IsA<gtk::Widget> + Clone,
{
    let controller = C::builder().launch(WidgetInit { grouped }).detach();
    let root: gtk::Widget = controller.widget().clone().upcast();
    Widget {
        name: C::NAME,
        root,
        _controller: Box::new(controller),
    }
}

// ── Group widget ───────────────────────────────────────────────────────

pub enum GroupEntry {
    Widget(gtk::Widget, Box<dyn std::any::Any>),
    Separator,
}

/// Build a group: a horizontal Box with `.bar-group` class containing
/// child widgets and gtk::Separator between them where `|` was used.
pub fn build_group(entries: Vec<GroupEntry>) -> Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("bar-group");

    let mut keepers: Vec<Box<dyn std::any::Any>> = Vec::new();

    for entry in entries {
        match entry {
            GroupEntry::Widget(w, keeper) => {
                row.append(&w);
                keepers.push(keeper);
            }
            GroupEntry::Separator => {
                let sep = gtk::Separator::new(gtk::Orientation::Vertical);
                row.append(&sep);
            }
        }
    }

    Widget {
        name: "group",
        root: row.upcast(),
        _controller: Box::new(keepers),
    }
}

/// Apply capsule styling to a widget root. Widgets call this in their `init`
/// when `!grouped`. Adds the `bar-capsule` CSS class.
pub fn capsule(w: &impl glib::IsA<gtk::Widget>, grouped: bool) {
    if !grouped {
        w.add_css_class("bar-capsule");
    }
}

// ── Macros ─────────────────────────────────────────────────────────────

/// Build a single grouped widget for use inside `group!()`.
#[doc(hidden)]
pub fn build_for_group<C>() -> GroupEntry
where
    C: NamedWidget + 'static,
    C::Root: glib::IsA<gtk::Widget> + Clone,
{
    let controller = C::builder().launch(WidgetInit { grouped: true }).detach();
    let root: gtk::Widget = controller.widget().clone().upcast();
    GroupEntry::Widget(root, Box::new(controller))
}

/// Construct a Group widget containing the given children, separated by `|`.
///
///     group!(CpuFreq, CpuUsage, |, CpuTemp)
#[macro_export]
macro_rules! group {
    (@item $entries:ident, |) => {
        $entries.push($crate::widgets::GroupEntry::Separator);
    };
    (@item $entries:ident, $w:ident) => {
        $entries.push($crate::widgets::build_for_group::<$crate::widgets::$w>());
    };
    ($($item:tt),* $(,)?) => {{
        let mut entries: Vec<$crate::widgets::GroupEntry> = Vec::new();
        $($crate::group!(@item entries, $item);)*
        $crate::widgets::build_group(entries)
    }};
}

/// Build a `Vec<Widget>` mixing plain widgets and `group!()` calls.
///
///     widgets!(Workspaces, group!(CpuUsage, |, CpuTemp), Memory)
#[macro_export]
macro_rules! widgets {
    (@acc [$($out:expr),*]) => {
        vec![$($out),*]
    };
    (@acc [$($out:expr),*] $crate:tt :: group!($($g:tt)*) , $($rest:tt)*) => {
        $crate::widgets!(@acc [$($out,)* $crate::group!($($g)*)] $($rest)*)
    };
    (@acc [$($out:expr),*] group!($($g:tt)*) , $($rest:tt)*) => {
        $crate::widgets!(@acc [$($out,)* $crate::group!($($g)*)] $($rest)*)
    };
    (@acc [$($out:expr),*] group!($($g:tt)*)) => {
        $crate::widgets!(@acc [$($out,)* $crate::group!($($g)*)])
    };
    (@acc [$($out:expr),*] $w:ident , $($rest:tt)*) => {
        $crate::widgets!(@acc [$($out,)* $crate::widgets::build::<$crate::widgets::$w>(false)] $($rest)*)
    };
    (@acc [$($out:expr),*] $w:ident) => {
        $crate::widgets!(@acc [$($out,)* $crate::widgets::build::<$crate::widgets::$w>(false)])
    };
    ($($items:tt)*) => {
        $crate::widgets!(@acc [] $($items)*)
    };
}

pub use group;
pub use widgets;
```

- [ ] **Step 2: Write 23 widget stubs**

For each name in this list, create `src/widgets/<name>.rs` with the stub below, substituting the type name:

```
battery, battery_draw, bluetooth, brightness, capslock, clock, cpu_draw,
cpu_freq, cpu_temp, cpu_usage, date, fcitx, gpu_busy, gpu_draw, memory,
minimap, notch, pkg_update, power, psys_draw, tray, volume, wifi,
window_title, wireguard, workspaces
```

Stub template (substitute `Notch` → the proper struct name; `notch` → file basename; both `NAME` constants likewise). The struct name is the upper-camel-case form: `notch` → `Notch`, `cpu_usage` → `CpuUsage`, `battery_draw` → `BatteryDraw`, etc.

```rust
//! STUB. Phase 1 (cpu_usage) or Phase 3 will replace this.

use gtk::prelude::*;
use relm4::prelude::*;

use super::{NamedWidget, WidgetInit, capsule};

pub struct Notch {
    grouped: bool,
}

#[derive(Debug)]
pub enum NotchMsg {}

#[relm4::component(pub)]
impl SimpleComponent for Notch {
    type Init = WidgetInit;
    type Input = NotchMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
        }
    }

    fn init(init: Self::Init, root: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let model = Notch { grouped: init.grouped };
        let widgets = view_output!();
        capsule(&root, model.grouped);
        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>) {}
}

impl NamedWidget for Notch {
    const NAME: &'static str = "notch";
}
```

For widgets that have no state at all (`Notch`, `Date`), the stub above is the final implementation modulo CSS (handled in Phase 4). For all others Phase 3 will replace the stub.

Write all 23 files now. Use the canonical name list above; every stub renders an empty `gtk::Box`.

- [ ] **Step 3: cargo build to verify**

```bash
cd ~/Develop/rs-bar-relm4
cargo check 2>&1 | tail -40
```

Expected: clean compile, only `dead_code` warnings allowed.

- [ ] **Step 4: Commit**

```bash
git add src/widgets/
git commit -m "widgets: framework + 23 widget stubs"
```

## Task 0.8: Bar Component (layer-shell, zones)

**Files:**
- Create: `src/bar.rs`

- [ ] **Step 1: Write src/bar.rs**

```rust
//! Bar Component — one per monitor. Owns the layer-shell ApplicationWindow
//! and renders the five zones (left, center_left, center, center_right, right).

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use relm4::prelude::*;

use crate::config;
use crate::widgets::Widget;

/// The five-zone layout produced by config::bar().
pub struct BarLayout {
    pub left: Vec<Widget>,
    pub center_left: Vec<Widget>,
    pub center: Vec<Widget>,
    pub center_right: Vec<Widget>,
    pub right: Vec<Widget>,
}

pub struct BarInit {
    pub monitor: gdk::Monitor,
    pub layout: BarLayout,
}

pub struct Bar {
    _layout: BarLayout, // keep widgets alive; window owns their roots
}

#[derive(Debug)]
pub enum BarMsg {}

pub struct BarWidgets {
    pub window: gtk::ApplicationWindow,
}

impl Component for Bar {
    type Init = BarInit;
    type Input = BarMsg;
    type Output = ();
    type CommandOutput = ();
    type Root = gtk::ApplicationWindow;
    type Widgets = BarWidgets;

    fn init_root() -> Self::Root {
        let app = relm4::main_application();
        let window = gtk::ApplicationWindow::new(&app);
        window.add_css_class("rs-bar");
        window.init_layer_shell();
        window.set_layer(Layer::Top);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);
        window.set_exclusive_zone(config::BAR_HEIGHT() as i32);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_namespace("rs-bar");
        window.set_default_size(1, config::BAR_HEIGHT() as i32);
        window
    }

    fn init(init: Self::Init, window: Self::Root, _sender: ComponentSender<Self>) -> ComponentParts<Self> {
        window.set_monitor(Some(&init.monitor));

        let row = gtk::CenterBox::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();

        // Left half: left + center_left
        let left_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        left_half.set_hexpand(true);
        let left_zone = build_zone(&init.layout.left, gtk::Align::Start);
        let center_left_zone = build_zone(&init.layout.center_left, gtk::Align::End);
        center_left_zone.set_hexpand(true);
        left_half.append(&left_zone);
        left_half.append(&center_left_zone);

        // Center
        let center_zone = build_zone(&init.layout.center, gtk::Align::Center);

        // Right half: center_right + right
        let right_half = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        right_half.set_hexpand(true);
        let center_right_zone = build_zone(&init.layout.center_right, gtk::Align::Start);
        let right_zone = build_zone(&init.layout.right, gtk::Align::End);
        right_zone.set_hexpand(true);
        right_half.append(&center_right_zone);
        right_half.append(&right_zone);

        row.set_start_widget(Some(&left_half));
        row.set_center_widget(Some(&center_zone));
        row.set_end_widget(Some(&right_half));

        // Outer overlay for borders
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&row));

        let top_border = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top_border.add_css_class("bar-border-top");
        top_border.set_valign(gtk::Align::Start);
        top_border.set_hexpand(true);
        overlay.add_overlay(&top_border);

        let bottom_border = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        bottom_border.add_css_class("bar-border-bottom");
        bottom_border.set_valign(gtk::Align::End);
        bottom_border.set_hexpand(true);
        overlay.add_overlay(&bottom_border);

        window.set_child(Some(&overlay));
        window.present();

        let model = Bar { _layout: init.layout };
        let widgets = BarWidgets { window: window.clone() };
        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>) {}
}

fn build_zone(widgets: &[Widget], align: gtk::Align) -> gtk::Box {
    let zone = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    zone.add_css_class("bar-zone");
    zone.set_halign(align);
    zone.set_valign(gtk::Align::Center);
    for w in widgets {
        zone.append(&w.root);
    }
    zone
}
```

- [ ] **Step 2: Commit**

```bash
git add src/bar.rs
git commit -m "bar: Bar Component with layer-shell, five-zone CenterBox layout"
```

## Task 0.9: App Component + monitor enumeration

**Files:**
- Create: `src/app.rs`

- [ ] **Step 1: Write src/app.rs**

```rust
//! Root App component. On init, opens one Bar per GdkMonitor and subscribes
//! to monitors `items-changed` for hot-plug. The App owns Controller<Bar>
//! handles in a HashMap keyed by monitor.

use std::collections::HashMap;
use std::time::Duration;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::bar::{Bar, BarInit};
use crate::config;
use crate::style;

pub struct App {
    bars: HashMap<gdk::Monitor, Controller<Bar>>,
}

#[derive(Debug)]
pub enum AppMsg {
    EnumerateMonitors,
    MonitorAdded(gdk::Monitor),
    MonitorRemoved(gdk::Monitor),
}

impl Component for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = ();
    type Root = gtk::Window;
    type Widgets = ();

    fn init_root() -> Self::Root {
        // Hidden management window — relm4 RelmApp expects a root, but our
        // bars open their own ApplicationWindows. This window is never shown.
        gtk::Window::new()
    }

    fn init(_: Self::Init, _root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        style::load();

        let display = gdk::Display::default().expect("no default GdkDisplay");
        let monitors = display.monitors();

        // Enumerate after a short delay (matches rs-bar's GPUI workaround for
        // late display enumeration; harmless if monitors are already known).
        let s = sender.clone();
        glib::timeout_add_local_once(Duration::from_millis(100), move || {
            s.input(AppMsg::EnumerateMonitors);
        });

        // Hot-plug: subscribe to items-changed on the monitors list.
        let s = sender.clone();
        let monitors_clone = monitors.clone();
        monitors.connect_items_changed(move |list, position, removed, added| {
            // Removed: indices [position, position+removed) — but the items
            // are already gone from the list. We track existing bars in the
            // App map and reconcile on each event by re-enumerating.
            let _ = (list, position, removed, added);
            s.input(AppMsg::EnumerateMonitors);
            let _ = &monitors_clone; // keep alive
        });

        ComponentParts {
            model: App { bars: HashMap::new() },
            widgets: (),
        }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            AppMsg::EnumerateMonitors => {
                let display = gdk::Display::default().expect("no default GdkDisplay");
                let list = display.monitors();
                let n = list.n_items();
                let mut current: Vec<gdk::Monitor> = (0..n)
                    .filter_map(|i| list.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
                    .collect();

                if current.is_empty() {
                    log::warn!("No monitors detected; nothing to do.");
                    return;
                }

                // Add bars for new monitors
                for m in &current {
                    if !self.bars.contains_key(m) {
                        log::info!("opening bar on {}", m.connector().unwrap_or_default());
                        let controller = Bar::builder()
                            .launch(BarInit {
                                monitor: m.clone(),
                                layout: config::bar(),
                            })
                            .detach();
                        self.bars.insert(m.clone(), controller);
                    }
                }

                // Remove bars for vanished monitors
                let still_present: std::collections::HashSet<_> = current.drain(..).collect();
                self.bars.retain(|m, _| still_present.contains(m));
            }
            AppMsg::MonitorAdded(_) | AppMsg::MonitorRemoved(_) => {
                // Reserved — currently we always re-enumerate.
            }
        }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/app.rs
git commit -m "app: root component with monitor enumeration and hot-plug"
```

## Task 0.10: main.rs

**Files:**
- Create: `src/main.rs`

- [ ] **Step 1: Write src/main.rs**

```rust
mod app;
mod bar;
mod config;
mod hub;
mod style;
mod theme;
mod widgets;

use env_logger::Env;

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.to_string();
        if msg.contains("no reactor running") {
            return;
        }
        default_hook(info);
    }));
}

fn main() {
    let env = Env::new().filter("RS_BAR_LOG").write_style("RS_BAR");
    env_logger::init_from_env(env);
    install_panic_hook();

    config::init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let _guard = rt.enter();

    let app = relm4::RelmApp::new("dev.rs-bar.relm4");
    app.run::<app::App>(());
}
```

- [ ] **Step 2: cargo build (full build)**

```bash
cd ~/Develop/rs-bar-relm4
cargo build 2>&1 | tail -60
```

Expected: clean build (warnings about unused stubs are OK).

- [ ] **Step 3: Run on each profile**

```bash
RS_BAR_LOG=info cargo run -- --config macbook
```

Expected behavior:
- One bar per monitor opens at the top edge.
- Bars are 38px tall (macbook profile).
- Bars are empty (no widgets visible since most are stubs).
- Background is Nord polar-night (`#2E3440`).
- Disconnect a monitor → its bar disappears. Reconnect → bar reappears.

```bash
RS_BAR_LOG=info cargo run -- --config intel
```

Expected: same, but 30px tall.

If anything fails, fix in this task — Phase 0 must end with a runnable empty bar on both profiles.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "main: tokio runtime + RelmApp boot, all profiles run"
```

---

# Phase 1 — Reference Pattern (sequential)

Phase 1 ports a single end-to-end widget (`CpuUsage`) to establish the canonical pattern. All Phase 3 widget tasks reference this as the template.

## Task 1.1: hub/cpu_usage.rs

**Files:**
- Modify: `src/hub/cpu_usage.rs`

**Source of truth:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_usage.rs`

- [ ] **Step 1: Replace stub with port**

Read `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_usage.rs`. Lift the entire system-reading half (lines 21..=154 in the source: `CpuTimes` struct, `read_cpu_times`, `compute_usage`, `cpu_monitor`). The `cpu_monitor` body uses timerfd+epoll directly — replace that with the shared `hub::sys::timerfd_loop` helper. The publish call changes from `bc.publish(usage)` to `let _ = tx.send(usage);`.

Final file:

```rust
//! CPU usage hub. /proc/stat reader on a 1-second timerfd, publishes %.

use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    user: u64, nice: u64, system: u64, idle: u64,
    iowait: u64, irq: u64, softirq: u64, steal: u64,
}

impl CpuTimes {
    fn total(&self) -> u64 {
        self.user + self.nice + self.system + self.idle
            + self.iowait + self.irq + self.softirq + self.steal
    }
    fn idle_total(&self) -> u64 { self.idle + self.iowait }
}

fn read_cpu_times() -> CpuTimes {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    for line in stat.lines() {
        if let Some(rest) = line.strip_prefix("cpu ") {
            let vals: Vec<u64> = rest.split_whitespace()
                .filter_map(|s| s.parse().ok()).collect();
            if vals.len() >= 8 {
                return CpuTimes {
                    user: vals[0], nice: vals[1], system: vals[2], idle: vals[3],
                    iowait: vals[4], irq: vals[5], softirq: vals[6], steal: vals[7],
                };
            }
        }
    }
    CpuTimes::default()
}

fn compute_usage(prev: &CpuTimes, cur: &CpuTimes) -> f32 {
    let dt = cur.total().saturating_sub(prev.total());
    if dt == 0 { return 0.0; }
    let di = cur.idle_total().saturating_sub(prev.idle_total());
    ((dt - di) as f32 / dt as f32) * 100.0
}

fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("cpu-usage".into())
            .spawn(move || {
                let mut prev = read_cpu_times();
                timerfd_loop(1, false, || {
                    let cur = read_cpu_times();
                    let usage = compute_usage(&prev, &cur);
                    prev = cur;
                    producer.send(usage).is_ok()
                });
            })
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<f32> {
    sender().subscribe()
}
```

- [ ] **Step 2: cargo build**

```bash
cargo build 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add src/hub/cpu_usage.rs
git commit -m "hub: cpu_usage poller (canonical pattern)"
```

## Task 1.2: widgets/cpu_usage.rs

**Files:**
- Modify: `src/widgets/cpu_usage.rs`

**Source of truth:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_usage.rs` (the BarWidget impl, lines 158..=232)

- [ ] **Step 1: Replace stub with full implementation**

```rust
//! CPU usage widget. Subscribes to hub::cpu_usage and renders an icon + %.

use std::sync::OnceLock;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::config;
use crate::hub;

use super::{NamedWidget, WidgetInit, capsule};

const ICON_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/cpu-usage.svg");

fn cached_texture() -> &'static gdk::Texture {
    static T: OnceLock<gdk::Texture> = OnceLock::new();
    T.get_or_init(|| gdk::Texture::from_filename(ICON_PATH).expect("icon load"))
}

pub struct CpuUsage {
    usage: f32,
    grouped: bool,
    icon: gtk::Image,
    label: gtk::Label,
}

#[derive(Debug)]
pub enum CpuUsageMsg {
    Update(f32),
}

#[relm4::component(pub)]
impl SimpleComponent for CpuUsage {
    type Init = WidgetInit;
    type Input = CpuUsageMsg;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_spacing: 4,
            set_valign: gtk::Align::Center,
            #[name = "icon"]
            gtk::Image {
                set_paintable: Some(cached_texture()),
                set_pixel_size: config::ICON_SIZE() as i32,
            },
            #[name = "label"]
            gtk::Label {
                set_label: "0%",
            },
        }
    }

    fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let widgets = view_output!();
        let model = CpuUsage {
            usage: 0.0,
            grouped: init.grouped,
            icon: widgets.icon.clone(),
            label: widgets.label.clone(),
        };

        capsule(&root, model.grouped);

        // Subscription: bridge watch::Receiver<f32> to component messages.
        let mut rx = hub::cpu_usage::subscribe();
        let s = sender.clone();
        relm4::spawn_local(async move {
            while rx.changed().await.is_ok() {
                let v = *rx.borrow_and_update();
                s.input(CpuUsageMsg::Update(v));
            }
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            CpuUsageMsg::Update(usage) => {
                let new_pct = usage.round() as u32;
                let old_pct = self.usage.round() as u32;
                if new_pct == old_pct {
                    return;
                }
                self.usage = usage;
                self.label.set_label(&format!("{:>2}%", new_pct));

                // Color band — same thresholds as rs-bar
                let class = if usage >= 80.0 { "cpu-usage-crit" }
                       else if usage >= 60.0 { "cpu-usage-warn" }
                       else if usage >= 25.0 { "cpu-usage-norm" }
                       else { "cpu-usage-dim" };
                for c in ["cpu-usage-crit", "cpu-usage-warn", "cpu-usage-norm", "cpu-usage-dim"] {
                    if c != class { self.label.remove_css_class(c); self.icon.remove_css_class(c); }
                }
                self.label.add_css_class(class);
                self.icon.add_css_class(class);
            }
        }
    }
}

impl NamedWidget for CpuUsage {
    const NAME: &'static str = "cpu-usage";
}
```

- [ ] **Step 2: Add color-band CSS to assets/default-theme.css**

Append:

```css
.cpu-usage-crit { color: @rs_red; }
.cpu-usage-warn { color: @rs_orange; }
.cpu-usage-norm { color: @rs_fg; }
.cpu-usage-dim  { color: @rs_fg_dark; }
```

- [ ] **Step 3: cargo build + run**

```bash
cargo build 2>&1 | tail -10
RS_BAR_LOG=info cargo run -- --config macbook
```

Expected: bar opens, the CpuUsage widget shows in the center-left zone, updates every second. Number is right-justified two characters wide.

Run `htop` in another terminal — verify the percent tracks.

Run on the second monitor too. Verify only one `cpu-usage` thread name in `ps -eL | grep cpu-usage`.

- [ ] **Step 4: Commit**

```bash
git add src/widgets/cpu_usage.rs assets/default-theme.css
git commit -m "widgets: CpuUsage (canonical pattern)"
```

## Task 1.3: Phase 1 wrap-up — document the pattern

This is a verification step, not new code.

- [ ] **Step 1: Confirm pattern parts**

Read `src/widgets/cpu_usage.rs` again. The pattern that all Phase 3 tasks must follow:

1. **Module-level cached resources**: SVG textures via `OnceLock<gdk::Texture>` (one parse, shared across all bar instances).
2. **Model fields**: held GTK widgets (`gtk::Image`, `gtk::Label`, etc.) needed in `update`, plus `grouped: bool` and the displayed value(s).
3. **Message enum**: at minimum an `Update(T)` variant matching the hub's value type.
4. **`#[relm4::component(pub)]`** + `view!` for declarative layout.
5. **`init`**: build view, apply `capsule(&root, grouped)`, subscribe via `relm4::spawn_local` + `rx.borrow_and_update()`.
6. **`update`**: short-circuit on no-display-change, then update GTK widgets directly (label text, css classes). Don't recreate widgets.
7. **`impl NamedWidget`** with `const NAME`.

8. **CSS class transitions**: when a widget toggles between mutually-exclusive classes (color bands, on/off states), use a helper or explicit `remove_css_class` for siblings before `add_css_class`. Don't accumulate stale classes.

- [ ] **Step 2: No commit** — this is a checkpoint only.

---

# Phase 2 — Hub modules (PARALLEL)

Seven independent tasks. Each fills in one `src/hub/<name>.rs` from a stub to a complete poller. Each task touches exactly one file. They can run concurrently. The widgets in Phase 3 depend on these.

**Common pattern for every Phase 2 task:**
1. Read the corresponding rs-bar source file under `~/Develop/rs-bar/src/gpui_bar/widgets/<name>.rs` (or `niri.rs`/`tray.rs` from `gpui_bar/`).
2. Lift the system-reading code (everything not GPUI/widget-render related).
3. Replace `Broadcast<T>::publish` with `tokio::sync::watch::Sender<T>::send`.
4. Replace inline timerfd+epoll with `hub::sys::timerfd_loop` helper where applicable.
5. Expose `pub fn subscribe() -> watch::Receiver<T>`.
6. Build with `cargo build`.
7. Commit one file at a time.

**Pattern reference:** `src/hub/cpu_usage.rs` (from Phase 1).

## Task 2.1: hub/cpu_temp.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_temp.rs`. Lift the hwmon temp_input reader, package-temp selection logic, fallback paths. Publish `f32` (degrees C). 1-second polling.

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: cpu_temp poller"`

## Task 2.2: hub/cpu_freq.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_freq.rs`. /proc/cpuinfo MHz averaging or sysfs `scaling_cur_freq` paths. Publish `f32` (GHz). Polling rate per source.

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: cpu_freq poller"`

## Task 2.3: hub/memory.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/memory.rs`. /proc/meminfo parser. Publish `f32` percent or `(used_gb, total_gb)` — match whatever shape rs-bar uses. Confirm the type from the rs-bar widget; agents must not invent a new shape.

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: memory poller"`

## Task 2.4: hub/gpu_busy.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/gpu_busy.rs`. AMDGPU `/sys/class/drm/card*/device/gpu_busy_percent`, with whatever fallbacks rs-bar implements. Publish `f32` (percent).

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: gpu_busy poller"`

## Task 2.5: hub/power_draw.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs`. Lines 1..=440 are infrastructure to lift (sysfs helpers already moved to `hub/sys.rs` in Phase 0; the rest is the four energy-uJ readers and shared sample logic). The four widgets each had their own poller in rs-bar — preserve that as four separate timerfd loops (or coalesce into one as a perf win, see Section 2 of the spec — agent's choice).

The published type is `PowerDrawSample` (already declared in the Phase 0 stub):

```rust
pub struct PowerDrawSample {
    pub battery_w: Option<f64>,
    pub cpu_w: Option<f64>,
    pub psys_w: Option<f64>,
    pub gpu_w: Option<f64>,
}
```

The four widget files in Phase 3 will read one field each. Subscribe interface stays as `pub fn subscribe() -> watch::Receiver<PowerDrawSample>`.

If `nvidia-smi` is needed for GPU power as a fallback, shell out via `tokio::process::Command` from a tokio task spawned in the producer thread (the producer thread has access to the runtime via `tokio::runtime::Handle::current()` if entered, otherwise spawn a `Handle::current().block_on(...)`). Or, simpler: do a blocking `std::process::Command::output()` from the poller thread. Match what rs-bar does.

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: power_draw poller (battery/cpu/psys/gpu watts)"`

## Task 2.6: hub/niri.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/niri.rs` (note: `niri.rs` is at the gpui_bar root level, not under `widgets/`).

The published type `NiriSnapshot` already exists in the stub. Lift the listener thread verbatim:
- One-shot socket fetch of initial outputs and windows.
- Main event-stream socket.
- `WorkspacesState` / `WindowsState` apply loop.
- Overview tracking.
- Replace `Broadcast::publish` with `tx.send`.

The publish helper (named `publish` in rs-bar) builds the `NiriSnapshot` from current state pieces — keep it.

The GPUI-specific `NiriState` global and `start_listener(cx: &mut App)` function are NOT needed in relm4 — widgets that care about overview state subscribe to the same channel and read `snapshot.overview_open`. Drop those pieces.

- [ ] Replace stub.
- [ ] Build.
- [ ] Commit: `git commit -m "hub: niri event stream listener"`

## Task 2.7: hub/tray.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/tray.rs`.

This is the most involved. The rs-bar tray widget mixes the StatusNotifier listener (`system-tray` crate) with rendering. Split:

**Hub side** (`hub/tray.rs`):
- StatusNotifier client setup (uses zbus via system-tray crate).
- Maintain a `HashMap<id, item>` of current tray items.
- For each item, decode the icon to a `gdk::gdk_pixbuf::Pixbuf` (use `png` crate or `gdk_pixbuf::Pixbuf::from_read`).
- For each item that has a menu, build a `gio::MenuModel` (or store the raw menu data — a relm4-friendly type).
- Publish the `TrayState` snapshot (already declared in stub) on every change.

The system-tray crate is async-tokio so the listener thread can run a tokio Runtime via `tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()` and `block_on(async { … })`.

**Note:** Building `gio::MenuModel` requires the GTK main thread. Either:
- (a) Send raw menu data over the channel and have the widget side build the MenuModel on the main thread, OR
- (b) Use a different menu representation in `TrayState` and convert on the widget side.

Option (b) is cleaner. Define in this file:

```rust
#[derive(Clone)]
pub enum TrayMenuEntry {
    Item { id: i32, label: String, enabled: bool },
    Submenu { label: String, children: Vec<TrayMenuEntry> },
    Separator,
}
```

And update the existing stub's `TrayItem` to use `Vec<TrayMenuEntry>` instead of `gio::MenuModel`. The tray widget in Phase 3 builds a `gio::Menu` from this on the main thread.

Update the stub accordingly and implement.

- [ ] Replace stub (and update the `TrayItem` shape).
- [ ] Build.
- [ ] Commit: `git commit -m "hub: tray StatusNotifier listener"`

---

# Phase 3 — Widgets (PARALLEL)

22 widget tasks (CpuUsage was done in Phase 1). Each task touches exactly one `src/widgets/<name>.rs` file. They can run concurrently.

**Pattern reference:** `src/widgets/cpu_usage.rs` (from Phase 1). Read it before starting any Phase 3 task — it's the canonical relm4 widget shape.

**Common per-task instructions:**
1. Read the corresponding `~/Develop/rs-bar/src/gpui_bar/widgets/<name>.rs`.
2. Replace the stub in `src/widgets/<name>.rs` with the full implementation.
3. Pattern: model + message enum + `view!` + `init` (subscribe) + `update` (mutate widgets, toggle CSS classes).
4. Where rs-bar used `rgb(t.color)` inline, define a CSS class (in `default-theme.css`) and toggle it.
5. Cache SVG icons in `OnceLock<gdk::Texture>` per icon path.
6. For widgets with a popup, use `gtk::Popover::set_parent(&trigger)` + `popover.popup()` on click. Build popover content lazily on first popup if it's expensive.
7. Build with `cargo build`. Run with `cargo run -- --config macbook` (or intel) and visually verify.
8. Commit one file per task.

For each task below, the **Source** path is the rs-bar file to port, and **Notes** lists per-widget specifics. Read the source first.

## Task 3.1: widgets/cpu_temp.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_temp.rs`

**Notes:** Subscribes to `hub::cpu_temp`. Icon is `assets/icons/thermometer.svg`. Color bands by temperature (rs-bar has thresholds — preserve them as CSS classes `.cpu-temp-cool`, `.cpu-temp-warm`, `.cpu-temp-hot`).

- [ ] Replace stub. Build. Visually verify (run `stress -c 8` to drive temp).
- [ ] Commit: `git commit -m "widgets: CpuTemp"`

## Task 3.2: widgets/cpu_freq.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/cpu_freq.rs`

**Notes:** Subscribes to `hub::cpu_freq`. Icon `assets/icons/cpu-freq.svg`. Format: `X.XX GHz` matching rs-bar.

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: CpuFreq"`

## Task 3.3: widgets/memory.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/memory.rs`

**Notes:** Subscribes to `hub::memory`. Icon `assets/icons/memory.svg`. Match rs-bar's display (percent or `used/total`).

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: Memory"`

## Task 3.4: widgets/gpu_busy.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/gpu_busy.rs`

**Notes:** Subscribes to `hub::gpu_busy`. Icon `assets/icons/gpu-busy.svg` or per-vendor (`amd-radeon.svg`, `nvidia-gpu.svg`, `intel-arc-gpu.svg`) — preserve rs-bar's icon-selection logic.

- [ ] Replace. Build. Verify (e.g. `glxgears`).
- [ ] Commit: `git commit -m "widgets: GpuBusy"`

## Task 3.5: widgets/battery_draw.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs` (lines 441..=559, the `BatteryDraw` widget).

**Notes:** Subscribes to `hub::power_draw`, reads `sample.battery_w`. Display format: `±N.NW` or similar (preserve rs-bar's exact format). Color band on draw level. Hide / show "—" when battery is absent.

- [ ] Replace. Build. Verify (run on a laptop).
- [ ] Commit: `git commit -m "widgets: BatteryDraw"`

## Task 3.6: widgets/cpu_draw.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs` (lines 560..=701, the `CpuDraw` widget).

**Notes:** Subscribes to `hub::power_draw`, reads `sample.cpu_w`. Format and color bands per rs-bar.

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: CpuDraw"`

## Task 3.7: widgets/psys_draw.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs` (lines 702..=829, the `PsysDraw` widget).

**Notes:** Reads `sample.psys_w`.

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: PsysDraw"`

## Task 3.8: widgets/gpu_draw.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power_draw.rs` (lines 830..=963, the `GpuDraw` widget).

**Notes:** Reads `sample.gpu_w`. Vendor-specific icon selection (amd-radeon.svg, nvidia-gpu.svg, intel-arc-gpu.svg).

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: GpuDraw"`

## Task 3.9: widgets/workspaces.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/workspaces.rs`

**Notes:** Subscribes to `hub::niri`. Reads `snapshot.workspaces` filtered by the current monitor's connector. The widget needs to know which monitor it's on — pass that via `WidgetInit`? No — `WidgetInit` is shared across all widgets. Instead, the widget can look up its own GdkMonitor at runtime by walking up the widget tree: `root.native()?.surface()?.display()...` — actually simpler: the Bar window's monitor is attached. The widget can query `root.root()` for the window and walk `LayerShell::monitor(&window)`.

The cleanest solution: each `Bar` Component, when launching its child widgets, sets a CSS class on the root with the connector name, and widgets that need the connector parse it from there. OR: add a per-bar global. **Recommended:** introduce a `BarContext` thread-local set by the Bar Component during `init`, holding the current monitor's connector name. The widget reads it once in `init`. Pattern:

```rust
// in bar.rs::init
crate::widgets::BAR_CTX.with(|c| c.set(Some(BarContext { connector: ... })));
// build widgets here, in this thread-local scope
crate::widgets::BAR_CTX.with(|c| c.set(None));
```

Update `widgets/mod.rs` (after Phase 0) to define `BAR_CTX: RefCell<Option<BarContext>>` and a `current_connector()` accessor. Either do this as a small Phase 0.7.5 fix or include the addition in this Workspaces task and document it.

**Decision:** include the `BAR_CTX` addition as Step 1 of this task. It's a one-time micro-change to two files (`widgets/mod.rs` and `bar.rs`); document it explicitly so the agent makes the change and the parallel agents downstream can rely on it.

```rust
// add to widgets/mod.rs
use std::cell::RefCell;

pub struct BarContext {
    pub connector: String,
}

thread_local! {
    pub static BAR_CTX: RefCell<Option<BarContext>> = const { RefCell::new(None) };
}

pub fn current_connector() -> Option<String> {
    BAR_CTX.with(|c| c.borrow().as_ref().map(|x| x.connector.clone()))
}
```

```rust
// in bar.rs::init, around the widget-building section — wrap the
// `init.layout` access (note: layout is already built before bar.rs gets it
// because it comes from config::bar()). So this thread-local needs to be set
// in app.rs around the call to config::bar(). Update app.rs::update for
// EnumerateMonitors:
//
// for m in &current {
//     if !self.bars.contains_key(m) {
//         crate::widgets::BAR_CTX.with(|c| {
//             *c.borrow_mut() = Some(crate::widgets::BarContext {
//                 connector: m.connector().map(|s| s.to_string()).unwrap_or_default(),
//             });
//         });
//         let layout = crate::config::bar();
//         crate::widgets::BAR_CTX.with(|c| *c.borrow_mut() = None);
//         let controller = Bar::builder().launch(BarInit { monitor: m.clone(), layout }).detach();
//         self.bars.insert(m.clone(), controller);
//     }
// }
```

- [ ] Step 1: Add `BAR_CTX` and `current_connector()` to `widgets/mod.rs`.
- [ ] Step 2: Update `app.rs::update` `EnumerateMonitors` arm to set/clear `BAR_CTX` around `config::bar()`.
- [ ] Step 3: Implement `Workspaces` widget. In `init`, capture `current_connector()` once. In `update` (which reacts to `Update(NiriSnapshot)`), filter `snapshot.workspaces` by `ws.output == self.connector`. Render each workspace as a small box with active/inactive state (per rs-bar). Click → niri IPC `Action::FocusWorkspace`.
- [ ] Build. Verify multi-monitor: each bar shows its own monitor's workspaces, switching workspaces highlights the correct box on the correct monitor.
- [ ] Commit: `git commit -m "widgets: Workspaces"`

## Task 3.10: widgets/minimap.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/minimap.rs`

**Notes:** Tiny per-window dots inside a bar showing window positions in the active workspace. Reads `current_connector()` like Workspaces. Reads `snapshot.windows` filtered by output, for the focused workspace.

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: Minimap"`

## Task 3.11: widgets/window_title.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/window_title.rs`

**Notes:** Shows the focused window title for the current monitor's focused workspace. Reads `current_connector()`. Truncate to a max length (preserve rs-bar's truncation length).

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: WindowTitle"`

## Task 3.12: widgets/clock.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/clock.rs`

**Notes:** Two-line display in macbook profile: `HH:MM` and `Mon DD`. Click to open popover with `gtk::Calendar` + seconds-resolution time updating 1Hz **only while open**. Use `glib::timeout_add_local` for the 1Hz timer; cancel when popover is hidden (`Popover::connect_show`/`connect_closed`). Use `chrono::Local::now()` like rs-bar.

The two-line layout is uncommon; use a `gtk::Box` with `Vertical` orientation containing two `gtk::Label`s.

- [ ] Replace. Build. Verify (click opens calendar; calendar dismisses on outside-click).
- [ ] Commit: `git commit -m "widgets: Clock with calendar popover"`

## Task 3.13: widgets/date.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/date.rs`

**Notes:** Simple date label. May not be in any current profile but is exported, so port for completeness.

- [ ] Replace. Build. Verify.
- [ ] Commit: `git commit -m "widgets: Date"`

## Task 3.14: widgets/notch.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/notch.rs`

**Notes:** A 196px-wide empty Box that reserves space for the macbook display's hardware notch in the center zone. The Phase 0 stub is already structurally correct; just set the explicit width. Final implementation:

```rust
// inside view!:
gtk::Box {
    set_orientation: gtk::Orientation::Horizontal,
    set_size_request: (196, -1),
}
```

- [ ] Replace stub with width-set version. Build. Verify on macbook profile.
- [ ] Commit: `git commit -m "widgets: Notch (196px reservation)"`

## Task 3.15: widgets/capslock.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/capslock.rs`

**Notes:** Polls capslock LED state via libc netlink or sysfs. The data source is currently inline in the rs-bar widget — you can either keep it inline (it's small) or make a `hub/capslock.rs`. **Decision:** make a small `hub/capslock.rs` to keep the pattern consistent. Update `hub/mod.rs` to add the module. Icon `assets/icons/capslock.svg`. Show only when active; hide otherwise (`set_visible(false)`).

- [ ] Add `hub/capslock.rs` (port of the data-reading half).
- [ ] Add `pub mod capslock;` to `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify (toggle CapsLock).
- [ ] Commit: `git commit -m "widgets: CapsLock + hub::capslock"`

## Task 3.16: widgets/fcitx.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/fcitx.rs`

**Notes:** Shows current input method state from fcitx5 D-Bus IPC. The data source likely uses `zbus` async. Add `hub/fcitx.rs` if not trivial; otherwise keep inline. Display the current method's name or icon.

- [ ] Add `hub/fcitx.rs` if data fetch is non-trivial.
- [ ] Update `hub/mod.rs` if added.
- [ ] Replace widget stub.
- [ ] Build. Verify (switch input methods).
- [ ] Commit: `git commit -m "widgets: Fcitx"`

## Task 3.17: widgets/pkg_update.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/pkg_update.rs`

**Notes:** Polls package manager (rs-bar uses pacman/checkupdates or similar) for available updates. Long polling interval (15+ minutes — preserve rs-bar's). Two icons: `pkg-uptodate.svg` when 0 updates, `pkg-updates.svg` when N>0. Show count when N>0. Add `hub/pkg_update.rs`.

- [ ] Add `hub/pkg_update.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify.
- [ ] Commit: `git commit -m "widgets: PkgUpdate + hub::pkg_update"`

## Task 3.18: widgets/wireguard.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/wireguard.rs`

**Notes:** Shows WireGuard tunnel state (up/down) for the configured connection name (`config::WIREGUARD_CONNECTION()`). Polls via `wg show` shell or rtnetlink. Add `hub/wireguard.rs`.

- [ ] Add `hub/wireguard.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify (`wg-quick up wg && wg-quick down wg`).
- [ ] Commit: `git commit -m "widgets: Wireguard + hub::wireguard"`

## Task 3.19: widgets/power.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/power.rs`

**Notes:** Click → `std::process::Command::new("sh").arg("-c").arg(config::POWER_COMMAND()).spawn()`. Icon `config::POWER_ICON()`. No subscriber; static.

- [ ] Replace widget stub.
- [ ] Build. Verify (clicking opens the configured logout menu script).
- [ ] Commit: `git commit -m "widgets: Power"`

## Task 3.20: widgets/volume.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/volume.rs`

**Notes:** Volume readout + popover with slider. Data source: `pactl`/PulseAudio or PipeWire IPC. Add `hub/volume.rs` for the percent/mute polling. Popover contains:
- A `gtk::Scale` slider bound to volume %, `connect_value_changed` shells out to `pactl set-sink-volume`.
- Mute toggle button.
- Output device picker (`gtk::DropDown`).

Icon swaps between `volume-high.svg`, `volume-low.svg`, `mute.svg`, `unmute.svg` per state.

- [ ] Add `hub/volume.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify (changing volume in popover changes system volume; system volume change reflects in widget).
- [ ] Commit: `git commit -m "widgets: Volume + hub::volume"`

## Task 3.21: widgets/brightness.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/brightness.rs`

**Notes:** Brightness icon + slider popover. Uses `config::BRIGHTNESS_GET_CMD()`, `BRIGHTNESS_UP_CMD()`, `BRIGHTNESS_DOWN_CMD()`. Icons `brightness-high.svg`, `brightness-low.svg`. Add `hub/brightness.rs` if polling is needed (it likely is).

- [ ] Add `hub/brightness.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify.
- [ ] Commit: `git commit -m "widgets: Brightness + hub::brightness"`

## Task 3.22: widgets/wifi.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/wifi.rs`

**Notes:** Active wifi SSID + signal. Popover with network list, refresh button, click-to-connect. Backend `iwctl` or `nmcli` (whichever rs-bar uses — read source). Add `hub/wifi.rs`.

- [ ] Add `hub/wifi.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify.
- [ ] Commit: `git commit -m "widgets: Wifi + hub::wifi"`

## Task 3.23: widgets/bluetooth.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/bluetooth.rs`

**Notes:** Bluetooth icon (3 states: off, on, connected). Popover with paired/discovered devices + connect/disconnect. `bluetoothctl` backend. Add `hub/bluetooth.rs`.

- [ ] Add `hub/bluetooth.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify.
- [ ] Commit: `git commit -m "widgets: Bluetooth + hub::bluetooth"`

## Task 3.24: widgets/battery.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/battery.rs`

**Notes:** Battery percent + charging indicator. `/sys/class/power_supply/BAT*/`. Add `hub/battery.rs`. Icons `battery.svg`, `battery-charging.svg`. Popover (if rs-bar has one) shows time-to-full / time-to-empty / charge cycles — match rs-bar.

- [ ] Add `hub/battery.rs`.
- [ ] Update `hub/mod.rs`.
- [ ] Replace widget stub.
- [ ] Build. Verify (laptop only).
- [ ] Commit: `git commit -m "widgets: Battery + hub::battery"`

## Task 3.25: widgets/tray.rs

**Source:** `~/Develop/rs-bar/src/gpui_bar/widgets/tray.rs`

**Notes:** Subscribes to `hub::tray`. Renders icons in a horizontal box. On left-click of an icon: trigger `Activate` action. On right-click (or per item config): open a `gtk::PopoverMenu` built from the item's `Vec<TrayMenuEntry>`.

Build menu lazily and rebuild when the entries change. Use `gio::Menu::new()` + `Menu::append_item` to construct from the entries. Wire menu actions to the system-tray crate's IPC for the underlying app.

Optionally an arrow toggle that collapses/expands the tray (rs-bar has `tray-arrow.svg` and `tray-arrow-left.svg`); preserve if present.

- [ ] Replace widget stub.
- [ ] Build. Verify (with at least one tray-publishing app, e.g. NetworkManager applet, KeepassXC, syncthing-tray).
- [ ] Commit: `git commit -m "widgets: Tray"`

---

# Phase 4 — CSS finalization & polish (sequential)

## Task 4.1: Full default-theme.css

**Files:**
- Modify: `assets/default-theme.css`

The Phase 0 CSS is minimal; Phase 4 fills in pixel-perfect styling. Read each widget's GPUI render code (rs-bar) to determine paddings, font sizes, line heights. Common patterns to add:

```css
/* Per-widget color states (carry forward inline rgb() from rs-bar) */

/* Workspaces */
.workspace-active { background-color: @rs_accent; color: @rs_bg; }
.workspace-inactive { color: @rs_fg_dark; }
.workspace-urgent { background-color: @rs_red; color: @rs_bg; }

/* Window title */
.window-title { color: @rs_fg; }

/* CPU temp bands */
.cpu-temp-cool { color: @rs_fg_dark; }
.cpu-temp-warm { color: @rs_orange; }
.cpu-temp-hot  { color: @rs_red; }

/* Battery */
.battery-charging { color: @rs_green; }
.battery-low { color: @rs_orange; }
.battery-crit { color: @rs_red; }

/* ... and so on for each widget that has color states ... */

/* Popovers */
popover.background contents {
    background-color: @rs_bg;
    color: @rs_fg;
    border: 1px solid @rs_border;
    border-radius: 8px;
    padding: 12px;
}

/* gtk::Scale (volume/brightness sliders) */
scale {
    min-width: 160px;
}
scale trough { background: @rs_surface; border-radius: 4px; }
scale highlight { background: @rs_accent; }
scale slider { background: @rs_fg; border-radius: 50%; }

/* gtk::Calendar */
calendar { background: @rs_bg; color: @rs_fg; }
calendar:selected { background: @rs_accent; color: @rs_bg; }

/* Per-bar borders (1px lines) — keep at 1px */
.bar-border-top, .bar-border-bottom {
    background-color: @rs_bg;
    min-height: 1px;
}
```

Walk through every widget after migration is done, in screenshot mode, and add color/spacing rules until visual diff vs rs-bar is minimal.

- [ ] Step 1: Identify all CSS classes used across the widget files (`grep -nh 'add_css_class' src/widgets/*.rs | sort -u`).
- [ ] Step 2: Define each in `default-theme.css` referencing `@rs_*` colors.
- [ ] Step 3: Run both profiles, compare to screenshots of rs-bar (GPUI), iterate.
- [ ] Step 4: Commit. `git commit -m "css: full Nord theming for all widgets"`

## Task 4.2: ~/.config bootstrap verification

The `style::load()` already writes `~/.config/rs-bar/gtk-theme.css` on first run. Verify:

- [ ] Step 1: `rm -rf ~/.config/rs-bar`. Run `cargo run -- --config macbook`. Confirm `~/.config/rs-bar/gtk-theme.css` exists and matches `assets/default-theme.css` byte-for-byte.
- [ ] Step 2: Edit `~/.config/rs-bar/gtk-theme.css` (e.g. change `.bar-capsule { background-color: ... }` to red). Re-run. Confirm change is visible.
- [ ] Step 3: No commit needed unless code changes were required. If `style.rs` needed a tweak, commit: `git commit -m "style: fix user-css bootstrap behavior"`

## Task 4.3: README + finalize

**Files:**
- Modify: `README.md`

- [ ] Step 1: Expand README from skeleton to full project docs (build instructions, profile descriptions, theme override instructions, troubleshooting, link to spec).
- [ ] Step 2: Commit: `git commit -m "docs: complete README"`

---

# Phase 5 — Acceptance verification (sequential)

## Task 5.1: Macbook profile checklist

- [ ] Step 1: `cargo build --release`. No errors. Warnings <= 5.
- [ ] Step 2: `./target/release/rs-bar-relm4 --config macbook`. Verify each of the 23 macbook widgets renders and updates: Workspaces, Minimap, WindowTitle, CpuFreq, CpuUsage, CpuTemp, Memory, Notch, Clock, Wifi, Bluetooth, PkgUpdate, BatteryDraw, CpuDraw, PsysDraw, Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power.
- [ ] Step 3: Click each clickable widget (Clock, Wifi, Bluetooth, Volume, Brightness, Tray icons, Power). Verify popover/action.
- [ ] Step 4: Hot-plug a monitor (if available). Confirm bar appears/disappears.

## Task 5.2: Intel profile checklist

- [ ] Step 1: `./target/release/rs-bar-relm4 --config intel`. Verify each of the 21 intel widgets: Workspaces, Minimap, WindowTitle, CpuFreq, CpuUsage, CpuTemp, Memory, Clock, Wifi, Bluetooth, PkgUpdate, GpuDraw, CpuDraw, GpuBusy, Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power.
- [ ] Step 2: Same popover/action checks as 5.1.

## Task 5.3: Visual-fidelity comparison

- [ ] Step 1: Take screenshots of rs-bar (GPUI) running and rs-bar-relm4 running, both profiles. Side-by-side diff. Record any deviations.
- [ ] Step 2: Open issues for any non-trivial deviations; fix easy ones immediately (CSS adjustments).
- [ ] Step 3: Commit any final tweaks.

## Task 5.4: Tag release

- [ ] Step 1: `git log --oneline | head` — verify clean history.
- [ ] Step 2: `git tag v0.1.0 -m "Initial relm4 port reaches feature parity"`.
- [ ] Step 3: Done.

---

# Operational notes for parallel execution

## Worktrees vs. shared tree

Phase 2 and Phase 3 tasks each touch exactly one file (the corresponding `<name>.rs`). Phase 0 + Phase 1 already established `widgets/mod.rs`, `hub/mod.rs`, both config profiles, and the canonical patterns — none of these are modified by Phase 2/3 tasks. Therefore parallel agents can safely operate on the same git tree.

Exception: Phase 3 widgets that need a new hub module (Task 3.15 capslock, 3.16 fcitx, 3.17 pkg_update, 3.18 wireguard, 3.20 volume, 3.21 brightness, 3.22 wifi, 3.23 bluetooth, 3.24 battery) each add a `pub mod <name>;` line to `hub/mod.rs`. This is the **one** shared edit point. Two strategies:

1. **Pre-add all `pub mod` lines in Phase 0** (preferred): when Phase 0 stubs the hub modules, also stub the per-widget hub modules (capslock, fcitx, pkg_update, wireguard, volume, brightness, wifi, bluetooth, battery). Each gets a stub publishing a default value. This pushes the only shared edit into the bootstrap phase; Phase 3 tasks then never touch `hub/mod.rs`.

2. **Sequentialize the registration**: Phase 3 agents commit only their own `<name>.rs` files; a final integration step appends all the new `pub mod` lines.

**Adopted: option 1.** Update Phase 0, Task 0.6 to stub these additional hub modules. Update the `mod.rs` listing accordingly. Each of the 9 listed Phase 3 tasks then no longer modifies `hub/mod.rs` — they replace `hub/<name>.rs` only.

## Pre-add additional hub stubs (addendum to Task 0.6)

Add to `src/hub/mod.rs`:

```rust
pub mod capslock;
pub mod fcitx;
pub mod pkg_update;
pub mod wireguard;
pub mod volume;
pub mod brightness;
pub mod wifi;
pub mod bluetooth;
pub mod battery;
```

For each, create a stub at `src/hub/<name>.rs`:

```rust
//! STUB. Phase 3 task replaces this.

use std::sync::OnceLock;
use tokio::sync::watch;

#[derive(Clone, Default)]
pub struct State; // replaced by per-widget Phase 3

fn sender() -> &'static watch::Sender<State> {
    static S: OnceLock<watch::Sender<State>> = OnceLock::new();
    S.get_or_init(|| watch::channel(State).0)
}

#[allow(dead_code)]
pub fn subscribe() -> watch::Receiver<State> {
    sender().subscribe()
}
```

The Phase 3 task that owns the module replaces both the `State` shape and the producer thread.

(Apply this addendum during Phase 0 execution.)

## Build cadence

Each task ends with `cargo build` + commit. After every batch of parallel commits, run:

```bash
cd ~/Develop/rs-bar-relm4
git pull --rebase   # if a remote is set
cargo build 2>&1 | tail -20
cargo run -- --config macbook   # spot-check
```

If build is broken, the most recent commits are inspected and fixed; revert is acceptable but rare since each task only touches one file.

---

# Self-review notes

This plan covers:
- Spec section 1 (goal): ✓ Phase 0..5 reach the stated goal.
- Spec section 2 (perf posture): ✓ widget pattern in Task 1.2 includes coalesced updates and CSS class diffing; Task 0.6 stubs lazy popovers; Task 1.2 establishes cached `gdk::Texture`; Task 0.5 single CssProvider.
- Spec section 3 (non-goals): respected — no tests, no behavioral changes, no rs-bar modifications.
- Spec section 4 (repo layout): ✓ Phase 0 builds it exactly.
- Spec section 5 (runtime model): ✓ Task 0.10 + Task 1.2 establish tokio + GLib coexistence and the spawn_local subscription pattern.
- Spec section 6 (layer-shell, multi-monitor): ✓ Tasks 0.8 + 0.9.
- Spec section 7 (widget abstraction + group/capsule + macros): ✓ Task 0.7.
- Spec section 8 (theme + CSS): ✓ Tasks 0.4, 0.5, 4.1, 4.2.
- Spec section 9 (popups): ✓ Tasks 3.12, 3.20, 3.21, 3.22, 3.23, 3.24, 3.25.
- Spec section 10 (system carry-overs): ✓ Phase 2 ports everything verbatim.
- Spec section 11 (config + profiles): ✓ Task 0.3.
- Spec section 12 (Cargo.toml): ✓ Task 0.1.
- Spec section 13 (migration order): plan follows it but with a more granular phase structure.
- Spec section 14 (out of scope): respected.
- Spec section 15 (acceptance): ✓ Phase 5.

Known plan gaps to acknowledge:
- The `BAR_CTX` thread-local introduced in Task 3.9 is the one cross-cutting addition that creates a hidden ordering constraint: Tasks 3.9, 3.10, 3.11 all depend on it being in place. Document this — Task 3.9 must run before 3.10 and 3.11. This is the only ordering constraint inside Phase 3.
- Some hub modules' value types are not fully specified in this plan (e.g., `hub::wifi::State`); the Phase 3 task that owns the widget defines them when porting from rs-bar. Subscribers (only that one widget per hub in those cases) will be updated in the same task.
