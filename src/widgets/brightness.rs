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

struct BrightnessServer {
    state: Arc<Mutex<BrightnessState>>,
    subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>>,
}

fn query_brightness() -> BrightnessState {
    let cmd = &crate::config::BRIGHTNESS_GET_CMD();
    let output = std::process::Command::new("sh").args(["-c", cmd]).output();
    let percent = match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            // Parse percentage from various formats: "50%", "50", "Current brightness: 50 (50%)"
            s.split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .last()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0)
        }
        Err(_) => 0,
    };
    BrightnessState { percent }
}

fn brightness_server() -> &'static BrightnessServer {
    static SERVER: OnceLock<BrightnessServer> = OnceLock::new();
    SERVER.get_or_init(|| {
        let state = Arc::new(Mutex::new(query_brightness()));
        let subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>> =
            Arc::new(Mutex::new(Vec::new()));

        let ev_state = state.clone();
        let ev_subs = subscribers.clone();

        // Poll-based since there's no universal brightness event stream.
        // Only polls every 2s to stay lightweight; scroll actions trigger immediate re-query.
        std::thread::Builder::new()
            .name("brightness-monitor".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                let new = query_brightness();
                let mut current = ev_state.lock().unwrap();
                if current.percent != new.percent {
                    *current = new;
                    drop(current);
                    let mut subs = ev_subs.lock().unwrap();
                    subs.retain(|tx| !tx.is_closed());
                    for tx in subs.iter() {
                        let _ = tx.try_send(());
                    }
                }
            })
            .expect("failed to spawn brightness monitor");

        BrightnessServer { state, subscribers }
    })
}

fn subscribe_brightness() -> async_channel::Receiver<()> {
    let server = brightness_server();
    let (tx, rx) = async_channel::bounded(1);
    server.subscribers.lock().unwrap().push(tx);
    rx
}

/// Force a re-query after a brightness change command
fn notify_brightness_change() {
    let server = brightness_server();
    let new = query_brightness();
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
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/brightness-low.svg")
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
        let t = crate::config::THEME();
        let content_h = crate::config::CONTENT_HEIGHT();
        let icon_size = crate::config::ICON_SIZE();
        let entity = cx.weak_entity();
        let pct = self.state.percent;
        let expanded = self.show_expanded;
        let animate = self.ever_expanded;
        let bar_color = t.yellow;

        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let collapsed_w = icon_size + 8.0;
        let expanded_w = icon_size + 8.0 + 4.0 + 32.0 + 4.0 + 28.0;
        let fill = (pct as f32 / 100.0).min(1.0);

        let content = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(4.0))
            .whitespace_nowrap()
            .px(px(4.0))
            .child(
                svg()
                    .external_path(self.icon_path().to_string())
                    .size(px(icon_size))
                    .text_color(rgb(t.fg))
                    .flex_shrink_0(),
            )
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

        let up_cmd = crate::config::BRIGHTNESS_UP_CMD();
        let down_cmd = crate::config::BRIGHTNESS_DOWN_CMD();

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
                            }).detach();
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
                            }).detach();
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
                    let _ = std::process::Command::new("sh")
                        .args(["-c", cmd])
                        .output();
                    notify_brightness_change();
                });
            })
            .child(content)
            .with_animation(
                if expanded { "brt-expand" } else { "brt-collapse" },
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
