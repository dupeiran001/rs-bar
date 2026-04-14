# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is rs-bar

A Wayland status bar for the [niri](https://github.com/YaLTeR/niri) compositor, built with Rust and [GPUI](https://github.com/zed-industries/zed) (the GPU-accelerated UI framework from Zed). It renders on a Wayland layer-shell surface and supports multi-monitor setups.

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

## Architecture

### Core flow

1. `main.rs`: Parse `--config <profile>`, init logging, start niri IPC listener, open one bar window per display via GPUI layer-shell
2. Each bar window contains a `Bar` struct with five widget zones: `left`, `center_left`, `center`, `center_right`, `right`
3. Widgets are type-erased to `AnyView` via the `Widget` wrapper and stored in `Vec<Widget>`

### Key abstractions

**`BarWidget` trait** (`src/widgets/mod.rs`): All widgets implement this. Provides `new()`, `render()`, and optional `set_grouped()`. The `impl_render!` macro generates the GPUI `Render` impl, bridging `BarWidget::render` → `Render::render` (needed because the orphan rule prevents implementing GPUI's `Render` directly in some contexts).

**`Broadcast<T>` / `Subscription<T>`** (`src/hub.rs`): Generic pub-sub hub with latest-value semantics. A single background thread per data source (CPU, memory, niri events, etc.) publishes to a `Broadcast`, and all widget instances across all monitors subscribe. Coalesces redundant wakes — subscribers always get the most recent value.

**Niri IPC** (`src/niri.rs`): A single background thread opens one niri event-stream socket, maintains workspace/window/output state, and publishes `NiriSnapshot` via `Broadcast`. A GPUI global `NiriState` tracks overview state for widgets that need it.

### Event bridging pattern

System I/O runs on background threads (std threads, NOT tokio). Data flows to GPUI's foreground executor via `async_channel`:

```
Background thread → Broadcast::publish() → async_channel → cx.spawn(async) → this.update() → cx.notify()
```

Widgets spawn a foreground task in `new()` that awaits `Subscription::next()` in a loop and calls `cx.notify()` on each update.

### Widget grouping

`group!(cx, CpuUsage, |, CpuTemp)` creates multiple widgets in a shared capsule. The `|` token inserts a visual separator. Grouped widgets have `set_grouped()` called so they skip their individual capsule styling.

`widgets!(cx, Clock, group!(cx, A, |, B))` builds a `Vec<Widget>` for a bar zone.

### Config system

`src/config/mod.rs` holds a global `OnceLock<Config>` initialized from CLI args. Profile modules (`macbook.rs`, `intel.rs`) define theme, font, bar height, widget layout, and external command paths. Convenience functions like `config::THEME()`, `config::BAR_HEIGHT()` read from the global.

### GPUI specifics

- Uses Wayland layer-shell API (`LayerShellOptions`) — not a regular window
- GPUI is vendored in `vendor/zed/crates/` and patched via `[patch]` in Cargo.toml
- GPUI's executor is NOT tokio — use `cx.spawn()` for foreground async, `cx.background_executor()` for background work
- Tailwind-like styling: `.flex()`, `.bg()`, `.text_xs()`, `.gap_2()`, `.rounded()`, etc.
- `on_hover(false)` doesn't fire when pointer leaves the Wayland surface (known GPUI limitation)
- A custom panic hook suppresses GPUI's internal "no reactor running" zbus/tokio panics

### System data sources

Widgets read directly from sysfs/procfs (`/proc/stat`, `/proc/cpuinfo`, `/proc/meminfo`, `/sys/class/powercap/`, hwmon) using raw `std::fs` reads and `libc` FFI (timerfd, epoll, netlink sockets). External commands are used only as fallbacks (e.g., `nvidia-smi` for GPU power).

## Adding a new widget

1. Create `src/widgets/my_widget.rs` implementing `BarWidget`
2. Call `impl_render!(MyWidget);` at the bottom of the file
3. Add `mod my_widget;` and `pub use my_widget::MyWidget;` in `src/widgets/mod.rs`
4. Add the widget to a config profile's `bar()` function using `widgets!()` or `group!()`
5. If the widget needs shared polling data, create a `Broadcast` singleton (see existing widgets for the pattern)

## Adding a new config profile

1. Create `src/config/my_profile.rs` with a `config() -> Config` and `bar(cx: &mut App) -> Bar`
2. Add `mod my_profile;` in `src/config/mod.rs`
3. Add the profile name to `PROFILES` and the match arms in `init()` and `bar()`
