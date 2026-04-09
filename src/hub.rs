//! Cross-bar broadcast hub for shared widget data sources.
//!
//! Each polling widget that shows **system-global** state (CPU usage, memory,
//! temperature, power draw, niri events, etc.) publishes through a single
//! producer. Multiple bar instances (one per monitor) subscribe to the same
//! producer, so the work of reading sysfs / listening to an event stream
//! happens exactly **once** no matter how many bars are open.
//!
//! ## Design
//!
//! - [`Broadcast<T>`] is a cheap-to-clone handle (`Arc`) holding the latest
//!   value and a list of per-subscriber notifier channels.
//! - `publish(value)` stores the value under a `Mutex` and fires `try_send(())`
//!   on every live subscriber's 1-slot notification channel. If a
//!   notification is already pending we drop the new one — the subscriber
//!   reads the *latest* stored value on wake, so nothing is lost; only
//!   redundant wakes are coalesced.
//! - `subscribe()` returns a [`Subscription<T>`] whose `.next().await` parks
//!   until a publish happens, then returns a clone of the latest value.
//!   The newest value is primed at subscription time so subscribers get the
//!   current state immediately on their first `.next().await`.
//!
//! ## Usage
//!
//! ```ignore
//! // At app init (main.rs, before any bar is built):
//! hub::cpu_usage();  // forces the singleton poller thread to spawn
//!
//! // In a widget's new():
//! let sub = hub::cpu_usage().subscribe();
//! cx.spawn(async move |this, cx| {
//!     while let Some(state) = sub.next().await {
//!         if this.update(cx, |this, cx| { this.state = state; cx.notify(); }).is_err() {
//!             break;
//!         }
//!     }
//! }).detach();
//! ```
//!
//! The singleton helpers (e.g. [`cpu_usage`]) are defined by individual widget
//! modules — this file only provides the generic [`Broadcast`] / [`Subscription`]
//! primitive.

use std::sync::{Arc, Mutex};

/// One-slot notification channel used internally by [`Broadcast`].
type Notifier = async_channel::Sender<()>;

struct Inner<T> {
    latest: Option<T>,
    subs: Vec<Notifier>,
}

/// A multi-subscriber broadcast handle for a latest-value data source.
///
/// `T` is the value type. Subscribers always see the most recently published
/// value; intermediate values may be skipped if a subscriber is slow.
pub struct Broadcast<T> {
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T> Clone for Broadcast<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: Clone> Broadcast<T> {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                latest: None,
                subs: Vec::new(),
            })),
        }
    }

    /// Store `value` and wake every subscriber.
    ///
    /// Dead subscribers (closed receivers) are reaped on publish.
    /// Subscribers whose notifier slot is already full are left alone — they
    /// will see `value` on their pending wake since we stored it first.
    pub fn publish(&self, value: T) {
        let mut inner = self.inner.lock().unwrap();
        inner.latest = Some(value);
        inner.subs.retain(|s| !s.is_closed());
        for s in &inner.subs {
            // Ignore Full: subscriber already has a pending wake; our new value
            // will be read on that wake. Ignore Closed: caught by retain above
            // on the next publish.
            let _ = s.try_send(());
        }
    }

    /// Create a new subscription. If a value has been published already, the
    /// returned subscription is primed so the first `.next().await` returns
    /// immediately with that value.
    pub fn subscribe(&self) -> Subscription<T> {
        let (tx, rx) = async_channel::bounded::<()>(1);
        let mut inner = self.inner.lock().unwrap();
        if inner.latest.is_some() {
            let _ = tx.try_send(());
        }
        inner.subs.push(tx);
        Subscription {
            rx,
            inner: self.inner.clone(),
        }
    }
}

/// Handle held by a subscriber. Drop to unsubscribe (the notifier will be
/// reaped on the next publish).
pub struct Subscription<T> {
    rx: async_channel::Receiver<()>,
    inner: Arc<Mutex<Inner<T>>>,
}

impl<T: Clone> Subscription<T> {
    /// Park until the next publish (or return `None` if the producer side
    /// has been dropped). On wake, returns a clone of the latest value.
    pub async fn next(&self) -> Option<T> {
        self.rx.recv().await.ok()?;
        self.inner.lock().unwrap().latest.clone()
    }
}
