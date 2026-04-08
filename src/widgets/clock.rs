use std::time::Duration;

use chrono::Local;
use gpui::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowKind, WindowOptions, div, px, rgb,
};

use super::{BarWidget, impl_render};

pub struct Clock {
    time: String,
    popup_open: bool,
}

fn format_time() -> String {
    Local::now().format("%H:%M").to_string()
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
        let t = crate::config::THEME();
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
        cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;

                if this
                    .update(cx, |this, cx| {
                        this.time = format_time();
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
            time: format_time(),
            popup_open: false,
        }
    }

    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME();
        let entity = cx.weak_entity();

        div()
            .id("clock")
            .flex()
            .items_center()
            .justify_center()
            .h(px(crate::config::CONTENT_HEIGHT()))
            .px_2()
            .rounded_md()
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(t.fg))
            .hover(|s| s.bg(rgb(t.surface)))
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
            .child(self.time.clone())
    }
}

impl_render!(Clock);
