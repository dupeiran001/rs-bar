mod bluetooth;
mod brightness;
mod capslock;
mod clock;
mod cpu_usage;
mod date;
mod fcitx;
mod minimap;
mod pkg_update;
mod power;
mod power_draw;
pub(crate) mod tray;
mod volume;
mod wifi;
mod window_title;
mod workspaces;

pub use bluetooth::Bluetooth;
pub use brightness::Brightness;
pub use capslock::CapsLock;
pub use clock::Clock;
pub use cpu_usage::CpuUsage;
pub use date::Date;
pub use fcitx::Fcitx;
pub use minimap::Minimap;
pub use pkg_update::PkgUpdate;
pub use power::Power;
pub use power_draw::PowerDraw;
pub use tray::Tray;
pub use volume::Volume;
pub use wifi::Wifi;
pub use window_title::WindowTitle;
pub use workspaces::Workspaces;

use gpui::{AnyView, AppContext, Context, IntoElement, Render, Window};

/// The single trait widget authors implement. Defines identity, construction,
/// and rendering.
///
/// Rust's orphan rules prevent a blanket `impl<W: BarWidget> Render for W`,
/// so use [`impl_render!`] to generate the trivial `Render` forwarding.
pub trait BarWidget: 'static + Sized {
    const NAME: &str;

    fn new(cx: &mut Context<Self>) -> Self;
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}

/// Generates the GPUI `Render` impl by forwarding to `BarWidget::render`.
macro_rules! impl_render {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ::gpui::Render for $ty {
                fn render(
                    &mut self,
                    window: &mut ::gpui::Window,
                    cx: &mut ::gpui::Context<Self>,
                ) -> impl ::gpui::IntoElement {
                    <Self as $crate::widgets::BarWidget>::render(self, window, cx)
                }
            }
        )+
    };
}
pub(crate) use impl_render;

/// Type-erased widget handle stored in the bar.
#[allow(dead_code)]
pub struct Widget {
    name: &'static str,
    view: AnyView,
}

#[allow(dead_code)]
impl Widget {
    /// Create a widget: builds the GPUI entity and type-erases it.
    pub fn build<W: BarWidget + Render>(cx: &mut impl AppContext) -> Self {
        let entity = cx.new(|cx| W::new(cx));
        Self {
            name: W::NAME,
            view: entity.into(),
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn view(&self) -> &AnyView {
        &self.view
    }
}
