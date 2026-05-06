use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AppContext as _, Context, InteractiveElement, IntoElement,
    MouseButton, ParentElement, ScrollWheelEvent, StatefulInteractiveElement, Styled, Window, div,
    px, rgb, svg,
};

use super::{BarWidget, impl_render};

#[derive(Clone)]
struct BrightnessState {
    percent: u32,
}

/// A sysfs backlight device discovered at startup. Stored with both the
/// current and max file paths so each poll is two tiny file reads — no
/// subprocess fork, no shell, no parsing gymnastics.
struct Backlight {
    current: PathBuf,
    max: u32,
}

struct BrightnessServer {
    state: Arc<Mutex<BrightnessState>>,
    subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>>,
    // `None` when no backlight was detected (common on desktops); callers
    // then treat the widget as a no-op rather than fork `brightnessctl`
    // every 2s for nothing.
    backlight: Option<Arc<Backlight>>,
}

/// Look for a backlight device in `/sys/class/backlight/`. Returns the first
/// entry that exposes a readable `brightness` + `max_brightness` pair.
fn detect_backlight() -> Option<Backlight> {
    let entries = std::fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.filter_map(Result::ok) {
        let dir = entry.path();
        let current = dir.join("brightness");
        let max = dir.join("max_brightness");
        if let (Ok(_), Ok(max_s)) = (
            std::fs::read_to_string(&current),
            std::fs::read_to_string(&max),
        ) {
            if let Ok(max_val) = max_s.trim().parse::<u32>() {
                if max_val > 0 {
                    return Some(Backlight {
                        current,
                        max: max_val,
                    });
                }
            }
        }
    }
    None
}

fn read_sysfs_brightness(bl: &Backlight) -> BrightnessState {
    let percent = std::fs::read_to_string(&bl.current)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|cur| (cur.saturating_mul(100) / bl.max).min(100))
        .unwrap_or(0);
    BrightnessState { percent }
}

fn brightness_server() -> &'static BrightnessServer {
    static SERVER: OnceLock<BrightnessServer> = OnceLock::new();
    SERVER.get_or_init(|| {
        let backlight = detect_backlight().map(Arc::new);
        let initial = match &backlight {
            Some(bl) => {
                log::info!(
                    "brightness: sysfs {} (max={})",
                    bl.current.display(),
                    bl.max
                );
                read_sysfs_brightness(bl)
            }
            None => {
                log::info!("brightness: no backlight detected, widget disabled");
                BrightnessState { percent: 0 }
            }
        };
        let state = Arc::new(Mutex::new(initial));
        let subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Only spawn the poll thread when there's actually something to read.
        if let Some(bl) = &backlight {
            let poll_state = state.clone();
            let poll_subs = subscribers.clone();
            let poll_bl = bl.clone();
            std::thread::Builder::new()
                .name("brightness-monitor".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_secs(2));
                        let new = read_sysfs_brightness(&poll_bl);
                        let mut current = poll_state.lock().unwrap();
                        if current.percent != new.percent {
                            *current = new;
                            drop(current);
                            let mut subs = poll_subs.lock().unwrap();
                            subs.retain(|tx| !tx.is_closed());
                            for tx in subs.iter() {
                                let _ = tx.try_send(());
                            }
                        }
                    }
                })
                .expect("failed to spawn brightness monitor");
        }

        BrightnessServer {
            state,
            subscribers,
            backlight,
        }
    })
}

fn subscribe_brightness() -> async_channel::Receiver<()> {
    let server = brightness_server();
    let (tx, rx) = async_channel::bounded(1);
    server.subscribers.lock().unwrap().push(tx);
    rx
}

/// Force a re-query after a brightness change command. No-op if there's no
/// backlight (we never spawned the monitor in that case).
fn notify_brightness_change() {
    let server = brightness_server();
    let Some(bl) = server.backlight.as_ref() else {
        return;
    };
    let new = read_sysfs_brightness(bl);
    *server.state.lock().unwrap() = new;
    let mut subs = server.subscribers.lock().unwrap();
    subs.retain(|tx| !tx.is_closed());
    for tx in subs.iter() {
        let _ = tx.try_send(());
    }
}

pub struct Brightness {
    state: BrightnessState,
    hovered: bool,
    show_expanded: bool,
    ever_expanded: bool,
}

impl Brightness {
    fn icon_path(&self) -> &'static str {
        if self.state.percent < 50 {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/icons/brightness-low.svg"
            )
        } else {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/icons/brightness-high.svg"
            )
        }
    }
}

impl BarWidget for Brightness {
    const NAME: &str = "brightness";

    fn new(cx: &mut Context<Self>) -> Self {
        let initial = brightness_server().state.lock().unwrap().clone();
        let server_state = brightness_server().state.clone();
        let rx = subscribe_brightness();

        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let new_state = server_state.lock().unwrap().clone();
                if this
                    .update(cx, |this, cx| {
                        this.state = new_state;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            state: initial,
            hovered: false,
            show_expanded: false,
            ever_expanded: false,
        }
    }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let icon_size = crate::gpui_bar::config::ICON_SIZE();
        let entity = cx.weak_entity();
        let pct = self.state.percent;
        let expanded = self.show_expanded;
        let animate = self.ever_expanded;
        let bar_color = t.yellow;

        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let collapsed_w = button_h; // at least as wide as tall for a circular look
        let expanded_w = collapsed_w + 4.0 + 32.0 + 4.0 + 28.0 + 8.0;
        let fill = (pct as f32 / 100.0).min(1.0);

        let content = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .whitespace_nowrap()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(collapsed_w))
                    .flex_shrink_0()
                    .child(
                        svg()
                            .external_path(self.icon_path().to_string())
                            .size(px(icon_size))
                            .text_color(rgb(t.fg))
                            .flex_shrink_0(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .pr(px(8.0))
                    .child(
                        div()
                            .w(px(32.0))
                            .h(px(3.0))
                            .rounded_full()
                            .bg(rgb(t.border))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .w(px(32.0 * fill))
                                    .h_full()
                                    .rounded_full()
                                    .bg(rgb(bar_color)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0()
                            .child(format!("{:>3}%", pct)),
                    ),
            );

        // Cubic ease-out: fast start, smooth deceleration
        fn ease_expand(t: f32) -> f32 {
            let u = 1.0 - t;
            1.0 - u * u * u
        }

        // Cubic ease-in-out: smooth S-curve
        fn ease_collapse(t: f32) -> f32 {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                let u = 2.0 * t - 2.0;
                1.0 + 0.5 * u * u * u
            }
        }

        let up_cmd = crate::gpui_bar::config::BRIGHTNESS_UP_CMD();
        let down_cmd = crate::gpui_bar::config::BRIGHTNESS_DOWN_CMD();

        div()
            .id("brightness")
            .flex()
            .items_center()
            .h(px(button_h))
            .overflow_hidden()
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .cursor_pointer()
            .on_hover({
                let entity = entity.clone();
                move |is_hovered, _window, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.hovered = *is_hovered;
                        if !is_hovered {
                            let entity = cx.weak_entity();
                            cx.spawn(async move |_this, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_millis(300))
                                    .await;
                                let _ = entity.update(cx, |this, cx| {
                                    if !this.hovered {
                                        this.show_expanded = false;
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                        } else {
                            let entity = cx.weak_entity();
                            cx.spawn(async move |_this, cx| {
                                cx.background_executor()
                                    .timer(Duration::from_millis(150))
                                    .await;
                                let _ = entity.update(cx, |this, cx| {
                                    if this.hovered {
                                        this.show_expanded = true;
                                        this.ever_expanded = true;
                                        cx.notify();
                                    }
                                });
                            })
                            .detach();
                        }
                    });
                }
            })
            .on_scroll_wheel(move |event: &ScrollWheelEvent, _window, _cx| {
                let delta = event.delta.pixel_delta(px(1.0));
                let cmd = if f32::from(delta.y) > 0.0 {
                    up_cmd
                } else {
                    down_cmd
                };
                std::thread::spawn(move || {
                    let _ = std::process::Command::new("sh").args(["-c", cmd]).output();
                    notify_brightness_change();
                });
            })
            .child(content)
            .with_animation(
                if expanded {
                    "brt-expand"
                } else {
                    "brt-collapse"
                },
                Animation::new(Duration::from_millis(if expanded { 400 } else { 300 }))
                    .with_easing(if expanded { ease_expand } else { ease_collapse }),
                move |el, progress| {
                    let target = if expanded { expanded_w } else { collapsed_w };
                    let from = if !animate {
                        target
                    } else if expanded {
                        collapsed_w
                    } else {
                        expanded_w
                    };
                    let w = from + (target - from) * progress;

                    let border_from: gpui::Hsla = rgb(t.border).into();
                    let border_to: gpui::Hsla = rgb(t.yellow).into();
                    let p = if expanded { progress } else { 1.0 - progress };
                    let mut blended = border_to;
                    blended.a = if animate { p } else { 0.0 };
                    let border = border_from.blend(blended);

                    el.w(px(w)).border_color(border)
                },
            )
    }
}

impl_render!(Brightness);
