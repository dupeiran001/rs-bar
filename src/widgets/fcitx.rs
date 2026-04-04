use gpui::{Context, IntoElement, ParentElement, Styled, Window, div, px, rgb};

use super::{BarWidget, impl_render};

#[derive(Clone, PartialEq)]
enum InputMethod {
    Chinese,
    English,
    Unknown,
}

fn query_fcitx() -> InputMethod {
    let output = std::process::Command::new("fcitx5-remote")
        .arg("-n")
        .output();

    match output {
        Ok(o) => {
            let im = String::from_utf8_lossy(&o.stdout).trim().to_string();
            match im.as_str() {
                s if s.starts_with("pinyin") || s.starts_with("rime") => InputMethod::Chinese,
                s if s.starts_with("keyboard") => InputMethod::English,
                "" => InputMethod::Unknown,
                _ => InputMethod::Unknown,
            }
        }
        Err(_) => InputMethod::Unknown,
    }
}

pub struct Fcitx {
    im: InputMethod,
}

impl BarWidget for Fcitx {
    const NAME: &str = "fcitx";

    fn new(cx: &mut Context<Self>) -> Self {
        // Subscribe to tray events — fcitx updates its SNI on input method switch
        let rx = super::tray::subscribe_tray();

        // Event-driven: wakes only when tray fires an event, then queries fcitx
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                // query_fcitx() is blocking but fast (single process call)
                let new_im = query_fcitx();

                if this
                    .update(cx, |this, cx| {
                        if this.im != new_im {
                            this.im = new_im;
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
            im: query_fcitx(),
        }
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;
        let content_h = crate::config::CONTENT_HEIGHT;

        let (label, color) = match self.im {
            InputMethod::Chinese => ("中", t.purple),
            InputMethod::English => ("EN", t.fg_dark),
            InputMethod::Unknown => ("?", t.yellow),
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(content_h))
            .px(px(6.0))
            .bg(rgb(t.border))
            .text_color(rgb(color))
            .font_weight(gpui::FontWeight::BOLD)
            .text_xs()
            .child(label)
    }
}

impl_render!(Fcitx);
