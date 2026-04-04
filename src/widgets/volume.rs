use std::io::BufRead;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AppContext as _, Bounds, Context, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, ScrollWheelEvent, Stateful, StatefulInteractiveElement,
    Styled, Window, WindowBounds, WindowKind, WindowOptions, div, point, px, rgb, size, svg,
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
};

use super::{BarWidget, impl_render};

// ── Audio state ──

#[derive(Clone)]
struct Sink {
    index: u32,
    name: String,
    description: String,
    volume_pct: u32,
    muted: bool,
    is_default: bool,
}

#[derive(Clone)]
struct Source {
    index: u32,
    name: String,
    description: String,
    volume_pct: u32,
    muted: bool,
    is_default: bool,
}

#[derive(Clone)]
struct AudioState {
    volume: f32,
    muted: bool,
    sinks: Vec<Sink>,
    sources: Vec<Source>,
}

// ── Audio server (singleton, event-driven) ──

struct AudioServer {
    state: Arc<Mutex<AudioState>>,
    subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>>,
}

fn query_volume() -> (f32, bool) {
    let output = std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output();
    match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            let muted = s.contains("[MUTED]");
            let volume = s.split_whitespace().nth(1)
                .and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
            (volume, muted)
        }
        Err(_) => (0.0, false),
    }
}

fn query_sinks() -> Vec<Sink> {
    let default_sink = std::process::Command::new("pactl")
        .args(["get-default-sink"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let Ok(o) = std::process::Command::new("pactl")
        .args(["-f", "json", "list", "sinks"]).output() else { return Vec::new() };
    let Ok(sinks) = serde_json::from_slice::<Vec<serde_json::Value>>(&o.stdout) else { return Vec::new() };

    sinks.into_iter().filter_map(|s| {
        let index = s.get("index")?.as_u64()? as u32;
        let name = s.get("name")?.as_str()?.to_string();
        let description = s.get("description")?.as_str()?.to_string();
        let muted = s.get("mute")?.as_bool()?;
        let vol_obj = s.get("volume")?;
        let first_channel = vol_obj.as_object()?.values().next()?;
        let volume_pct = first_channel.get("value_percent")?.as_str()?
            .trim_end_matches('%').parse::<u32>().ok()?;
        Some(Sink { index, is_default: name == default_sink, name, description, volume_pct, muted })
    }).collect()
}

fn query_sources() -> Vec<Source> {
    let default_source = std::process::Command::new("pactl")
        .args(["get-default-source"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    let Ok(o) = std::process::Command::new("pactl")
        .args(["-f", "json", "list", "sources"]).output() else { return Vec::new() };
    let Ok(sources) = serde_json::from_slice::<Vec<serde_json::Value>>(&o.stdout) else { return Vec::new() };

    sources.into_iter().filter_map(|s| {
        let name = s.get("name")?.as_str()?.to_string();
        // Skip monitor sources (they mirror sinks)
        if name.contains(".monitor") { return None; }
        let index = s.get("index")?.as_u64()? as u32;
        let description = s.get("description")?.as_str()?.to_string();
        let muted = s.get("mute")?.as_bool()?;
        let vol_obj = s.get("volume")?;
        let first_channel = vol_obj.as_object()?.values().next()?;
        let volume_pct = first_channel.get("value_percent")?.as_str()?
            .trim_end_matches('%').parse::<u32>().ok()?;
        Some(Source { index, is_default: name == default_source, name, description, volume_pct, muted })
    }).collect()
}

fn query_full_state() -> AudioState {
    let (volume, muted) = query_volume();
    AudioState { volume, muted, sinks: query_sinks(), sources: query_sources() }
}

fn audio_server() -> &'static AudioServer {
    static SERVER: OnceLock<AudioServer> = OnceLock::new();
    SERVER.get_or_init(|| {
        let state = Arc::new(Mutex::new(query_full_state()));
        let subscribers: Arc<Mutex<Vec<async_channel::Sender<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let ev_state = state.clone();
        let ev_subs = subscribers.clone();

        std::thread::Builder::new().name("audio-monitor".into()).spawn(move || loop {
            let Ok(mut child) = std::process::Command::new("pactl")
                .arg("subscribe").stdout(std::process::Stdio::piped()).spawn()
            else { std::thread::sleep(Duration::from_secs(5)); continue };

            let stdout = child.stdout.take().unwrap();
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.contains("sink") || line.contains("server") {
                    *ev_state.lock().unwrap() = query_full_state();
                    let mut subs = ev_subs.lock().unwrap();
                    subs.retain(|tx| !tx.is_closed());
                    for tx in subs.iter() { let _ = tx.try_send(()); }
                }
            }
            let _ = child.wait();
            std::thread::sleep(Duration::from_secs(1));
        }).expect("failed to spawn audio monitor");

        AudioServer { state, subscribers }
    })
}

fn subscribe_audio() -> async_channel::Receiver<()> {
    let server = audio_server();
    let (tx, rx) = async_channel::bounded(1);
    server.subscribers.lock().unwrap().push(tx);
    rx
}

// ── Bar widget ──

pub struct Volume {
    state: AudioState,
    hovered: bool,
    show_expanded: bool,
    ever_expanded: bool,
}

impl Volume {
    fn icon_path(&self) -> &'static str {
        if self.state.muted || self.state.volume == 0.0 {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-muted.svg")
        } else if self.state.volume < 0.33 {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-low.svg")
        } else if self.state.volume < 0.66 {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-medium.svg")
        } else {
            concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/volume-high.svg")
        }
    }
}

impl BarWidget for Volume {
    const NAME: &str = "volume";

    fn new(cx: &mut Context<Self>) -> Self {
        let initial = audio_server().state.lock().unwrap().clone();
        let server_state = audio_server().state.clone();
        let rx = subscribe_audio();

        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let new_state = server_state.lock().unwrap().clone();
                if this.update(cx, |this, cx| { this.state = new_state; cx.notify(); }).is_err() {
                    break;
                }
            }
        }).detach();

        Self { state: initial, hovered: false, show_expanded: false, ever_expanded: false }
    }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let icon_size = crate::config::ICON_SIZE;
        let entity = cx.weak_entity();
        let volume_pct = (self.state.volume * 100.0).round() as u32;
        let expanded = self.show_expanded;
        let animate = self.ever_expanded;
        let bar_color = if self.state.muted { t.border } else { t.accent };

        // Capsule dimensions
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let collapsed_w = icon_size + 8.0; // icon + px padding
        let expanded_w = icon_size + 8.0 + 4.0 + 32.0 + 4.0 + 28.0; // icon + gap + bar + gap + text
        let vol_fill = self.state.volume.min(1.0);

        // Content: icon is always at the left, bar+text follow
        let content = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(4.0))
            .whitespace_nowrap()
            .px(px(4.0))
            .child(
                svg().external_path(self.icon_path().to_string())
                    .size(px(icon_size)).text_color(rgb(t.fg)).flex_shrink_0(),
            )
            .child(
                div().w(px(32.0)).h(px(3.0)).rounded_full().bg(rgb(t.border)).flex_shrink_0()
                    .child(div().w(px(32.0 * vol_fill)).h_full().rounded_full().bg(rgb(bar_color))),
            )
            .child(
                div().text_xs().text_color(rgb(t.text_dim)).flex_shrink_0()
                    .child(format!("{:>3}%", volume_pct)),
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

        div()
            .id("volume")
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
                            // Delay before collapsing
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
                let step = if f32::from(delta.y) > 0.0 { "5%+" } else { "5%-" };
                std::thread::spawn(move || {
                    let _ = std::process::Command::new("wpctl")
                        .args(["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", step]).output();
                });
            })
            // Click to toggle mute
            .on_mouse_down(MouseButton::Left, move |_event, _window, _cx| {
                std::thread::spawn(|| {
                    let _ = std::process::Command::new("wpctl")
                        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]).output();
                });
            })
            .child(content)
            .with_animation(
                if expanded { "vol-expand" } else { "vol-collapse" },
                Animation::new(Duration::from_millis(if expanded { 400 } else { 300 }))
                    .with_easing(if expanded { ease_expand } else { ease_collapse }),
                move |el, progress| {
                    let target = if expanded { expanded_w } else { collapsed_w };
                    // Before first expansion, skip animation (from == target)
                    let from = if !animate {
                        target
                    } else if expanded {
                        collapsed_w
                    } else {
                        expanded_w
                    };
                    let w = from + (target - from) * progress;

                    let border_from: gpui::Hsla = rgb(t.border).into();
                    let border_to: gpui::Hsla = rgb(t.accent).into();
                    let p = if expanded { progress } else { 1.0 - progress };
                    let mut blended = border_to;
                    blended.a = if animate { p } else { 0.0 };
                    let border = border_from.blend(blended);

                    el.w(px(w)).border_color(border)
                },
            )
    }
}

impl_render!(Volume);
