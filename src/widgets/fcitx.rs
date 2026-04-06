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
    grouped: bool,
}

impl BarWidget for Fcitx {
    const NAME: &str = "fcitx";

    fn new(cx: &mut Context<Self>) -> Self {
        let rx = super::tray::subscribe_tray();

        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
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
            grouped: false,
        }
    }

    fn set_grouped(&mut self) {
        self.grouped = true;
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::config::THEME;

        let (label, color) = match self.im {
            InputMethod::Chinese => ("中", t.purple),
            InputMethod::English => ("EN", t.fg_dark),
            InputMethod::Unknown => ("?", t.yellow),
        };

        super::capsule(
            div()
                .flex()
                .items_center()
                .justify_center()
                .px(px(6.0))
                .text_color(rgb(color))
                .font_weight(gpui::FontWeight::BOLD)
                .text_xs()
                .child(label),
            self.grouped,
        )
    }
}

impl_render!(Fcitx);
