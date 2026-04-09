# rs-bar

A Wayland status bar built with Rust + [GPUI](https://github.com/zed-industries/zed), designed for the [niri](https://github.com/YaLTeR/niri) compositor.

## Widgets

### Left Section

| Widget | Description |
|--------|-------------|
| **Workspaces** | Niri workspace switcher. Capsule-shaped container with rounded buttons per workspace. Click to switch, active workspace highlighted with accent color + shadow. Filtered per-output via UUID matching. Event-driven via niri IPC socket. |
| **Minimap** | Tiled window layout visualization. Shows a miniature representation of the current workspace layout. |
| **WindowTitle** | Focused window title + application icon. Resolves icons via freedesktop icon theme (configurable). Truncates long titles. Event-driven via niri IPC. |

### Center-Left Section

| Widget | Description |
|--------|-------------|
| **CpuFreq** | Current CPU frequency in GHz. Reads from `/proc/cpuinfo`. Polls via timerfd + epoll (2s interval). |
| **CpuUsage** | CPU usage percentage. Reads from `/proc/stat`. Polls via timerfd + epoll (2s interval). Color-coded: green (low), yellow (moderate), red (high). |
| **CpuTemp** | CPU temperature in Celsius. Reads from hwmon sysfs. Detects coretemp/k10temp/macsmc automatically. Color-coded by temperature threshold. |
| **Memory** | RAM usage percentage + used/total. Reads from `/proc/meminfo`. Polls via timerfd + epoll (2s interval). |
| **GpuBusy** | GPU utilization icon. Reads from sysfs (`gpu_busy_percent`). Shows icon only when GPU is active. |

### Center Section

| Widget | Description |
|--------|-------------|
| **Clock** | Two-line time + date display (HH:MM on top, Mon DD on bottom). Capsule-styled. Updates every second. Click for popup with full time and date. |
| **Notch** | Decorative notch element for MacBook-style layouts. |

### Center-Right Section

| Widget | Description |
|--------|-------------|
| **Wifi** | Wi-Fi status indicator. Reads state from sysfs/procfs/NL80211 (zero subprocesses). Monitors link changes via netlink RTMGRP_LINK socket, polls signal via timerfd (5s). Icon and color reflect signal strength: excellent/good/fair/weak/off. |
| **Bluetooth** | Bluetooth state indicator. Reads initial power state from rfkill sysfs, then monitors via a long-lived `bluetoothctl` process with epoll on stdout. Three states: off (dim), on (normal), connected (blue). |
| **PkgUpdate** | Package update indicator. Auto-detects distro package manager (Arch pacman/yay/paru, Debian apt, Fedora dnf) + Flatpak. Polls every 10 minutes. Green icon when updates available, dim when up-to-date. |

### Right Section

| Widget | Description |
|--------|-------------|
| **Wireguard** | WireGuard VPN toggle. Click to connect/disconnect via nmcli. Monitors state via `nmcli monitor`. Green when active, dim when off. |
| **Battery** | Battery level indicator with expandable hover details. Shows battery icon colored by charge level (green >= 60%, yellow >= 30%, orange >= 15%, red < 15%). On hover, smoothly expands to show: charge bar, percentage, real-time power draw (W), estimated time remaining, battery health (%), and design capacity (Wh). Reads all data from sysfs (zero subprocesses). Polls every 2s via timerfd + epoll. |
| **Volume** | PulseAudio/PipeWire volume control. Circular icon when collapsed, expands on hover to show volume bar + percentage. Scroll to adjust volume (5% steps), click to toggle mute. Monitors via `pactl subscribe` for real-time updates. |
| **Brightness** | Screen brightness control. Circular icon when collapsed, expands on hover to show brightness bar + percentage. Scroll to adjust. Configurable backend commands. Polls every 2s. |
| **Tray** | System tray (SNI protocol). Circular icon when collapsed, expands on hover to show tray item icons. Supports StatusNotifierItem/Watcher D-Bus protocol with pixmap and icon-name resolution. |
| **Fcitx** | Input method indicator for fcitx5. Shows current input method icon. Monitors via D-Bus signals. |
| **CapsLock** | CapsLock state indicator. Event-driven via evdev + epoll on `/dev/input/event*`. Shows icon only when CapsLock is active. Requires `input` group membership. |
| **Power** | Power/logout button. Teal circle with shadow. Click executes configurable power command. |

### Power Draw Widgets (Groupable)

These widgets show real-time power consumption and are designed to be composed via `group!()`:

| Widget | Description |
|--------|-------------|
| **BatteryDraw** | Battery discharge/charge watts. Reads from sysfs `power_now` or `current_now * voltage_now`. |
| **CpuDraw** | CPU package watts. Intel/AMD via RAPL energy counters (delta-based), Apple via macsmc hwmon. |
| **PsysDraw** | Platform/system total watts. RAPL psys domain or macsmc "Total System Power". |
| **GpuDraw** | Discrete GPU watts. hwmon sysfs (AMD/Intel) or nvidia-smi fallback. |

## Widget Grouping

Widgets can be grouped into a shared capsule with `|` separators:

```rust
group!(cx, CpuUsage, |, CpuTemp)       // [CPU% │ 45°C]
group!(cx, BatteryDraw, |, CpuDraw)     // [12.5W │ 8.2W]
```

Grouped widgets share a single rounded container and skip their individual capsule styling.

## Prerequisites

### Quick setup

Run this to apply all permissions at once (log out and back in afterwards):

```sh
# Group memberships
sudo usermod -aG input $USER    # CapsLock widget: evdev access
sudo usermod -aG video $USER    # Brightness widget: backlight write access

# RAPL powercap tmpfiles rule (CpuDraw/PsysDraw, persists across reboots)
sudo tee /etc/tmpfiles.d/rapl-powercap.conf <<'EOF'
z /sys/devices/virtual/powercap/intel-rapl/intel-rapl:*/energy_uj 0444 root root -
z /sys/devices/virtual/powercap/intel-rapl/intel-rapl:*:*/energy_uj 0444 root root -
z /sys/devices/virtual/powercap/intel-rapl/intel-rapl:*/max_energy_range_uj 0444 root root -
z /sys/devices/virtual/powercap/intel-rapl/intel-rapl:*:*/max_energy_range_uj 0444 root root -
EOF
sudo systemd-tmpfiles --create /etc/tmpfiles.d/rapl-powercap.conf
```

### Runtime dependencies

Widgets spawn these external tools. Install whichever you need:

| Widget | Package (Arch) | Command used |
|--------|---------------|--------------|
| Volume | `wireplumber`, `libpulse` | `wpctl`, `pactl` |
| Bluetooth | `bluez-utils` | `bluetoothctl` |
| Wireguard | `networkmanager` | `nmcli` |
| Brightness | `brightnessctl` | `brightnessctl` |
| Fcitx | `fcitx5` | `fcitx5-remote` |
| PkgUpdate | `pacman-contrib` | `checkupdates` |
| GpuDraw (nvidia) | `nvidia-utils` | `nvidia-smi` (fallback) |

On Arch:

```sh
sudo pacman -S wireplumber libpulse bluez-utils networkmanager brightnessctl pacman-contrib
```

Widgets gracefully degrade (hide or show defaults) when their tools are missing.

### Permission details

#### input group (CapsLock)

The CapsLock widget reads keyboard LED events from `/dev/input/event*` via evdev.
These device files are owned by `root:input`, so your user must be in the `input` group.
Without this, the widget silently disables itself.

#### video group (Brightness)

The Brightness widget uses `brightnessctl` to write `/sys/class/backlight/*/brightness`.
On most distros this file requires the `video` group for unprivileged write access.
Not needed on desktops without a backlight device.

#### RAPL powercap (CpuDraw/PsysDraw)

The CpuDraw and PsysDraw widgets read Intel/AMD RAPL energy counters from `/sys/class/powercap/`.
These files are root-only on most distros and require the tmpfiles rule above.
Without this, BatteryDraw and GpuDraw still work but CpuDraw/PsysDraw are omitted.

## Building

```sh
cargo build --release
```

## Configuration

Select a config profile at runtime:

```sh
rs-bar --config macbook   # default
rs-bar --config intel
```

Config profiles are defined in `src/config/`. Each profile specifies:

- Theme, font, icon theme, icon size
- Bar height and border colors
- Widget layout (left / center-left / center / center-right / right zones)
- External commands (power menu, brightness control)
- WireGuard connection name

## Logging

```sh
RS_BAR_LOG=info cargo run
RS_BAR_LOG=debug cargo run
```

## License

MIT
