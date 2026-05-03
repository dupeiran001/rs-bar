//! [`BarPopover`] is a minimal scaffolding helper around `gtk::Popover`.
//!
//! Every panel popover (Volume, Brightness, WiFi, Bluetooth, Tray) needs the
//! same five-step boilerplate: build with `autohide(true)`, add a
//! `*-popover` CSS class, set the parent to the bar-line widget, set a
//! content child, and wire a primary-button click on the bar-line widget to
//! `popup()`. Going through this builder collapses those copies into one
//! place, and gives the animations stage a single hook (the auto-added
//! `popover-animated` CSS class) for entrance keyframes once they land.
//!
//! Each widget's content layout is bespoke (different spacings, margins,
//! sections, headers) so the builder deliberately does *not* provide
//! convenience layouts — callers assemble their own `gtk::Box` and pass it
//! to [`BarPopoverBuilder::build`].

use gtk::prelude::*;
use std::time::Duration;

const POPOVER_EXIT_MS: u64 = 300;

/// Constructed popover with the click handler not yet wired. Either call
/// [`Self::attach_click`] on the bar-line widget that should toggle it, or
/// stash the [`Self::popover`] handle and call [`Self::popup`] from a custom
/// gesture / event controller.
pub struct BarPopover {
    pub popover: gtk::Popover,
}

pub struct BarPopoverBuilder {
    popover: gtk::Popover,
}

impl BarPopoverBuilder {
    /// `parent` is the bar-line widget the popover anchors to. `css_class`
    /// is the per-widget hook (`"volume-popover"`, `"wifi-popover"`, …)
    /// the theme CSS keys off of.
    pub fn new(parent: &impl IsA<gtk::Widget>, css_class: &str) -> Self {
        let popover = gtk::Popover::builder().autohide(true).build();
        popover.add_css_class(css_class);
        popover.set_has_arrow(false);
        popover.set_offset(0, 0);
        popover.set_parent(parent);
        Self { popover }
    }

    /// Add an extra CSS class. Useful for per-widget variant hooks.
    #[allow(dead_code)]
    pub fn with_class(self, class: &str) -> Self {
        self.popover.add_css_class(class);
        self
    }

    /// Set the popover's child widget and finalize the builder. The caller
    /// owns the content layout (margins, spacings, sections); we just plug
    /// it in.
    pub fn build(self, content: &impl IsA<gtk::Widget>) -> BarPopover {
        set_liquid_child(&self.popover, content);
        let bp = BarPopover { popover: self.popover };
        bp.install_entrance_animation();
        bp
    }
}

impl BarPopover {
    pub fn builder(
        parent: &impl IsA<gtk::Widget>,
        css_class: &str,
    ) -> BarPopoverBuilder {
        BarPopoverBuilder::new(parent, css_class)
    }

    #[allow(dead_code)]
    pub fn popup(&self) {
        popup(&self.popover);
    }

    #[allow(dead_code)]
    pub fn popdown(&self) {
        popdown(&self.popover);
    }

    /// Wire a primary-button click on `target` to call `popup()` on this
    /// popover. The default for bar-line widgets that just open on click.
    pub fn attach_click(&self, target: &impl IsA<gtk::Widget>) {
        let popover = self.popover.clone();
        let click = gtk::GestureClick::new();
        click.set_button(gtk::gdk::BUTTON_PRIMARY);
        click.connect_pressed(move |_, _, _, _| toggle(&popover));
        target.as_ref().add_controller(click);
    }

    /// Underlying `gtk::Popover`. Escape-hatch for callers that need to
    /// hook `connect_show`, set a custom position, store it on the model,
    /// or call `popdown()` from a message handler.
    #[allow(dead_code)]
    pub fn inner(&self) -> &gtk::Popover {
        &self.popover
    }

    /// Hook `connect_show` / `connect_closed` to toggle a one-shot
    /// `popover-entering` CSS class so the entrance keyframe replays on
    /// every open. Without this, GTK4 reuses the popover `contents` node
    /// across `popup()` calls and CSS animations fire only the first time.
    /// We remove the class then re-add it via an idle callback so the
    /// browser-equivalent "force layout pass" happens between the two.
    fn install_entrance_animation(&self) {
        install_motion(&self.popover);
    }

    /// Stagger-reveal the given `revealers` in order on every popover
    /// open. Each revealer is flipped to `set_reveal_child(true)` with
    /// `stagger_ms` between starts. On close, all revealers reset so the
    /// next open re-cascades.
    ///
    /// `Crossfade` is the recommended transition type when wrapping
    /// sections — it allocates the child's full size up front, so the
    /// popover surface doesn't resize as sections appear; only opacity
    /// fades. The outer popover's scale-in animation supplies the slide
    /// motion; the inner cascade supplies the staggered cue.
    ///
    /// Currently no caller — the volume popover that started with this
    /// pattern was simplified back to direct appends because the cascade
    /// felt like a slow-start every time. Kept for the tray collapse
    /// (Stage 7), where a long item list benefits more from staggering.
    #[allow(dead_code)]
    pub fn cascade_reveal(&self, revealers: Vec<gtk::Revealer>, stagger_ms: u64) {
        if revealers.is_empty() {
            return;
        }
        let on_show = revealers.clone();
        self.popover.connect_show(move |_| {
            for (i, r) in on_show.iter().enumerate() {
                let delay = (i as u64) * stagger_ms;
                let r = r.clone();
                if delay == 0 {
                    r.set_reveal_child(true);
                } else {
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(delay),
                        move || {
                            r.set_reveal_child(true);
                        },
                    );
                }
            }
        });
        let on_close = revealers;
        self.popover.connect_closed(move |_| {
            for r in &on_close {
                r.set_reveal_child(false);
            }
        });
    }
}

pub fn set_liquid_child(popover: &impl IsA<gtk::Popover>, content: &impl IsA<gtk::Widget>) {
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("popover-liquid-shell");

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("popover-liquid-body");
    body.append(content);
    shell.append(&body);

    let popover = popover.as_ref();
    popover.set_has_arrow(false);
    popover.set_offset(0, 0);
    popover.set_child(Some(&shell));
}

/// Install the common popover motion classes on any `gtk::Popover` subclass
/// (`gtk::PopoverMenu` included). Call once after constructing the popup.
pub fn install_motion(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    popover.add_css_class("popover-animated");

    let popover_for_show = popover.clone();
    popover.connect_show(move |_| {
        popover_for_show.remove_css_class("popover-exiting");
        popover_for_show.remove_css_class("popover-entering");
        let popover = popover_for_show.clone();
        glib::idle_add_local_once(move || {
            popover.add_css_class("popover-entering");
        });
    });

    let popover_for_closed = popover.clone();
    popover.connect_closed(move |_| {
        popover_for_closed.remove_css_class("popover-entering");
        popover_for_closed.remove_css_class("popover-exiting");
    });
}

pub fn popup(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    popover.remove_css_class("popover-exiting");
    popover.popup();
}

pub fn toggle(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    if popover.is_visible() {
        popdown(popover);
    } else {
        popup(popover);
    }
}

pub fn popdown(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    if !popover.is_visible() {
        popover.popdown();
        return;
    }

    popover.remove_css_class("popover-entering");
    popover.add_css_class("popover-exiting");

    let popover = popover.clone();
    glib::timeout_add_local_once(Duration::from_millis(POPOVER_EXIT_MS), move || {
        if popover.has_css_class("popover-exiting") {
            popover.remove_css_class("popover-exiting");
            popover.popdown();
        }
    });
}
