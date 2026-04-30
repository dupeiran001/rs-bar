# rs-bar-relm4 — Design Spec

**Date:** 2026-04-30
**Status:** Approved, pending implementation plan
**Source project:** `~/Develop/rs-bar` (GPUI-based)
**Target project:** `~/Develop/rs-bar-relm4` (relm4 / GTK4 / gtk4-layer-shell)

## 1. Goal

Port rs-bar — a Wayland status bar for the niri compositor — from GPUI to relm4 (GTK4-based) as a fully self-contained sibling project at `~/Develop/rs-bar-relm4`. The new project must:

- Reproduce all 23 widgets with bit-identical behavior and visually-identical styling.
- Reproduce the same two compile-time profiles (`--config macbook`, `--config intel`) selected at runtime.
- Reproduce the multi-monitor / layer-shell behavior (one bar per `GdkMonitor`, exclusive zone reserved at top edge).
- Reproduce all popups (Clock, Wifi, Bluetooth, Volume, Brightness, Power, Tray menus) with the same trigger semantics.
- Reproduce all command fallbacks (nvidia-smi for GPU power, brightnessctl, etc.) and all sysfs/procfs/libc readers.

The new project depends only on stable upstream crates — no vendored framework fork.

## 2. Performance posture (secondary goal)

Where it can be done **without changing observable behavior or complicating the verbatim port of system-reading code**, prefer the more efficient option. This is a soft goal: anywhere a perf win conflicts with the "behavior-identical port" rule, the rule wins.

Concrete optimizations that fit this posture and should be applied:

- **Coalesce updates at the value level.** Like the current code does for the clock and CPU usage, only emit a `sender.input(Update(v))` message when the *displayed* value would change (rounded percent, formatted minute string, etc.). Free; carry forward.
- **`borrow_and_update()` over clone.** Read `watch::Receiver` values without cloning where the type is `Copy`. No behavior change.
- **Lazy popover construction.** Build `gtk::Popover` content on first open, not at widget init. Avoids paying for popover widget trees that may never be opened (most users never click the clock).
- **Cached `gdk::Texture` for SVG icons.** Each unique icon path is parsed once at startup; widgets share `gdk::Texture` handles across all bar instances. Avoids re-parsing SVG per repaint and per monitor.
- **Single shared `gtk::CssProvider`.** Registered once on `gdk::Display::default()`, not per-window — applies to every bar automatically.
- **Skip CSS class toggles when state hasn't changed.** Track the currently-applied class in the widget model; only call `add_css_class` / `remove_css_class` on transition. GTK emits property-change signals even for no-op class changes.
- **Single-thread invariant for shared data sources.** Built into the hub design (one `OnceLock` poller thread per data source regardless of bar count). Documented here as a property to *preserve* during the port, not a new optimization.
- **Dirty-tracking via the `tracker` crate** (relm4-native, optional). For widgets with several display fields (e.g. battery shows percent + icon + charging state), use `#[tracker::track]` so only the changed field's GTK call fires on each update.

Out of scope as performance work:
- Polling cadence changes (CPU/memory/etc. stay at their current intervals).
- Fallback-command reordering (e.g. trying nvidia-smi before /sys readings).
- Subprocess pooling, IPC connection pooling.
- Anything that requires changing the data shape published over the watch channel.

## 3. Non-goals

- No new widgets, no new theme, no behavioral changes.
- No interim refactors of the system-reading code; carry it over verbatim.
- No coexistence with the GPUI implementation in the same repo. The two projects live in separate trees.

## 4. Repository layout

```
rs-bar-relm4/
├── Cargo.toml
├── README.md
├── assets/
│   ├── icons/                    ← copied from rs-bar/assets/icons/
│   └── default-theme.css         ← bundled default, embedded with include_str!
├── docs/superpowers/specs/
│   └── 2026-04-30-relm4-migration-design.md   (this file)
└── src/
    ├── main.rs                   ← arg parse, tokio rt boot, RelmApp::run
    ├── app.rs                    ← root Component: opens N bar windows
    ├── bar.rs                    ← Bar Component (one per monitor)
    ├── style.rs                  ← CSS load: embedded default + user override
    ├── theme/
    │   ├── mod.rs                ← Theme struct (same fields as rs-bar)
    │   └── nord.rs               ← Nord palette
    ├── config/
    │   ├── mod.rs                ← Config struct, OnceLock, --config parsing
    │   ├── macbook.rs            ← Macbook profile (widgets + tunables)
    │   └── intel.rs              ← Intel profile
    ├── hub/
    │   ├── mod.rs                ← re-exports
    │   ├── cpu_usage.rs          ← /proc/stat poller, watch::Sender<f32>
    │   ├── cpu_temp.rs
    │   ├── cpu_freq.rs
    │   ├── memory.rs
    │   ├── power_draw.rs
    │   ├── gpu_busy.rs
    │   ├── battery.rs            (if shared)
    │   ├── niri.rs               ← niri event listener, watch::Sender<NiriSnapshot>
    │   └── tray.rs               ← StatusNotifier listener, watch::Sender<TrayState>
    └── widgets/
        ├── mod.rs                ← BarWidget trait, Group, capsule helpers, widgets!/group! macros
        ├── battery.rs
        ├── battery_draw.rs       ← split out of rs-bar's power_draw.rs
        ├── bluetooth.rs
        ├── brightness.rs
        ├── capslock.rs
        ├── clock.rs
        ├── cpu_draw.rs           ← split out of rs-bar's power_draw.rs
        ├── cpu_freq.rs
        ├── cpu_temp.rs
        ├── cpu_usage.rs
        ├── date.rs
        ├── fcitx.rs
        ├── gpu_busy.rs
        ├── gpu_draw.rs           ← split out of rs-bar's power_draw.rs
        ├── memory.rs
        ├── minimap.rs
        ├── notch.rs
        ├── pkg_update.rs
        ├── power.rs
        ├── psys_draw.rs          ← split out of rs-bar's power_draw.rs
        ├── tray.rs
        ├── volume.rs
        ├── wifi.rs
        ├── window_title.rs
        ├── wireguard.rs
        └── workspaces.rs
```

**One widget per file.** The current rs-bar mixes (a) the system-reading thread and the GPUI render in the same widget file, and (b) packs four widgets — `BatteryDraw`, `CpuDraw`, `GpuDraw`, `PsysDraw` — into a single `power_draw.rs`. rs-bar-relm4 splits both:

- Each widget gets its own file under `widgets/`. `power_draw.rs` (963 lines, four widgets) becomes four separate widget files: `battery_draw.rs`, `cpu_draw.rs`, `gpu_draw.rs`, `psys_draw.rs`. Each contains exactly one widget Component.
- Shared system-reading infrastructure that those four widgets currently rely on (the energy-uJ poller, common helpers — roughly the first 440 lines of the existing `power_draw.rs`) moves to `hub/power_draw.rs` and is consumed by the four widget files via watch subscriptions. If the hub side itself also becomes large enough to warrant splitting (e.g. `hub/cpu_power.rs` vs `hub/gpu_power.rs`), that's allowed; but the hub/widgets split is the primary one.

Net effect: pure-Rust readers in `hub/`, exactly one widget Component per `widgets/<name>.rs`, no exceptions.

## 5. Runtime model

### 5.1 Tokio + GLib coexistence

`tokio::sync::watch` itself does **not** require a tokio runtime — it's a self-contained channel built on `parking_lot`/`std`. However, the `system-tray` crate transitively pulls in `zbus` which spawns its own tasks expecting a tokio reactor. The same is true for some widgets that may shell out via `tokio::process` (kept for parity with the current code's approach where applicable). To keep behavior identical we boot a tokio runtime up front:

```rust
// src/main.rs
fn main() {
    let env = env_logger::Env::new().filter("RS_BAR_LOG").write_style("RS_BAR");
    env_logger::init_from_env(env);

    install_panic_hook();           // suppress zbus "no reactor running"
    let args = cli::parse();        // --config <profile>
    config::init(args.profile);

    // Tokio runtime entered for the app's lifetime — needed by zbus
    // (transitively, via system-tray) and any tokio::process callers.
    // The hub poller threads themselves use plain std::thread.
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

Inside relm4 components, the subscription loop runs on the GTK main loop via `relm4::spawn_local`:

```rust
let mut rx = hub::cpu_usage::subscribe();
let s = sender.clone();
relm4::spawn_local(async move {
    while rx.changed().await.is_ok() {
        let value = *rx.borrow_and_update();
        s.input(CpuUsageMsg::Update(value));
    }
});
```

`relm4::spawn_local` delegates to `glib::MainContext::spawn_local`, so the future runs on the GTK main thread. `watch::Receiver::changed()` is a pure `tokio::sync` primitive that does not require a runtime to drive the future — it works on the GLib main loop unchanged.

### 5.2 Hub pattern

Each shared data source has a `hub/<name>.rs` module exposing two functions:

```rust
// hub/cpu_usage.rs
fn sender() -> &'static watch::Sender<f32> {
    static S: OnceLock<watch::Sender<f32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0.0);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("cpu-usage".into())
            .spawn(move || cpu_monitor(producer))
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<f32> {
    sender().subscribe()
}

fn cpu_monitor(tx: watch::Sender<f32>) {
    // /proc/stat reader with timerfd+epoll — copied verbatim from
    // rs-bar/src/gpui_bar/widgets/cpu_usage.rs
    // Replace `bc.publish(usage)` with `let _ = tx.send(usage);`
}
```

First call to `subscribe()` lazily spawns the OS thread. Multi-monitor setups still get exactly one poller thread per data source — `tx.subscribe()` returns a fresh `Receiver` cheaply.

### 5.3 Latest-value semantics

`tokio::sync::watch` matches the `Broadcast<T>` semantics this project relied on:
- Fast subscribers see every value.
- Slow subscribers see the most recent value, intermediate values may coalesce.
- `Receiver::borrow_and_update()` returns the current value without cloning where possible.

This is a behavior preservation, not a change.

## 6. Layer-shell, multi-monitor, hot-plug

Use `gtk4-layer-shell-rs` — upstream, mature, mirrors GPUI's `LayerShellOptions` closely.

```rust
// bar.rs — inside Component::init
let window = gtk::ApplicationWindow::new(&app);
window.init_layer_shell();
window.set_layer(Layer::Top);
window.set_anchor(Edge::Top, true);
window.set_anchor(Edge::Left, true);
window.set_anchor(Edge::Right, true);
window.set_exclusive_zone(config::BAR_HEIGHT() as i32);
window.set_keyboard_mode(KeyboardMode::None);
window.set_namespace("rs-bar");
window.set_monitor(Some(&monitor));
window.set_default_size(1, config::BAR_HEIGHT() as i32);
```

### Multi-monitor & hot-plug

- On startup, the root `App` component enumerates `gdk::Display::default().monitors()` (a `gio::ListModel`) and opens one `Bar` Component per `GdkMonitor`.
- Subscribe to the `items-changed` signal on the monitors list model. On change:
  - Added monitors → open a new `Bar` window.
  - Removed monitors → close the corresponding `Bar`.
- Maintain a `HashMap<GdkMonitor, Controller<Bar>>` in the App component. (`GdkMonitor` is reference-equal across signal callbacks.)
- Initial enumeration happens after a brief delay (100 ms timer fired off in `App::init`), matching the current GPUI workaround for "displays not yet known at app start". This is a defensive carry-over; the `items-changed` subscription handles it cleanly even if the initial list is empty.

## 7. Widget abstraction

Each widget is a relm4 `Component` (or `SimpleComponent` where messages are trivial). Each widget chooses its own root type (`gtk::Box`, `gtk::Button`, etc. — whatever fits best). The top-level `widgets/mod.rs` defines a wrapper that erases the controller's concrete type and exposes only the upcast `gtk::Widget`:

```rust
pub struct WidgetInit {
    pub grouped: bool,
}

pub struct Widget {
    pub name: &'static str,
    pub root: gtk::Widget,                 // upcast from the controller's actual root
    _controller: Box<dyn std::any::Any>,   // keeps the Controller<C> alive
}

pub fn build<C>(grouped: bool) -> Widget
where
    C: Component<Init = WidgetInit> + 'static,
    C::Root: glib::IsA<gtk::Widget> + Clone,
{
    let controller = C::builder().launch(WidgetInit { grouped }).detach();
    let root: gtk::Widget = controller.widget().clone().upcast();
    Widget { name: C::NAME, root, _controller: Box::new(controller) }
}
```

(`C::NAME` is a manual associated `const` declared on each widget type, since it isn't part of the relm4 `Component` trait.) The upcast trait bound `C::Root: IsA<gtk::Widget>` works for every concrete GTK widget type — `gtk::Box`, `gtk::Button`, `gtk::Label`, etc. all implement it.

The bar holds five `Vec<Widget>` zones (`left`, `center_left`, `center`, `center_right`, `right`) — same shape as today.

### 7.1 Group + capsule

The `group!()` and `widgets!()` macros stay. They emit `Vec<Widget>` constructed via `build::<T>(grouped)`. A `Group` is itself a Component whose root is a `gtk::Box` with `.bar-group` CSS class, containing child widget roots and `gtk::Separator` between them where `|` was used.

A widget renders its content into a `gtk::Box`. The macro passes `grouped: true` to children of `group!()`; the widget's `init` adds `.bar-capsule` class only if `!grouped`. CSS rules in `assets/default-theme.css` style both classes identically (rounded pill, surface bg, border) — the difference is only that grouped children skip their own capsule wrapper.

### 7.2 Macros

```rust
// In widgets/mod.rs
macro_rules! widgets {
    ($($items:tt)*) => { /* same shape as gpui_bar — emit Vec<Widget> */ };
}

macro_rules! group {
    ($($item:tt),* $(,)?) => { /* emits a Widget whose root is a Group */ };
}
```

Macros do not need `$cx` — relm4 components are launched globally without an `App` context being passed in. This simplifies the macro vs. the GPUI version.

## 8. Theme & styling

### 8.1 Theme struct

`Theme` has the same fields as today (`bg`, `fg`, `accent`, `green`, etc., as `u32` colors). The Nord palette is identical.

### 8.2 CSS pipeline

`src/style.rs`:

1. Generate `@define-color` directives from the active `Theme` struct:
   ```css
   @define-color rs_bg #2E3440;
   @define-color rs_fg #ECEFF4;
   @define-color rs_accent #88C0D0;
   /* … all Theme fields … */
   ```
2. Append the embedded default CSS: `include_str!("../assets/default-theme.css")` — references the colors as `@rs_bg` etc.
3. On first run, if `~/.config/rs-bar/gtk-theme.css` does not exist, copy the embedded default to that path.
4. If `~/.config/rs-bar/gtk-theme.css` exists, append it after the embedded default. User rules win on equal specificity.
5. Load the concatenated CSS into a single `gtk::CssProvider` registered at `STYLE_PROVIDER_PRIORITY_APPLICATION` on the default `gdk::Display`.

This means:
- Out of the box, the bar looks identical to the GPUI version.
- A user can `cp ~/.config/rs-bar/gtk-theme.css ...` and edit it freely (the same file rs-bar-relm4 first wrote on startup).
- Swapping the Rust `Theme` struct (e.g. to a future non-Nord palette) re-keys all the `@rs_*` colors without touching CSS.

### 8.3 default-theme.css contents (sketch)

Covers:
- Bar root (`window.rs-bar`): bg, fg, font-family, font-weight, font-size.
- Top/bottom 1px borders (drawn as inset box-shadow on the root, since GTK Window has limited border control).
- `.bar-capsule`: rounded pill, surface bg, 1px border, padding.
- `.bar-group`: same as capsule.
- `.bar-group separator`: 1px wide vertical separator, `@rs_fg_gutter`.
- Per-widget classes for color overrides (`.cpu-usage-high`, `.cpu-usage-warn`, `.battery-low`, etc.) — GPUI used inline `rgb()`; we move that to classes toggled in `update()`.

Total ~300-500 lines of CSS, generated from inspecting each widget's current rendering.

## 9. Popups

All popups use `gtk::Popover` anchored to the originating widget. Pattern:

```rust
struct Clock {
    button: gtk::Button,
    popover: gtk::Popover,   // built once in init, hidden by default
    /* ... */
}

// init
let popover = gtk::Popover::builder().autohide(true).build();
popover.set_parent(&button);
button.connect_clicked(clone!(popover => move |_| popover.popup()));
```

Per-widget popup contents:

| Widget | Popup contents |
| --- | --- |
| Clock | Calendar (`gtk::Calendar`) + seconds-resolution time, updates 1Hz while open |
| Wifi | Network list, click to connect, refresh button |
| Bluetooth | Device list, click to (dis)connect, refresh button |
| Volume | `gtk::Scale` slider, mute toggle, output device picker |
| Brightness | `gtk::Scale` slider |
| Tray | Per-icon `gtk::PopoverMenu` from the StatusNotifier `MenuModel` |
| Power | Either popover menu OR shell out to `~/.config/waybar/scripts/logout-menu.sh` (current behavior is the script — keep that exactly) |

Auto-dismiss on outside click is built into `Popover::set_autohide(true)`.

## 10. System data carry-overs

These are copied verbatim — only the publish call changes from `Broadcast::publish` to `watch::Sender::send`.

- `niri.rs` listener thread (event-stream socket, `WorkspacesState`/`WindowsState` apply, overview tracking).
- `system-tray` StatusNotifier listener.
- `/proc/stat` reader (timerfd + epoll) for CPU usage.
- `/proc/cpuinfo` and `/sys/class/hwmon/*/temp*_input` for CPU freq / temp.
- `/proc/meminfo` for memory.
- `/sys/class/powercap/intel-rapl:*/energy_uj` for CPU power (PsysDraw, CpuDraw).
- AMDGPU `/sys/class/drm/card*/device/gpu_busy_percent` for GpuBusy.
- nvidia-smi shell-out fallback for GPU power.
- `/sys/class/power_supply/BAT*/` for battery.
- `brightnessctl` for brightness get/up/down (commands configured per profile).
- `wpa_cli`/`iwctl` for wifi (whichever the existing code uses).
- `bluetoothctl` for bluetooth.
- libc netlink socket for capslock state (if present in current code).
- D-Bus / fcitx5 IPC for input method.
- Shell-out to package manager for pkg_update.

All of these are pure Rust + libc + shell-out; none touch GPUI today. They survive the migration unchanged.

## 11. Configuration

### 11.1 Profiles

`src/config/{macbook,intel}.rs` — both compiled in. Runtime selection via `--config <name>`, default `macbook`. Same as today. No cargo features.

### 11.2 Config struct

Same fields as today:

```rust
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
```

Stored in a `OnceLock<Config>`, accessed via `config::THEME()`, `config::BAR_HEIGHT()`, etc. Identical to current API so widget code reads the same.

### 11.3 Bar layout

`src/config/macbook.rs::bar()` and `intel.rs::bar()` return a `Bar` struct (5 zones × `Vec<Widget>`) using the same `widgets!()` / `group!()` macro syntax.

## 12. Cargo.toml

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

tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros"] }

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

No `[patch]` section — gtk4-layer-shell-rs is a regular upstream crate. Versions are pinned at known-good minor versions; lockfile commits the exact patch versions.

## 13. Migration order (high level — full plan to be produced by writing-plans skill)

1. Skeleton: Cargo.toml, src/main.rs, app.rs (empty bar), bar.rs, style.rs, theme/, config/. Bar opens with empty zones, layer-shell working, CSS loaded. Verify on both monitors, both profiles.
2. Widget infrastructure: widgets/mod.rs, BarWidget convention, widgets!()/group!() macros, capsule CSS. Add a stub `Notch` widget to confirm wiring.
3. Hub infrastructure: hub/mod.rs, port `Broadcast<T>` → `watch::Sender<T>` pattern in hub/cpu_usage.rs. Add `CpuUsage` widget. Verify subscription, latest-value semantics, single thread despite N bars.
4. Niri hub + niri-aware widgets: hub/niri.rs (verbatim port), then `Workspaces`, `Minimap`, `WindowTitle`.
5. Polling-only widgets (no popup): `CpuTemp`, `CpuFreq`, `Memory`, `GpuBusy`, then the four power-draw widgets (`BatteryDraw`, `CpuDraw`, `PsysDraw`, `GpuDraw`) — each in its own file, sharing `hub/power_draw.rs`.
6. Trivial widgets: `Date`, `Clock` (with calendar popup), `CapsLock`, `Fcitx`, `PkgUpdate`, `Wireguard`, `Power`.
7. Interactive widgets with popups: `Volume`, `Brightness`, `Wifi`, `Bluetooth`, `Battery`.
8. Tray: hub/tray.rs (StatusNotifier), `Tray` widget with per-icon menus.
9. Polish: top/bottom border overlays, exact paddings, font, icon paths, panic hook, ~/.config bootstrap, README.

Each step gets independent visual verification on both `--config macbook` and `--config intel` profiles before moving on.

## 14. Out of scope / explicit non-decisions

- The existing rs-bar repo is **not modified**. The two projects coexist on disk but are independent.
- No tests (rs-bar has none today; we don't introduce a test suite as part of the migration).
- No feature additions, no theme changes, no layout changes.
- The GPUI version remains the canonical reference for visual fidelity comparisons during development.

## 15. Acceptance criteria

- `cargo build --release` succeeds on a clean checkout.
- `rs-bar-relm4 --config macbook` opens one bar per monitor, all 23 macbook-profile widgets visible and live (Workspaces, Minimap, WindowTitle, CpuFreq, CpuUsage, CpuTemp, Memory, Notch, Clock, Wifi, Bluetooth, PkgUpdate, BatteryDraw, CpuDraw, PsysDraw, Wireguard, Battery, Volume, Brightness, Tray, Fcitx, CapsLock, Power).
- `rs-bar-relm4 --config intel` opens one bar per monitor, all 21 intel-profile widgets visible and live (Workspaces, Minimap, WindowTitle, CpuFreq, CpuUsage, CpuTemp, Memory, Clock, Wifi, Bluetooth, PkgUpdate, GpuDraw, CpuDraw, GpuBusy, Wireguard, Volume, Brightness, Tray, Fcitx, CapsLock, Power).
- All popups open and dismiss correctly.
- Multi-monitor hot-plug: connecting/disconnecting a display adds/removes a bar without restart.
- A diff'd screenshot against rs-bar (GPUI) shows visually-equivalent rendering modulo subpixel font rasterization differences.
- `~/.config/rs-bar/gtk-theme.css` is created on first run; editing it changes appearance without rebuild.
