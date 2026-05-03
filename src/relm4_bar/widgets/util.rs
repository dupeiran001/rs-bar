//! Small helpers shared across multiple widgets.
//!
//! [`SuppressGuard`] standardises the "set this flag while I mutate the GTK
//! widget so my own change-signal handler returns early" pattern that
//! several widgets used to spell inline. [`subscribe_into_msg!`] collapses
//! the boilerplate `relm4::spawn_local` block that bridges every hub's
//! `watch::Receiver<T>` into component messages.

use std::cell::RefCell;

/// RAII guard that flips a `RefCell<bool>` flag to `true` for its lifetime.
///
/// Intended use: a widget's `update()` mutates a GTK widget (slider value,
/// toggle state, dropdown selection) in response to a hub publish. Those
/// mutations fire the same signals as user interaction would, so without
/// gating they'd loop back through the hub's command API. A `SuppressGuard`
/// raised at the top of the match arm tells the closure-captured signal
/// handlers to early-return; on drop, the flag flips back to `false`.
pub struct SuppressGuard<'a>(&'a RefCell<bool>);

impl<'a> SuppressGuard<'a> {
    pub fn new(flag: &'a RefCell<bool>) -> Self {
        *flag.borrow_mut() = true;
        SuppressGuard(flag)
    }
}

impl Drop for SuppressGuard<'_> {
    fn drop(&mut self) {
        *self.0.borrow_mut() = false;
    }
}

/// Bridge a `tokio::sync::watch::Receiver<T: Clone>` into a relm4 component's
/// input messages.
///
/// Sends the *current* value immediately (so the widget renders something
/// before the next publish), then forwards every subsequent
/// `changed()`-wake. The `$variant` argument is any expression that
/// converts a `T` into the component's input message — typically a tuple
/// enum constructor like `MyMsg::Update`.
///
/// ```ignore
/// let rx = hub::cpu_freq::subscribe();
/// subscribe_into_msg!(rx, sender, CpuFreqMsg::Update);
/// ```
#[macro_export]
macro_rules! subscribe_into_msg {
    ($rx:expr, $sender:expr, $variant:expr) => {{
        let mut __sub_rx = $rx;
        let __sub_sender = $sender.clone();
        relm4::spawn_local(async move {
            let initial = __sub_rx.borrow_and_update().clone();
            __sub_sender.input($variant(initial));
            while __sub_rx.changed().await.is_ok() {
                let v = __sub_rx.borrow_and_update().clone();
                __sub_sender.input($variant(v));
            }
        });
    }};
}
