use env_logger::Env;
use gpui::{
    App, AppContext as _, Bounds, Context, FontWeight, ParentElement, Render, Styled, Window,
    WindowBounds, WindowKind, WindowOptions, div,
    layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions},
    point, px, rgb, size,
};
use gpui_platform::application;

mod config;
mod niri;
mod theme;
mod widgets;

use widgets::Widget;

pub(crate) struct Bar {
    left: Vec<Widget>,
    center_left: Vec<Widget>,
    center: Vec<Widget>,
    center_right: Vec<Widget>,
    right: Vec<Widget>,
}

impl Render for Bar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let t = config::THEME;
        div()
            .relative()
            .h(px(config::BAR_HEIGHT))
            .w_full()
            .bg(rgb(t.bg))
            .text_color(rgb(t.fg))
            .font_family(config::FONT_FAMILY)
            .font_weight(FontWeight::SEMIBOLD)
            .text_xs()
            // Content row: left | center_left | center (fixed) | center_right | right
            .child(
                div()
                    .flex()
                    .h_full()
                    // Left group: takes available space, content aligned left
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .overflow_hidden()
                            .items_center()
                            .gap_2()
                            .children(self.left.iter().map(|w| w.view().clone())),
                    )
                    // Center-left group: takes available space, content aligned right
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .overflow_hidden()
                            .justify_end()
                            .items_center()
                            .gap_2()
                            .children(self.center_left.iter().map(|w| w.view().clone())),
                    )
                    // Center group: auto width, always exactly centered
                    .child(
                        div()
                            .flex()
                            .overflow_hidden()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .children(self.center.iter().map(|w| w.view().clone())),
                    )
                    // Center-right group: takes available space, content aligned left
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .overflow_hidden()
                            .items_center()
                            .gap_2()
                            .children(self.center_right.iter().map(|w| w.view().clone())),
                    )
                    // Right group: takes available space, content aligned right
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .overflow_hidden()
                            .justify_end()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .children(self.right.iter().map(|w| w.view().clone())),
                    ),
            )
            // Top border overlay — painted last, always on top
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(1.0))
                    .bg(rgb(config::BORDER_TOP)),
            )
            // Bottom border overlay
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(1.0))
                    .bg(rgb(config::BORDER_BOTTOM)),
            )
    }
}

fn open_bar(display_id: Option<gpui::DisplayId>, cx: &mut App) {
    cx.open_window(
        WindowOptions {
            display_id,
            kind: WindowKind::LayerShell(LayerShellOptions {
                namespace: "bar".to_string(),
                layer: Layer::Top,
                anchor: Anchor::LEFT | Anchor::RIGHT | Anchor::TOP,
                exclusive_zone: Some(px(config::BAR_HEIGHT)),
                exclusive_edge: None,
                margin: None,
                keyboard_interactivity: KeyboardInteractivity::None,
            }),
            titlebar: None,
            focus: false,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            app_id: Some("mybar".into()),
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(1.), px(config::BAR_HEIGHT)),
            })),
            ..Default::default()
        },
        |_window, cx| {
            let bar = config::bar(cx);
            cx.new(|cx| {
                cx.observe_global::<niri::NiriState>(|_bar, cx| {
                    cx.notify();
                })
                .detach();
                bar
            })
        },
    )
    .unwrap();
}

fn main() {
    let env = Env::new().filter("RS_BAR_LOG").write_style("RS_BAR");
    env_logger::init_from_env(env);

    // Suppress GPUI's internal zbus/tokio panic on worker threads
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.to_string();
        if msg.contains("no reactor running") {
            return; // swallow GPUI's zbus/tokio context panic
        }
        default_hook(info);
    }));

    application().run(|cx: &mut App| {
        niri::start_listener(cx);

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(100))
                .await;

            cx.update(|cx: &mut App| {
                let displays = cx.displays();

                if displays.is_empty() {
                    log::warn!("No displays yet; opening fallback on main display");
                    open_bar(None, cx);
                } else {
                    for display in displays {
                        open_bar(Some(display.id()), cx);
                    }
                }
            });
        })
        .detach();
    });
}
