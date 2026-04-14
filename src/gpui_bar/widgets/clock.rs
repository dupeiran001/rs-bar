use std::time::Duration;

use chrono::Local;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowKind, WindowOptions, div, px, rgb,
};

use super::{BarWidget, impl_render};

pub struct Clock {
    time: String,
    date: String,
    popup_open: bool,
}

fn format_time() -> String {
    Local::now().format("%H:%M").to_string()
}

fn format_date_short() -> String {
    Local::now().format("%b %-d").to_string()
}

fn format_time_full() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

fn format_date() -> String {
    Local::now().format("%A, %B %e, %Y").to_string()
}

struct ClockPopup {
    time: String,
    date: String,
}

impl ClockPopup {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;

                if this
                    .update(cx, |this, cx| {
                        this.time = format_time_full();
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
            time: format_time_full(),
            date: format_date(),
        }
    }
}

impl Render for ClockPopup {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        div()
            .bg(rgb(t.bg))
            .border_1()
            .border_color(rgb(t.border))
            .rounded_lg()
            .p_4()
            .min_w(px(200.0))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_color(rgb(t.fg))
                    .text_xl()
                    .flex()
                    .justify_center()
                    .child(self.time.clone()),
            )
            .child(
                div()
                    .text_color(rgb(t.text_dim))
                    .flex()
                    .justify_center()
                    .child(self.date.clone()),
            )
    }
}

impl BarWidget for Clock {
    const NAME: &str = "clock";

    fn new(cx: &mut Context<Self>) -> Self {
        // Wake once per second to check for a minute-rollover, but only
        // notify GPUI (triggering a repaint) when the visible "HH:MM" /
        // "Mon DD" strings actually change. With a minute-granularity clock
        // that's one notify per minute instead of 60 — eliminates 59 of 60
        // main-thread repaint wakeups per minute.
        cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;

                if this
                    .update(cx, |this, cx| {
                        let new_time = format_time();
                        let new_date = format_date_short();
                        if new_time != this.time || new_date != this.date {
                            this.time = new_time;
                            this.date = new_date;
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
            time: format_time(),
            date: format_date_short(),
            popup_open: false,
        }
    }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let entity = cx.weak_entity();

        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;

        div()
            .id("clock")
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .h(px(button_h))
            .px(px(9.0))
            .pb(px(2.0))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .cursor_pointer()
            .text_color(rgb(t.fg))
            .hover(|s| s.bg(rgb(t.border)))
            .on_click(move |_event, _window, cx| {
                let _ = entity.update(cx, |this, cx| {
                    if this.popup_open {
                        return;
                    }
                    this.popup_open = true;
                    let entity = cx.weak_entity();

                    cx.open_window(
                        WindowOptions {
                            kind: WindowKind::PopUp,
                            focus: true,
                            app_id: Some("mybar-clock".into()),
                            ..Default::default()
                        },
                        |_window, cx| cx.new(|cx| ClockPopup::new(cx)),
                    )
                    .ok();

                    cx.spawn(async move |_this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(500))
                            .await;
                        let _ = entity.update(cx, |this, cx| {
                            this.popup_open = false;
                            cx.notify();
                        });
                    })
                    .detach();
                });
            })
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(t.accent))
                    .line_height(px(12.0))
                    .child(self.time.clone()),
            )
            .child(
                div()
                    .text_color(rgb(t.accent_dim))
                    .text_size(px(9.0))
                    .line_height(px(10.0))
                    .child(self.date.clone()),
            )
    }
}

impl_render!(Clock);
