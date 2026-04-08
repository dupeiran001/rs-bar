use gpui::{Context, IntoElement, Styled, Window, div, px};

use super::{BarWidget, impl_render};

pub struct Notch;

impl BarWidget for Notch {
    const NAME: &str = "notch";

    fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(196.0))
    }
}

impl_render!(Notch);
