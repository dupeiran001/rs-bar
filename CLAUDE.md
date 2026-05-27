# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is rs-bar

A Wayland status bar for the [niri](https://github.com/YaLTeR/niri) compositor, written in Rust on top of [relm4](https://relm4.org/) (GTK4) and [gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell). It renders one layer-shell surface per monitor and supports multi-monitor setups.

The repo contains three sibling backends under `src/`:
- **`relm4_bar/` — the active backend.** All current development happens here.
- `gpui_bar/` — the original Zed-GPUI implementation, kept around for reference.
- `iced_bar/` — a small iced experiment, also dormant.

`vendor/zed/` is the vendored GPUI fork the dormant `gpui_bar/` builds against; the active relm4 backend has no GPUI dependency at runtime.

## Build & Run

```sh
cargo build --release           # release build
cargo build                     # debug build
RS_BAR_LOG=info cargo run       # run with logging
RS_BAR_LOG=debug cargo run      # verbose logging
rs-bar --config macbook         # select config profile (default)
rs-bar --config intel           # intel workstation profile
```

Requires Rust edition 2024 (rustc 1.93.0+). No test suite exists.

## Architecture (relm4 backend)

### Core flow

1. `main.rs`: parse `--config <profile>`, init logging, init the niri IPC listener, then enter the relm4 / GTK4 main loop.
2. `relm4_bar/app.rs` enumerates monitors, loads the embedded CSS via `style.rs`, and launches one `Bar` component per monitor.
3. `relm4_bar/bar.rs` builds the layer-shell window for each monitor — `Layer::Top`, anchored to all three top edges, `KeyboardMode::None` (popovers handle their own keyboard interactivity as separate xdg_popup surfaces). Five widget zones: `left`, `center_left`, `center`, `center_right`, `right`.
4. Widgets are launched as relm4 `SimpleComponent`s and type-erased to a `Widget { name, root, _controller }` handle so the bar layout can hold a heterogeneous `Vec<Widget>`.

### Key abstractions

**`SimpleComponent`** (relm4): every widget is a relm4 component with a `view!` macro defining its bar-line layout, an `init` function that wires popovers + subscriptions, and an `update` function that applies hub state to the held GTK widgets. The model holds the GTK widgets needed for in-place updates (so `update` mutates rather than rebuilds).

**`NamedWidget` trait** (`widgets/mod.rs`): tiny extension on relm4's `Component` providing a `const NAME: &'static str` for tracing and a uniform `Init = WidgetInit { grouped: bool }` payload. Pair with the `widgets!()` and `group!()` macros to compose a bar zone.

**`hub::*`** (`relm4_bar/hub/`): one module per data source. Canonical pattern: a `OnceLock<watch::Sender<T>>` singleton with a single named background `std::thread` lazily spawned on first `subscribe()`. Examples: `hub::cpu_usage`, `hub::niri`, `hub::wifi`, `hub::tray`. Some hubs run a tokio runtime internally (e.g. `hub::tray` because the `system-tray` crate is async); the rest poll sysfs/procfs directly.

**`subscribe_into_msg!`** (`widgets/util.rs`): macro that bridges a hub's `watch::Receiver<T: Clone>` into a component's input messages. Sends the current value immediately, then forwards every `changed()` wake. Replaces the hand-rolled `relm4::spawn_local` block every popover widget used to spell inline.

**`SuppressGuard`** (`widgets/util.rs`): RAII guard that flips a `RefCell<bool>` flag for its lifetime. Wraps the "I'm about to programmatically mutate this slider/switch — don't bounce my own change back through the hub" pattern that several widgets need.

**`BarPopover`** (`widgets/popover.rs`): minimal scaffolding helper around `gtk::Popover` — handles `autohide`, the `*-popover` CSS class, the `popover-animated` animation hook, parent attachment, content child, and click-to-popup wiring. Each panel widget (Volume, Brightness, WiFi, Bluetooth, Tray) builds its content layout inline and hands it to the builder.

**Niri IPC** (`hub/niri.rs`): one background thread opens a niri event-stream socket, maintains workspace/window/output state, and publishes `NiriSnapshot` via `watch`. Used by `Workspaces`, `Minimap`, `WindowTitle`.

### Event bridging pattern

System I/O runs on background threads (std threads, no project-wide tokio runtime). Data crosses to the GTK main thread via tokio `watch` channels:

```
Background thread → watch::Sender::send() → watch::Receiver::changed().await
                  → relm4::spawn_local task on GTK main loop
                  → ComponentSender::input(Msg::Update(state))
                  → SimpleComponent::update mutates held GTK widgets
```

The `subscribe_into_msg!` macro hides the GTK-loop-side half of this pipe.

### Widget grouping

`group!(CpuFreq, CpuUsage, |, CpuTemp)` builds a horizontal box with `.bar-group` styling, separating items where `|` appears. Grouped widgets receive `WidgetInit { grouped: true }` and skip their individual capsule wrapper.

`widgets!(Workspaces, group!(CpuFreq, |, CpuTemp), Memory)` returns a `Vec<Widget>` for a bar zone.

### Config system

`config/mod.rs` holds a global `OnceLock<Config>` initialized from CLI args. Profile modules (`macbook.rs`, `intel.rs`) define theme, font, bar height, widget layout, and external command paths. Convenience functions like `config::THEME()`, `config::BAR_HEIGHT()` read from the global.

### Theming and CSS

CSS lives in two halves:

1. **`assets/default-theme.css`** — bundled at compile time (`include_str!`), holds every static rule. Authors use color tokens defined as `@define-color rs_*` (e.g. `@rs_accent`, `@rs_surface`) and non-color tokens as `@RS_RADIUS_MD`, `@RS_SPACING_LG`, `@RS_ANIM_MED`, `@RS_EASING_SPRING`, etc.
2. **`relm4_bar/theme/`** — Rust-side. `theme/nord.rs` is the active palette (a `Theme` struct of `u32` colors); `theme/tokens.rs` is the design-token table for radii, spacings, typography, animation. `style.rs` assembles the final CSS at startup: `@define-color` block from `Theme` + embedded `default-theme.css` + optional user override at `~/.config/rs-bar/gtk-theme.css`, then `tokens::apply_tokens` substitutes `@RS_*` placeholders with concrete values.

Custom symbolic SVG icons live in `assets/icons/`; `style.rs` registers the path with `gtk::IconTheme` so widgets can use `Image::from_icon_name("foo-symbolic")`.

### GTK4 / layer-shell specifics

- Layer-shell window: `Layer::Top`, anchored to top + left + right, `KeyboardMode::None` on the bar surface itself. GTK popovers become their own xdg_popup subsurfaces and handle their keyboard interactivity independently.
- Custom CSS variables aren't supported by GTK CSS — only `@define-color` is — which is why the non-color tokens are spliced via `tokens::apply_tokens` rather than declared in CSS.
- Animation harness: GTK4 fully supports CSS Animations Level 1 (`@keyframes`, `transform:`, `transition:`). `gtk::Revealer` is the right primitive for staggered child reveals; its `set_reveal_child(true)` triggered via `glib::timeout_add_local_once` gives precise per-child delays (CSS `animation-delay` does not propagate through Revealer).
- `gtk4-layer-shell` works fine with nested popovers (popover spawning a popover), since both are xdg_popup surfaces.

### System data sources

Widgets read directly from sysfs/procfs (`/proc/stat`, `/proc/cpuinfo`, `/proc/meminfo`, `/sys/class/powercap/`, `/sys/class/backlight/`, hwmon, rfkill) using `std::fs` and a small `libc` FFI helper (`hub/sys.rs`: timerfd + epoll). External commands are used where no kernel interface exists: `nmcli` for WiFi list/connect, `bluetoothctl` for BlueZ, `wpctl`/`pactl` for PipeWire, `brightnessctl` for the configured backlight.

## Adding a new widget

1. Create `src/relm4_bar/widgets/my_widget.rs` implementing relm4's `SimpleComponent` and the `NamedWidget` trait
2. If the widget reads shared data, add a `hub::my_data` module with a `OnceLock<watch::Sender<T>>` and `subscribe()`/command functions
3. Use `subscribe_into_msg!(rx, sender, MyMsg::Update)` to bridge the hub into the component's input messages
4. Use `BarPopover::builder(&root, "my-popover").build(&content_box)` if it has a popover; `attach_click(&root)` for the standard left-click-opens behaviour
5. Use `SuppressGuard::new(&self.flag)` in `update()` if you mutate widgets in ways that would re-fire signals
6. Register the module in `src/relm4_bar/widgets/mod.rs` (both `mod my_widget;` and `pub use my_widget::MyWidget;`)
7. Place the widget into a config profile's `bar()` function using `widgets!()` or `group!()`

## Adding a new config profile

1. Create `src/relm4_bar/config/my_profile.rs` with a `config() -> Config` and `bar(...) -> Bar`
2. Add `mod my_profile;` in `src/relm4_bar/config/mod.rs`
3. Add the profile name to `PROFILES` and the match arms in `init()` and `bar()`
