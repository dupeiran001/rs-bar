use gpui::{
    BoxShadow, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, point, px, rgb, svg,
};

use super::{BarWidget, impl_render};

pub struct Power;

impl BarWidget for Power {
    const NAME: &str = "power";

    fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;
        let command = crate::config::POWER_COMMAND;
        let button_h = content_h - 4.0;
        let radius = (button_h - 2.0) / 2.0;
        let shadow_color = rgb(t.bg).into();

        div()
            .flex()
            .items_center()
            .h(px(content_h))
            .pr(px(4.0))
            .child(
                div()
                    .id("power")
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(button_h))
                    .px(px(4.0))
                    .rounded(px(radius))
                    .cursor_pointer()
                    .bg(rgb(t.teal))
                    .text_color(rgb(t.bg))
                    .shadow(vec![BoxShadow {
                        color: shadow_color,
                        offset: point(px(1.), px(0.)),
                        blur_radius: px(2.),
                        spread_radius: px(1.),
                    }])
                    .hover(|s| s.bg(rgb(t.surface)).text_color(rgb(t.fg)))
                    .on_click(move |_, _, _| {
                        std::thread::spawn(move || {
                            std::process::Command::new("sh")
                                .arg("-c")
                                .arg(command)
                                .spawn()
                                .ok();
                        });
                    })
                    .child(
                        svg()
                            .external_path(crate::config::POWER_ICON.to_string())
                            .size(px(button_h - 4.0))
                            .text_color(rgb(t.bg)),
                    ),
            )
    }
}

impl_render!(Power);
