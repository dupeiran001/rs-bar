//! Battery widget with expandable hover details.
//!
//! Shows battery icon with level-based coloring. On hover, expands to show
//! charge percentage, real-time power draw, estimated time remaining,
//! battery health, and design capacity.
//! All data from sysfs — zero subprocesses.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, px, rgb, svg,
};

use super::power_draw;
use super::{BarWidget, impl_render};

// ── battery state ───────────────────────────────────────────────────────

#[derive(Clone)]
struct BatteryState {
    percent: u32,
    watts: f64,
    charging: bool,
    energy_full_wh: f64,
    energy_design_wh: f64,
    health_pct: f32,
    est_hours: Option<f64>,
}

const ICON_BATTERY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/battery.svg");
const ICON_CHARGING: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/icons/battery-charging.svg"
);

fn read_state(bat: &power_draw::BatteryInfo) -> BatteryState {
    let percent = power_draw::sysfs_u64(&bat.dir.join("capacity")).unwrap_or(0) as u32;
    let charging = power_draw::sysfs_str(&bat.dir.join("status")) == "Charging";

    let watts = if let Some(w) = power_draw::sysfs_i64(&bat.dir.join("power_now")) {
        w.unsigned_abs() as f64 / 1e6
    } else if let (Some(ua), Some(uv)) = (
        power_draw::sysfs_i64(&bat.dir.join("current_now")),
        power_draw::sysfs_i64(&bat.dir.join("voltage_now")),
    ) {
        (ua.unsigned_abs() as f64 * uv.unsigned_abs() as f64) / 1e12
    } else {
        0.0
    };

    let energy_now_wh = power_draw::sysfs_u64(&bat.dir.join("energy_now"))
        .map(|u| u as f64 / 1e6)
        .unwrap_or(0.0);
    let energy_full_wh = power_draw::sysfs_u64(&bat.dir.join("energy_full"))
        .map(|u| u as f64 / 1e6)
        .unwrap_or(0.0);
    let energy_design_wh = power_draw::sysfs_u64(&bat.dir.join("energy_full_design"))
        .map(|u| u as f64 / 1e6)
        .unwrap_or(0.0);

    let health_pct = if energy_design_wh > 0.0 {
        ((energy_full_wh / energy_design_wh) * 100.0) as f32
    } else {
        100.0
    };

    let est_hours = if watts > 0.1 {
        if charging {
            Some((energy_full_wh - energy_now_wh) / watts)
        } else {
            Some(energy_now_wh / watts)
        }
    } else {
        None
    };

    BatteryState {
        percent,
        watts,
        charging,
        energy_full_wh,
        energy_design_wh,
        health_pct,
        est_hours,
    }
}

fn fmt_time(hours: Option<f64>) -> String {
    let Some(h) = hours else {
        return "--".to_string();
    };
    if h < 0.0 || h > 99.0 {
        return "--".to_string();
    }
    let total_min = (h * 60.0) as u32;
    let hr = total_min / 60;
    let mn = total_min % 60;
    if hr > 0 {
        format!("{}h{}m", hr, mn)
    } else {
        format!("{}m", mn)
    }
}

// ── widget ──────────────────────────────────────────────────────────────

pub struct Battery {
    state: Option<BatteryState>,
    hovered: bool,
    show_expanded: bool,
    ever_expanded: bool,
}

impl BarWidget for Battery {
    const NAME: &str = "battery";

    fn new(cx: &mut Context<Self>) -> Self {
        let bat = power_draw::detect_battery();

        if let Some(bat) = bat {
            log::info!("battery: {}", bat.dir.display());
            let (tx, rx) = async_channel::bounded::<BatteryState>(1);

            std::thread::Builder::new()
                .name("battery".into())
                .spawn(move || {
                    power_draw::timerfd_loop(2, true, || {
                        let state = read_state(&bat);
                        !tx.try_send(state).is_err() || !tx.is_closed()
                    });
                })
                .ok();

            cx.spawn(async move |this, cx| {
                while let Ok(s) = rx.recv().await {
                    if this
                        .update(cx, |this, cx| {
                            this.state = Some(s);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        } else {
            log::info!("battery: no battery found");
        }

        Self {
            state: None,
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
        let expanded = self.show_expanded;
        let animate = self.ever_expanded;

        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        let collapsed_w = button_h;

        // Extract state-dependent values (defaults when no battery)
        let (icon, level_color, has_data, pct, watts, time_str, health, capacity, fill) =
            if let Some(s) = &self.state {
                let lc = if s.charging {
                    t.green
                } else if s.percent >= 60 {
                    t.green
                } else if s.percent >= 30 {
                    t.yellow
                } else if s.percent >= 15 {
                    t.orange
                } else {
                    t.red
                };
                let ic = if s.charging {
                    ICON_CHARGING
                } else {
                    ICON_BATTERY
                };
                (
                    ic,
                    lc,
                    true,
                    s.percent,
                    s.watts,
                    fmt_time(s.est_hours),
                    s.health_pct,
                    s.energy_design_wh,
                    (s.percent as f32 / 100.0).min(1.0),
                )
            } else {
                (
                    ICON_BATTERY,
                    t.fg_dark,
                    false,
                    0,
                    0.0,
                    "--".into(),
                    0.0,
                    0.0,
                    0.0,
                )
            };

        // Expanded content: bar(32) + pct + watts + time + health + capacity
        let expanded_w = if has_data {
            collapsed_w
                + 4.0
                + 32.0
                + 4.0
                + 28.0
                + 4.0
                + 36.0
                + 4.0
                + 40.0
                + 4.0
                + 36.0
                + 4.0
                + 40.0
                + 8.0
        } else {
            collapsed_w
        };

        let mut content = div()
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
                            .external_path(icon.to_string())
                            .size(px(icon_size))
                            .text_color(rgb(level_color))
                            .flex_shrink_0(),
                    ),
            );

        if has_data {
            content = content.child(
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
                                    .bg(rgb(level_color)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.fg))
                            .flex_shrink_0()
                            .child(format!("{:>3}%", pct)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0()
                            .child(format!("{:.1}W", watts)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0()
                            .child(format!("~{}", time_str)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0()
                            .child(format!("{:.0}%h", health)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_dim))
                            .flex_shrink_0()
                            .child(format!("{:.1}Wh", capacity)),
                    ),
            );
        }

        fn ease_expand(t: f32) -> f32 {
            let u = 1.0 - t;
            1.0 - u * u * u
        }

        fn ease_collapse(t: f32) -> f32 {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                let u = 2.0 * t - 2.0;
                1.0 + 0.5 * u * u * u
            }
        }

        div()
            .id("battery")
            .flex()
            .items_center()
            .h(px(button_h))
            .overflow_hidden()
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
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
            .child(content)
            .with_animation(
                if expanded {
                    "bat-expand"
                } else {
                    "bat-collapse"
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
                    let border_to: gpui::Hsla = rgb(level_color).into();
                    let p = if expanded { progress } else { 1.0 - progress };
                    let mut blended = border_to;
                    blended.a = if animate { p } else { 0.0 };
                    let border = border_from.blend(blended);

                    el.w(px(w)).border_color(border)
                },
            )
    }
}

impl_render!(Battery);
