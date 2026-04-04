# rs-bar

A Wayland status bar built with Rust + [GPUI](https://github.com/zed-industries/zed), designed for the [niri](https://github.com/YaLTeR/niri) compositor.

## Widgets

| Widget | Description |
|--------|-------------|
| Workspaces | Niri workspace switcher |
| Minimap | Tiled window layout visualization |
| WindowTitle | Focused window title + icon |
| CpuUsage | CPU usage % (timerfd + epoll on `/proc/stat`) |
| Clock | Time display with date popup |
| Date | Date display (MM-DD) |
| Wifi | Wi-Fi status (netlink + sysfs, zero subprocesses) |
| Bluetooth | Bluetooth state (epoll on bluetoothctl) |
| PkgUpdate | Package update indicator (Arch/Debian/Fedora + Flatpak) |
| PowerDraw | Power consumption (battery, RAPL, dGPU hwmon, timerfd + epoll) |
| Volume | PulseAudio volume (scroll to adjust, click to mute) |
| Brightness | Screen brightness (scroll to adjust) |
| Tray | Collapsible system tray (SNI protocol) |
| Fcitx | Input method indicator (fcitx5) |
| CapsLock | CapsLock indicator (evdev + epoll, event-driven) |
| Power | Power/logout button |

## Prerequisites

### input group (CapsLock widget)

The CapsLock widget reads keyboard LED events from `/dev/input/event*` via evdev.
These device files are owned by `root:input`, so your user must be in the `input` group:

```sh
sudo usermod -aG input $USER
```

Log out and back in for the change to take effect.
Without this, the widget silently disables itself.

### RAPL powercap permissions (PowerDraw CPU/PSYS)

The PowerDraw widget reads Intel RAPL energy counters from `/sys/class/powercap/`.
These files are root-only on most distros. Install a udev rule to make them readable:

```sh
sudo tee /etc/udev/rules.d/99-rapl-powercap.rules <<'EOF'
SUBSYSTEM=="powercap", RUN+="/bin/find /sys/devices/virtual/powercap/ -name energy_uj -exec /bin/chmod a+r {} +"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

For immediate effect (persists via the udev rule on next boot):

```sh
sudo find /sys/devices/virtual/powercap/ -name "energy_uj" -o -name "max_energy_range_uj" | xargs sudo chmod a+r
```

Without this, the widget still shows battery and GPU power but omits CPU/PSYS.

## Building

```sh
cargo build --release
```

## Configuration

Edit `src/config.rs` to customize:

- Theme, font, icon theme
- Bar height and border colors
- Widget layout (left / center / right zones)
- External commands (power menu, brightness control)

## Logging

```sh
RS_BAR_LOG=info cargo run
RS_BAR_LOG=debug cargo run
```

## License

MIT
