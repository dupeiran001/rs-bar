//! Package-update hub. Auto-detects the distro's package manager at startup
//! and polls for the count of available updates every 10 minutes.
//!
//! Singleton background thread (`"pkg-update"`) shared across every bar
//! instance. Subscribers receive the latest total count via
//! `tokio::sync::watch`. The first poll fires immediately at startup so the
//! widget can render a real value without waiting 10 minutes.
//!
//! Supports Arch (pacman/checkupdates + yay/paru), Debian/Ubuntu (apt),
//! Fedora/RHEL (dnf), plus Flatpak. The published value is the total across
//! all sources.

use std::process::Command;
use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

/// Poll interval in seconds — 10 minutes, matching rs-bar.
const INTERVAL_SECS: i64 = 600;

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
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| l.contains("upgradable"))
            .count() as u32,
        Err(_) => 0,
    }
}

fn query_fedora() -> u32 {
    // dnf check-update exits 100 when updates available, 0 when none
    let output = Command::new("dnf").args(["check-update", "-q"]).output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("Last metadata"))
            .count() as u32,
        Err(_) => 0,
    }
}

fn query_flatpak() -> u32 {
    if !has_cmd("flatpak") {
        return 0;
    }
    count_lines("flatpak", &["remote-ls", "--updates"])
}

fn query_total(distro: Option<Distro>) -> u32 {
    let (official, aur) = match distro {
        Some(Distro::Arch) => query_arch(),
        Some(Distro::Debian) => (query_debian(), 0),
        Some(Distro::Fedora) => (query_fedora(), 0),
        None => (0, 0),
    };
    official + aur + query_flatpak()
}

// ── singleton ──────────────────────────────────────────────────────────

fn sender() -> &'static watch::Sender<u32> {
    static S: OnceLock<watch::Sender<u32>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, _rx) = watch::channel(0u32);
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("pkg-update".into())
            .spawn(move || {
                let distro = detect_distro();
                // `fire_immediately=true` so the first count is produced at
                // startup rather than after a 10-minute wait.
                timerfd_loop(INTERVAL_SECS, true, || {
                    let total = query_total(distro);
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send(total).is_ok()
                });
            })
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<u32> {
    sender().subscribe()
}
