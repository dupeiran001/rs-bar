//! Package update indicator widget.
//!
//! Auto-detects the distro's package manager at startup and queries
//! for available updates. Supports Arch (pacman/checkupdates + yay/paru),
//! Debian/Ubuntu (apt), Fedora/RHEL (dnf), plus Flatpak.
//! Polls every 10 minutes in a background thread.

use std::process::Command;
use std::time::Duration;

use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

#[derive(Clone, PartialEq)]
struct UpdateState {
    official: u32,
    aur: u32,
    flatpak: u32,
}

impl UpdateState {
    fn total(&self) -> u32 {
        self.official + self.aur + self.flatpak
    }
}

// ── helpers ────────────────────────────────────────────────────────────

fn has_cmd(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn count_lines(cmd: &str, args: &[&str]) -> u32 {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .count() as u32
        })
        .unwrap_or(0)
}

// ── distro detection ───────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Distro {
    Arch,
    Debian,
    Fedora,
}

fn detect_distro() -> Option<Distro> {
    if std::path::Path::new("/etc/arch-release").exists() {
        Some(Distro::Arch)
    } else if std::path::Path::new("/etc/debian_version").exists() {
        Some(Distro::Debian)
    } else if std::path::Path::new("/etc/fedora-release").exists()
        || std::path::Path::new("/etc/redhat-release").exists()
    {
        Some(Distro::Fedora)
    } else if has_cmd("pacman") {
        Some(Distro::Arch)
    } else if has_cmd("apt") {
        Some(Distro::Debian)
    } else if has_cmd("dnf") {
        Some(Distro::Fedora)
    } else {
        None
    }
}

// ── per-distro queries ─────────────────────────────────────────────────

fn query_arch() -> (u32, u32) {
    let official = if has_cmd("checkupdates") {
        count_lines("checkupdates", &[])
    } else {
        // fallback: pacman -Qu (requires synced db)
        count_lines("pacman", &["-Qu"])
    };

    let aur = if has_cmd("yay") {
        count_lines("yay", &["-Qua"])
    } else if has_cmd("paru") {
        count_lines("paru", &["-Qua"])
    } else {
        0
    };

    (official, aur)
}

fn query_debian() -> u32 {
    // apt update is typically run by a systemd timer; we just check
    let output = Command::new("apt")
        .args(["list", "--upgradable"])
        .env("LANG", "C")
        .output();
    match output {
        Ok(o) => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| l.contains("upgradable"))
                .count() as u32
        }
        Err(_) => 0,
    }
}

fn query_fedora() -> u32 {
    // dnf check-update exits 100 when updates available, 0 when none
    let output = Command::new("dnf")
        .args(["check-update", "-q"])
        .output();
    match output {
        Ok(o) => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty() && !l.starts_with("Last metadata"))
                .count() as u32
        }
        Err(_) => 0,
    }
}

fn query_flatpak() -> u32 {
    if !has_cmd("flatpak") {
        return 0;
    }
    count_lines("flatpak", &["remote-ls", "--updates"])
}

fn query_updates(distro: Option<Distro>) -> UpdateState {
    let (official, aur) = match distro {
        Some(Distro::Arch) => query_arch(),
        Some(Distro::Debian) => (query_debian(), 0),
        Some(Distro::Fedora) => (query_fedora(), 0),
        None => (0, 0),
    };
    let flatpak = query_flatpak();
    UpdateState {
        official,
        aur,
        flatpak,
    }
}

// ── widget ─────────────────────────────────────────────────────────────

pub struct PkgUpdate {
    state: UpdateState,
    grouped: bool,
}

impl BarWidget for PkgUpdate {
    const NAME: &str = "pkg-update";

    fn new(cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<UpdateState>(1);
        let distro = detect_distro();

        {
            let tx = tx.clone();
            std::thread::Builder::new()
                .name("pkg-update-init".into())
                .spawn(move || {
                    let _ = tx.send_blocking(query_updates(distro));
                })
                .ok();
        }

        std::thread::Builder::new()
            .name("pkg-update-poll".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(600));
                let _ = tx.send_blocking(query_updates(distro));
            })
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(new) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.state != new {
                            this.state = new;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            state: UpdateState {
                official: 0,
                aur: 0,
                flatpak: 0,
            },
            grouped: false,
        }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();

        let total = self.state.total();
        let (icon_path, color) = if total == 0 {
            (
                concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/pkg-uptodate.svg"),
                t.fg_dark,
            )
        } else {
            (
                concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/pkg-updates.svg"),
                t.green,
            )
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .justify_center()
                .px(px(6.0))
                .child(
                    svg()
                        .external_path(icon_path.to_string())
                        .size(px(crate::config::ICON_SIZE()))
                        .text_color(rgb(color))
                        .flex_shrink_0(),
                ),
            self.grouped,
        )
    }
}

impl_render!(PkgUpdate);
