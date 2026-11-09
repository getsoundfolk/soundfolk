// SPDX-License-Identifier: MPL-2.0
//! Drops a connection that stops receiving after a network change or a wake from sleep, instead
//! of waiting for the read timeout.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use super::super::network::NetState;
use super::Interrupt;

/// After a network change or a wake from sleep, a connection that receives nothing for this long
/// is dead (a live stream delivers several times per second). One that keeps receiving is kept:
/// switching Ethernet → Wi-Fi with both up does not cut the sound.
const STALL_AFTER_CHANGE: Duration = Duration::from_secs(3);

/// Why the watchdog dropped the connection, read once the decoder sees the stream end.
pub(super) type Fired = Arc<Mutex<Option<Interrupt>>>;

/// What the watchdog needs: the network state, and where to say why it fired.
pub(super) struct NetGuard {
    pub(super) net: watch::Receiver<NetState>,
    pub(super) fired: Fired,
}

/// When the connection last received data.
pub(super) struct Activity {
    base: Instant,
    last_ms: AtomicU64,
}

impl Activity {
    pub(super) fn new() -> Self {
        Self {
            base: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    pub(super) fn touch(&self) {
        self.last_ms
            .store(self.base.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle(&self) -> Duration {
        self.base
            .elapsed()
            .saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// Task that drops the connection when it stops receiving after a network change or a sleep.
/// Ends with the stream (`Drop`).
pub(super) struct Watchdog(tokio::task::JoinHandle<()>);

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Watchdog {
    pub(super) fn spawn(
        runtime: &tokio::runtime::Handle,
        guard: &NetGuard,
        opened_at: NetState,
        activity: Arc<Activity>,
        stop: impl FnOnce() + Send + 'static,
    ) -> Self {
        let net = guard.net.clone();
        let fired = guard.fired.clone();
        Self(runtime.spawn(async move {
            let mut net = net;
            // Nothing to check before a change; returns at once if one came since the opening.
            let changed =
                |now: &NetState| now.wake != opened_at.wake || now.network != opened_at.network;
            if net.wait_for(changed).await.is_err() {
                return; // the network is no longer watched
            }
            // Then the stream has until STALL_AFTER_CHANGE without data: wait exactly that long
            // after the last data received, again and again while data keeps coming.
            loop {
                let idle = activity.idle();
                if idle < STALL_AFTER_CHANGE {
                    tokio::time::sleep(STALL_AFTER_CHANGE - idle).await;
                    continue;
                }
                let now = *net.borrow();
                let why = if now.wake != opened_at.wake {
                    Interrupt::Wake
                } else {
                    Interrupt::Network
                };
                tracing::info!("{why}, and the stream stopped receiving: reconnecting");
                *fired.lock().unwrap_or_else(|p| p.into_inner()) = Some(why);
                stop();
                return;
            }
        }))
    }
}
