//! WireGuard VPN toggle widget.
//!
//! Two states: connected / disconnected.
//! Left-click toggles the configured nmcli WireGuard connection.
//! Monitors state via `nmcli monitor` (epoll on stdout).

use std::io::BufRead;
use std::time::Duration;

use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, Window, div, px,
    rgb, svg,
};

use super::{BarWidget, impl_render};

const ICON_ON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/vpn-on.svg");
const ICON_OFF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/vpn-off.svg");

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

pub struct Wireguard {
    active: bool,
    connection: &'static str,
    grouped: bool,
}

impl BarWidget for Wireguard {
    const NAME: &str = "wireguard";

    fn new(cx: &mut Context<Self>) -> Self {
        let connection = crate::config::WIREGUARD_CONNECTION();
        let initial = query_active(connection);

        let (tx, rx) = async_channel::bounded::<bool>(4);

        // Monitor via nmcli monitor — event-driven
        std::thread::Builder::new()
            .name("wg-monitor".into())
            .spawn({
                let conn = connection.to_string();
                move || {
                    let mut backoff = Duration::from_secs(1);
                    let max_backoff = Duration::from_secs(60);
                    loop {
                        use std::os::unix::process::CommandExt;
                        let mut cmd = std::process::Command::new("nmcli");
                        cmd.args(["monitor"])
                            .stdout(std::process::Stdio::piped())
                            .stdin(std::process::Stdio::null());
                        // Kill this child when the parent dies instead
                        // of leaving it orphaned. Must run in the forked
                        // child between fork and exec.
                        unsafe {
                            cmd.pre_exec(|| {
                                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                                Ok(())
                            });
                        }
                        let Ok(mut child) = cmd.spawn()
                        else {
                            std::thread::sleep(backoff);
                            backoff = (backoff * 2).min(max_backoff);
                            continue;
                        };

                        let stdout = child.stdout.take().unwrap();
                        let reader = std::io::BufReader::new(stdout);
                        backoff = Duration::from_secs(1);

                        for line in reader.lines() {
                            let Ok(line) = line else { break };
                            // Fire on any connection state change
                            if line.contains("connected")
                                || line.contains("disconnected")
                                || line.contains(&conn)
                                || line.contains("wireguard")
                            {
                                let state = query_active(&conn);
                                let _ = tx.try_send(state);
                            }
                        }
                        let _ = child.wait();
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            })
            .ok();

        cx.spawn(async move |this, cx| {
            while let Ok(active) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.active != active {
                            this.active = active;
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
            active: initial,
            connection,
            grouped: false,
        }
    }

    fn set_grouped(&mut self) { self.grouped = true; }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let icon_size = crate::config::ICON_SIZE();

        let (icon, color) = if self.active {
            (ICON_ON, t.green)
        } else {
            (ICON_OFF, t.fg_dark)
        };

        let conn = self.connection.to_string();
        let active = self.active;

        // Channel for click-triggered state updates
        let (click_tx, click_rx) = async_channel::bounded::<bool>(1);
        cx.spawn(async move |this, cx| {
            while let Ok(new_state) = click_rx.recv().await {
                if this.update(cx, |this, cx| {
                    this.active = new_state;
                    cx.notify();
                }).is_err() { break; }
            }
        }).detach();

        let content_h = crate::config::CONTENT_HEIGHT();
        let button_h = content_h - 4.0;

        super::capsule(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(button_h))
                .child(
                    svg()
                        .external_path(icon.to_string())
                        .size(px(icon_size))
                        .text_color(rgb(color))
                        .flex_shrink_0(),
                ),
            self.grouped,
        )
            .id("wireguard")
            .cursor_pointer()
            .hover(|s| s.bg(rgb(t.bg_dark)))
            .on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
                let conn = conn.clone();
                let tx = click_tx.clone();
                std::thread::spawn(move || {
                    if active {
                        let _ = std::process::Command::new("nmcli")
                            .args(["con", "down", "id", &conn])
                            .output();
                    } else {
                        let _ = std::process::Command::new("nmcli")
                            .args(["con", "up", "id", &conn])
                            .output();
                    }
                    let _ = tx.send_blocking(query_active(&conn));
                });
            })
    }
}

impl_render!(Wireguard);
