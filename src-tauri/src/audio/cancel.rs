// SPDX-License-Identifier: MPL-2.0
//! Cancelling a playback session: a flag every thread and task of the session reads, and hooks
//! that interrupt what is blocked on the network.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Cancellation flag for a session, with hooks to interrupt a blocked network read
/// (without them, `Stop` would wait for the HTTP read timeout to expire).
#[derive(Default)]
pub(crate) struct Cancel {
    flag: AtomicBool,
    /// Each with its id, so that a [`ScopedHook`] can take its own back.
    hooks: Mutex<Vec<(u64, CancelHook)>>,
    next_hook: AtomicU64,
    /// The flag again, for the tasks that wait for it ([`Cancel::cancelled`]).
    done: tokio::sync::watch::Sender<bool>,
}

impl Cancel {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Returns once the session is cancelled (at once if it already is).
    pub(crate) async fn cancelled(&self) {
        // `self` holds the sender, so the channel cannot close while this waits.
        let _ = self.done.subscribe().wait_for(|&done| done).await;
    }

    pub(super) fn cancel(&self) {
        // The flag is set before taking the lock, so a hook registered after the drain
        // sees it and runs itself: every hook runs exactly once.
        self.flag.store(true, Ordering::SeqCst);
        self.done.send_replace(true);
        let hooks = std::mem::take(&mut *self.hooks.lock().unwrap_or_else(|p| p.into_inner()));
        for (_, hook) in hooks {
            hook();
        }
    }

    /// Adds an action to run on cancellation (immediately if already cancelled), for one attempt
    /// within the session (a connection, a diagnosis): the hook is dropped with the returned
    /// guard, so a session that reconnects for hours does not keep one per attempt. Other hooks
    /// are kept: each one runs.
    pub(crate) fn on_cancel_scoped(
        self: &Arc<Self>,
        hook: impl FnOnce() + Send + 'static,
    ) -> ScopedHook {
        ScopedHook {
            id: self.add(Box::new(hook)),
            cancel: self.clone(),
        }
    }

    /// Keeps the hook, or runs it at once if already cancelled (`None`).
    fn add(&self, hook: CancelHook) -> Option<u64> {
        let mut hooks = self.hooks.lock().unwrap_or_else(|p| p.into_inner());
        if self.is_cancelled() {
            drop(hooks);
            hook();
            return None;
        }
        let id = self.next_hook.fetch_add(1, Ordering::Relaxed);
        hooks.push((id, hook));
        Some(id)
    }
}

type CancelHook = Box<dyn FnOnce() + Send>;

/// A hook of [`Cancel::on_cancel_scoped`], removed when this is dropped (if it has not run).
pub(crate) struct ScopedHook {
    cancel: Arc<Cancel>,
    id: Option<u64>,
}

impl Drop for ScopedHook {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.cancel
                .hooks
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .retain(|(hook, _)| *hook != id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    use super::*;

    #[test]
    fn cancel_runs_hook_once_even_if_registered_late() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));

        let h = hits.clone();
        let _first = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        cancel.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        let h = hits.clone();
        let _late = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn cancel_runs_every_registered_hook() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));
        let hooks: Vec<_> = (0..3)
            .map(|_| {
                let h = hits.clone();
                cancel.on_cancel_scoped(move || {
                    h.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        cancel.cancel();
        assert_eq!(hits.load(Ordering::SeqCst), 3);
        drop(hooks);
    }

    #[test]
    fn an_attempt_takes_its_hook_back_when_it_ends() {
        let cancel = Arc::new(Cancel::default());
        let hits = Arc::new(AtomicU32::new(0));
        for _ in 0..100 {
            let h = hits.clone();
            let _attempt = cancel.on_cancel_scoped(move || {
                h.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert!(cancel.hooks.lock().unwrap().is_empty(), "no hook piles up");
        let h = hits.clone();
        let _current = cancel.on_cancel_scoped(move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        cancel.cancel();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "only the attempt still going runs"
        );
    }

    #[tokio::test]
    async fn cancelled_returns_once_the_session_is_cancelled() {
        let cancel = Arc::new(Cancel::default());
        let waiter = tokio::spawn({
            let cancel = cancel.clone();
            async move { cancel.cancelled().await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("still waiting after the cancel")
            .unwrap();
        // Already cancelled: returns at once.
        tokio::time::timeout(Duration::from_secs(1), cancel.cancelled())
            .await
            .unwrap();
    }
}
