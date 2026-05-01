//! WireGuard tunnel-state hub. Polls nmcli once a second.
//!
//! Tracks whether the nmcli connection named by
//! [`config::WIREGUARD_CONNECTION`] is currently active. Subscribers receive a
//! `bool` (`true` = up, `false` = down) via `tokio::sync::watch`.
//!
//! The poll itself is a one-shot `nmcli -t -f NAME,TYPE con show --active`
//! parse — there is no stable, low-overhead sysfs path for nmcli connection
//! state. We poll once a second on a timerfd; click-driven transitions in the
//! widget are reflected within ~1 s without any extra plumbing, and the
//! syscall cost is dominated by spawning `nmcli` (a few ms once a second).
//!
//! Singleton background thread (`"wireguard"`) shared across every bar
//! instance.

use std::sync::OnceLock;
use tokio::sync::watch;

use super::sys::timerfd_loop;

use crate::relm4_bar::config;

/// Run `nmcli -t -f NAME,TYPE con show --active` and check whether a row
/// matches our connection name and is of `wireguard` type.
fn query_active(connection: &str) -> bool {
    std::process::Command::new("nmcli")
        .args(["-t", "-f", "NAME,TYPE", "con", "show", "--active"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l.contains("wireguard") && l.starts_with(connection))
        })
        .unwrap_or(false)
}

fn sender() -> &'static watch::Sender<bool> {
    static S: OnceLock<watch::Sender<bool>> = OnceLock::new();
    S.get_or_init(|| {
        let connection = config::WIREGUARD_CONNECTION();
        let (tx, _rx) = watch::channel(query_active(connection));
        let producer = tx.clone();
        std::thread::Builder::new()
            .name("wireguard".into())
            .spawn(move || {
                // 1s poll: nmcli shellout costs ~10ms each; 0.5s would
                // burn a noticeable percent of one core for a binary signal
                // that rarely changes.
                timerfd_loop(std::time::Duration::from_secs(1), false, || {
                    let active = query_active(connection);
                    // Returning false would exit the loop; in practice the
                    // sender is held by the OnceLock for the program's
                    // lifetime so this never happens.
                    producer.send_if_modified(|cur| {
                        if *cur != active {
                            *cur = active;
                            true
                        } else {
                            false
                        }
                    });
                    true
                });
            })
            .ok();
        tx
    })
}

pub fn subscribe() -> watch::Receiver<bool> {
    sender().subscribe()
}
