use std::time::Duration;

use chrono::Local;
use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb, svg};

use super::{BarWidget, impl_render};

pub struct Date {
    date: String,
}

fn format_date() -> String {
    Local::now().format("%m-%d").to_string()
}

impl BarWidget for Date {
    const NAME: &str = "date";

    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                let new = format_date();
                if this
                    .update(cx, |this, cx| {
                        if this.date != new {
                            this.date = new;
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
            date: format_date(),
        }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;

        div()
            .flex()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .px(px(8.0))
            .gap(px(4.0))
            .text_xs()
            .text_color(rgb(t.fg))
            .child(
                svg()
                    .external_path(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/calendar.svg").to_string())
                    .size(px(crate::config::ICON_SIZE))
                    .text_color(rgb(t.fg))
                    .flex_shrink_0(),
            )
            .child(self.date.clone())
    }
}

impl_render!(Date);
