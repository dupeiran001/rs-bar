//! Widget framework. Each widget is a relm4 Component with `const NAME` and
//! `Init = WidgetInit`. The `widgets!()` and `group!()` macros build a
//! `Vec<Widget>` for a bar zone, type-erasing each Component's controller.

use std::cell::RefCell;

use gtk::prelude::*;
use relm4::prelude::*;

/// Per-bar context set by `app.rs` immediately before launching widgets for a
/// given monitor's bar. Niri-aware widgets read `current_connector()` once in
/// their `init` to know which monitor they live on.
pub struct BarContext {
    pub connector: String,
}

thread_local! {
    pub static BAR_CTX: RefCell<Option<BarContext>> = const { RefCell::new(None) };
}

/// Returns the current bar's monitor connector name (e.g. "DP-2"), if a Bar
/// is currently being built. Returns None outside a bar-build window.
#[allow(dead_code)]
pub fn current_connector() -> Option<String> {
    BAR_CTX.with(|c| c.borrow().as_ref().map(|x| x.connector.clone()))
}

mod battery;
mod battery_draw;
mod bluetooth;
mod brightness;
mod capslock;
mod clock;
mod cpu_draw;
mod cpu_freq;
mod cpu_temp;
mod cpu_usage;
mod date;
mod fcitx;
mod gpu_busy;
mod gpu_draw;
mod memory;
mod minimap;
mod notch;
mod pkg_update;
mod power;
mod psys_draw;
mod tray;
mod volume;
mod wifi;
mod window_title;
mod wireguard;
mod workspaces;

pub use battery::Battery;
pub use battery_draw::BatteryDraw;
pub use bluetooth::Bluetooth;
pub use brightness::Brightness;
pub use capslock::CapsLock;
pub use clock::Clock;
pub use cpu_draw::CpuDraw;
pub use cpu_freq::CpuFreq;
pub use cpu_temp::CpuTemp;
pub use cpu_usage::CpuUsage;
#[allow(unused_imports)]
pub use date::Date;
pub use fcitx::Fcitx;
pub use gpu_busy::GpuBusy;
pub use gpu_draw::GpuDraw;
pub use memory::Memory;
pub use minimap::Minimap;
pub use notch::Notch;
pub use pkg_update::PkgUpdate;
pub use power::Power;
pub use psys_draw::PsysDraw;
pub use tray::Tray;
pub use volume::Volume;
pub use wifi::Wifi;
pub use window_title::WindowTitle;
pub use wireguard::Wireguard;
pub use workspaces::Workspaces;

/// Init payload all widgets accept. `grouped` true means the widget skips its
/// own capsule wrapper because a parent Group provides one.
#[derive(Clone, Copy, Default)]
pub struct WidgetInit {
    pub grouped: bool,
}

/// Type-erased handle to a launched widget Component. `root` is the GTK widget
/// to attach into the bar layout; `_controller` keeps the Controller alive.
#[allow(dead_code)]
pub struct Widget {
    pub name: &'static str,
    pub root: gtk::Widget,
    _controller: Box<dyn std::any::Any>,
}

/// Trait implemented by all widget Components for stable name access.
pub trait NamedWidget: Component<Init = WidgetInit> {
    const NAME: &'static str;
}

/// Build a widget by launching its Component. Returns a type-erased Widget.
pub fn build<C>(grouped: bool) -> Widget
where
    C: NamedWidget + 'static,
    C::Root: glib::object::IsA<gtk::Widget> + Clone,
{
    let controller = C::builder().launch(WidgetInit { grouped }).detach();
    let root: gtk::Widget = controller.widget().clone().upcast();
    Widget {
        name: C::NAME,
        root,
        _controller: Box::new(controller),
    }
}

// ── Group widget ───────────────────────────────────────────────────────

pub enum GroupEntry {
    Widget(gtk::Widget, Box<dyn std::any::Any>),
    Separator,
}

/// Build a group: a horizontal Box with `.bar-group` class containing
/// child widgets and gtk::Separator between them where `|` was used.
pub fn build_group(entries: Vec<GroupEntry>) -> Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    row.add_css_class("bar-group");

    let mut keepers: Vec<Box<dyn std::any::Any>> = Vec::new();

    for entry in entries {
        match entry {
            GroupEntry::Widget(w, keeper) => {
                row.append(&w);
                keepers.push(keeper);
            }
            GroupEntry::Separator => {
                let sep = gtk::Separator::new(gtk::Orientation::Vertical);
                row.append(&sep);
            }
        }
    }

    Widget {
        name: "group",
        root: row.upcast(),
        _controller: Box::new(keepers),
    }
}

/// Apply capsule styling to a widget root. Widgets call this in their `init`
/// when `!grouped`. Adds the `bar-capsule` CSS class.
pub fn capsule(w: &impl glib::object::IsA<gtk::Widget>, grouped: bool) {
    if !grouped {
        w.add_css_class("bar-capsule");
    }
}

/// Toggle a widget into exactly one CSS class out of a mutually-exclusive set,
/// stripping all the others first. Used for color bands / on-off states.
///
/// Reusable across every widget that selects from a fixed palette of classes.
pub(crate) fn set_exclusive_class<W: glib::object::IsA<gtk::Widget>>(
    w: &W,
    class: &str,
    all: &[&str],
) {
    for c in all {
        if *c != class {
            w.remove_css_class(c);
        }
    }
    w.add_css_class(class);
}

// ── Macros ─────────────────────────────────────────────────────────────

/// Build a single grouped widget for use inside `group!()`.
#[doc(hidden)]
pub fn build_for_group<C>() -> GroupEntry
where
    C: NamedWidget + 'static,
    C::Root: glib::object::IsA<gtk::Widget> + Clone,
{
    let controller = C::builder().launch(WidgetInit { grouped: true }).detach();
    let root: gtk::Widget = controller.widget().clone().upcast();
    GroupEntry::Widget(root, Box::new(controller))
}

/// Construct a Group widget containing the given children, separated by `|`.
///
///     group!(CpuFreq, CpuUsage, |, CpuTemp)
#[macro_export]
macro_rules! group {
    (@item $entries:ident, |) => {
        $entries.push($crate::relm4_bar::widgets::GroupEntry::Separator);
    };
    (@item $entries:ident, $w:ident) => {
        $entries.push($crate::relm4_bar::widgets::build_for_group::<$crate::relm4_bar::widgets::$w>());
    };
    ($($item:tt),* $(,)?) => {{
        let mut entries: Vec<$crate::relm4_bar::widgets::GroupEntry> = Vec::new();
        $($crate::group!(@item entries, $item);)*
        $crate::relm4_bar::widgets::build_group(entries)
    }};
}

/// Build a `Vec<Widget>` mixing plain widgets and `group!()` calls.
///
///     widgets!(Workspaces, group!(CpuUsage, |, CpuTemp), Memory)
#[macro_export]
macro_rules! widgets {
    (@acc [$($out:expr),*]) => {
        vec![$($out),*]
    };
    (@acc [$($out:expr),*] group!($($g:tt)*) , $($rest:tt)*) => {
        $crate::widgets!(@acc [$($out,)* $crate::group!($($g)*)] $($rest)*)
    };
    (@acc [$($out:expr),*] group!($($g:tt)*)) => {
        $crate::widgets!(@acc [$($out,)* $crate::group!($($g)*)])
    };
    (@acc [$($out:expr),*] $w:ident , $($rest:tt)*) => {
        $crate::widgets!(@acc [$($out,)* $crate::relm4_bar::widgets::build::<$crate::relm4_bar::widgets::$w>(false)] $($rest)*)
    };
    (@acc [$($out:expr),*] $w:ident) => {
        $crate::widgets!(@acc [$($out,)* $crate::relm4_bar::widgets::build::<$crate::relm4_bar::widgets::$w>(false)])
    };
    ($($items:tt)*) => {
        $crate::widgets!(@acc [] $($items)*)
    };
}
