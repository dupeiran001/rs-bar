mod battery;
mod bluetooth;
mod brightness;
mod capslock;
mod clock;
mod cpu_freq;
mod cpu_temp;
mod cpu_usage;
mod date;
mod fcitx;
mod gpu_busy;
mod memory;
mod minimap;
mod notch;
mod pkg_update;
mod power;
pub(crate) mod power_draw;
pub(crate) mod tray;
mod volume;
mod wifi;
mod window_title;
mod wireguard;
mod workspaces;

pub use battery::Battery;
pub use bluetooth::Bluetooth;
pub use brightness::Brightness;
pub use capslock::CapsLock;
pub use clock::Clock;
pub use cpu_freq::CpuFreq;
pub use cpu_temp::CpuTemp;
pub use cpu_usage::CpuUsage;
pub use date::Date;
pub use fcitx::Fcitx;
pub use gpu_busy::GpuBusy;
pub use memory::Memory;
pub use minimap::Minimap;
pub use notch::Notch;
pub use pkg_update::PkgUpdate;
pub use power::Power;
#[allow(unused_imports)]
pub use power_draw::{BatteryDraw, CpuDraw, GpuDraw, PsysDraw};
pub use tray::Tray;
pub use volume::Volume;
pub use wifi::Wifi;
pub use window_title::WindowTitle;
pub use wireguard::Wireguard;
pub use workspaces::Workspaces;

use gpui::{AnyView, AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb};

/// The single trait widget authors implement.
pub trait BarWidget: 'static + Sized {
    const NAME: &str;

    fn new(cx: &mut Context<Self>) -> Self;
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;

    /// Called when this widget is placed inside a `group!()`.
    /// Widgets should skip their own capsule wrapper when grouped.
    fn set_grouped(&mut self) {}
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
                    <Self as $crate::gpui_bar::widgets::BarWidget>::render(self, window, cx)
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

    /// Build a grouped widget from entries produced by `group!()`.
    pub fn build_group(entries: Vec<GroupEntry>, cx: &mut impl AppContext) -> Self {
        let entity = cx.new(|_cx| WidgetGroup { entries });
        Self {
            name: "group",
            view: entity.into(),
        }
    }
}

// ── Widget grouping ────────────────────────────────────────────────────

pub enum GroupEntry {
    Widget(AnyView),
    Separator,
}

struct WidgetGroup {
    entries: Vec<GroupEntry>,
}

impl Render for WidgetGroup {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::gpui_bar::config::THEME();
        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;

        let mut row = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
            .px(px(4.0))
            .gap(px(4.0))
            .text_xs();

        let sep_h = (button_h - 10.0).max(6.0);

        for entry in &self.entries {
            match entry {
                GroupEntry::Widget(view) => {
                    row = row.child(view.clone());
                }
                GroupEntry::Separator => {
                    row = row.child(
                        div()
                            .flex_shrink_0()
                            .w(px(1.0))
                            .h(px(sep_h))
                            .bg(rgb(t.fg_gutter)),
                    );
                }
            }
        }

        row
    }
}

/// Build a group of widgets in a shared capsule.
///
/// ```ignore
/// group!(cx, CpuFreq, CpuUsage, |, CpuTemp, |, Memory)
/// ```
///
/// Widgets are separated by commas. Use `|` for a visual separator (│).
macro_rules! group {
    (@item $cx:expr, $entries:ident, |) => {
        $entries.push($crate::gpui_bar::widgets::GroupEntry::Separator);
    };
    (@item $cx:expr, $entries:ident, $w:ident) => {{
        use $crate::gpui_bar::widgets::BarWidget as _;
        use ::gpui::AppContext as _;
        let entity = $cx.new(|cx| {
            let mut w = $w::new(cx);
            w.set_grouped();
            w
        });
        $entries.push($crate::gpui_bar::widgets::GroupEntry::Widget(entity.into()));
    }};
    ($cx:expr, $($item:tt),* $(,)?) => {{
        let mut entries: Vec<$crate::gpui_bar::widgets::GroupEntry> = Vec::new();
        $(group!(@item $cx, entries, $item);)*
        $crate::gpui_bar::widgets::Widget::build_group(entries, $cx)
    }};
}
pub(crate) use group;

/// Helper: apply capsule styling to a div, or skip if grouped.
/// Returns the div with or without capsule shell.
pub(crate) fn capsule(el: gpui::Div, grouped: bool) -> gpui::Div {
    if grouped {
        el
    } else {
        let t = crate::gpui_bar::config::THEME();
        let content_h = crate::gpui_bar::config::CONTENT_HEIGHT();
        let button_h = content_h - 4.0;
        let radius = button_h / 2.0;
        el.h(px(button_h))
            .rounded(px(radius))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.surface))
    }
}
