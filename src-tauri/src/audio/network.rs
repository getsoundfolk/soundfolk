// SPDX-License-Identifier: MPL-2.0
//! Watches the network and the system sleep, so a stream cut by either resumes at once instead
//! of waiting for its read timeout.
//!
//! A background thread (`net-watch`) checks every second:
//! * **the route to the Internet**: the local address the system would use to reach a public
//!   IPv4 and IPv6 address (a UDP `connect` only looks up the route, no packet is sent). It
//!   changes when switching Wi-Fi ↔ Ethernet ↔ 4G, joining a VPN, or getting a new DHCP lease;
//!   it disappears when the network is lost.
//! * **a wake from sleep**: the monotonic clock stops while the computer sleeps (Linux, macOS),
//!   the wall clock does not; a gap between the two means the computer was asleep, and that
//!   every connection open before is most likely dead. On Windows the monotonic clock keeps
//!   counting during sleep: the system's resume notification is used instead ([`windows`]).
//!
//! Two system calls per second, **only while a station plays**: the thread parks when nobody
//! listens (no receiver left) and [`subscribe`] wakes it up.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::OnceLock;
use std::thread::{self, Thread};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::watch;

const POLL: Duration = Duration::from_secs(1);
/// Wall-clock time beyond monotonic time that counts as a sleep (NTP adjustments are far
/// smaller, a real sleep far longer).
const SLEEP_GAP: Duration = Duration::from_secs(4);

/// Public addresses whose route is looked up (nothing is ever sent to them).
const PROBE_V4: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53);
const PROBE_V6: SocketAddr =
    SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111)), 53);

/// What the watcher knows. Generations only go up: a change since a connection was opened is
/// seen by comparing them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NetState {
    /// Some route to the Internet exists.
    pub online: bool,
    /// Bumped when the route changes (new interface or address, lost, back).
    pub network: u64,
    /// Bumped when the computer wakes from sleep.
    pub wake: u64,
}

struct Watcher {
    tx: watch::Sender<NetState>,
    thread: Option<Thread>,
}

/// The shared watcher, started on first use. Watches while the receiver is alive.
pub(crate) fn subscribe() -> watch::Receiver<NetState> {
    static WATCH: OnceLock<Watcher> = OnceLock::new();
    let watcher = WATCH.get_or_init(|| {
        let (tx, _) = watch::channel(NetState { online: true, ..Default::default() });
        let sender = tx.clone();
        let thread = thread::Builder::new()
            .name("net-watch".into())
            .spawn(move || watch_loop(sender))
            .inspect_err(|e| tracing::warn!("network watch unavailable: {e}"))
            .ok()
            .map(|handle| handle.thread().clone());
        Watcher { tx, thread }
    });
    // Up to date at once: a playback started offline waits for the network instead of failing.
    let online = routes().online();
    watcher.tx.send_if_modified(|s| std::mem::replace(&mut s.online, online) != online);
    let rx = watcher.tx.subscribe();
    if let Some(thread) = &watcher.thread {
        thread.unpark();
    }
    rx
}

fn watch_loop(tx: watch::Sender<NetState>) {
    windows::watch_resume();
    loop {
        // Nothing playing: no checks at all until the next playback.
        while tx.receiver_count() == 0 {
            thread::park();
        }
        watch_while_listened(&tx);
    }
}

/// Checks every second until nobody listens. Starts from the current state: what changed while
/// nothing played does not matter.
fn watch_while_listened(tx: &watch::Sender<NetState>) {
    let mut detector = Detector::new(routes());
    let (mut wall, mut mono, mut resumes) = (SystemTime::now(), Instant::now(), windows::resumes());
    while tx.receiver_count() > 0 {
        thread::sleep(POLL);
        let (wall_now, mono_now, resumes_now) = (SystemTime::now(), Instant::now(), windows::resumes());
        let slept = wall_now.duration_since(wall).unwrap_or_default();
        let awake = mono_now.duration_since(mono);
        let resumed = resumes_now != resumes;
        (wall, mono, resumes) = (wall_now, mono_now, resumes_now);

        let mut changes = detector.observe(routes(), slept, awake);
        changes.wake |= resumed;
        if changes.network || changes.wake {
            tracing::info!(
                online = detector.routes.online(),
                v4 = ?detector.routes.v4,
                v6 = ?detector.routes.v6,
                woke = changes.wake,
                "network changed"
            );
            tx.send_modify(|s| {
                s.online = detector.routes.online();
                s.network += u64::from(changes.network);
                s.wake += u64::from(changes.wake);
            });
        }
    }
}

/// Local addresses of the default routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Routes {
    v4: Option<IpAddr>,
    v6: Option<IpAddr>,
}

impl Routes {
    fn online(&self) -> bool {
        self.v4.is_some() || self.v6.is_some()
    }
}

fn routes() -> Routes {
    Routes { v4: local_address(PROBE_V4), v6: local_address(PROBE_V6) }
}

fn local_address(target: SocketAddr) -> Option<IpAddr> {
    let bind: SocketAddr = match target {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(target).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// Windows: the monotonic clock (`QueryPerformanceCounter`) keeps counting during sleep, so the
/// clock gap shows nothing. The system calls back on resume instead (no window needed).
#[cfg(windows)]
mod windows {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows_sys::Win32::System::Power::{PowerRegisterSuspendResumeNotification, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
    };

    static RESUMES: AtomicU64 = AtomicU64::new(0);

    /// How many times the computer woke up.
    pub(super) fn resumes() -> u64 {
        RESUMES.load(Ordering::SeqCst)
    }

    unsafe extern "system" fn on_power(_context: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
        // Both are sent on a wake by the user; one is enough to count it.
        if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
            RESUMES.fetch_add(1, Ordering::SeqCst);
        }
        0
    }

    /// Registers for the process lifetime (never unregistered: the watcher never stops).
    pub(super) fn watch_resume() {
        let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS = Box::leak(Box::new(
            DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: std::ptr::null_mut() },
        ));
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: `params` lives forever; `handle` is a valid out pointer.
        let error = unsafe {
            PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, (params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS).cast(), &mut handle)
        };
        if error != 0 {
            tracing::warn!("wake from sleep not watched (error {error})");
        }
    }
}

/// Elsewhere the clock gap is enough.
#[cfg(not(windows))]
mod windows {
    pub(super) fn resumes() -> u64 {
        0
    }

    pub(super) fn watch_resume() {}
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Changes {
    network: bool,
    wake: bool,
}

/// The decision part of the watcher, without clocks or sockets.
struct Detector {
    routes: Routes,
}

impl Detector {
    fn new(routes: Routes) -> Self {
        Self { routes }
    }

    /// `slept` / `awake`: wall-clock and monotonic time since the previous call.
    fn observe(&mut self, routes: Routes, slept: Duration, awake: Duration) -> Changes {
        let wake = slept > awake + SLEEP_GAP;
        // IPv6 privacy addresses rotate: a new IPv6 address alone, with IPv4 unchanged and
        // present, is not a network change (connections on the old address keep working).
        let v6_rotation = routes.v4.is_some()
            && routes.v4 == self.routes.v4
            && routes.v6.is_some()
            && self.routes.v6.is_some();
        let network = routes != self.routes && !v6_rotation;
        self.routes = routes;
        Changes { network, wake }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    const SEC: Duration = Duration::from_secs(1);

    #[test]
    fn interface_switches_and_outages_are_changes() {
        let wifi = Routes { v4: ip("192.168.1.20"), v6: None };
        let lte = Routes { v4: ip("10.64.3.7"), v6: ip("2a01:cb00::7") };
        let mut d = Detector::new(wifi);
        assert_eq!(d.observe(wifi, SEC, SEC), Changes::default());
        assert!(d.observe(lte, SEC, SEC).network, "Wi-Fi → 4G");
        assert!(d.observe(Routes::default(), SEC, SEC).network, "network lost");
        assert!(!d.routes.online());
        assert!(d.observe(wifi, SEC, SEC).network, "network back");
    }

    #[test]
    fn ipv6_address_rotation_alone_is_not_a_change() {
        let before = Routes { v4: ip("192.168.1.20"), v6: ip("2a01:cb00::1") };
        let after = Routes { v4: ip("192.168.1.20"), v6: ip("2a01:cb00::2") };
        let mut d = Detector::new(before);
        assert!(!d.observe(after, SEC, SEC).network);
        // IPv6 only: the address is the route, a new one is a change.
        let mut d = Detector::new(Routes { v4: None, v6: ip("2a01:cb00::1") });
        assert!(d.observe(Routes { v4: None, v6: ip("2a01:cb00::2") }, SEC, SEC).network);
    }

    #[test]
    fn a_wall_clock_jump_is_a_wake() {
        let net = Routes { v4: ip("192.168.1.20"), v6: None };
        let mut d = Detector::new(net);
        assert!(!d.observe(net, Duration::from_millis(1_300), SEC).wake, "a late tick is not a sleep");
        assert!(d.observe(net, Duration::from_secs(3_600), SEC).wake);
    }

    #[test]
    fn the_real_route_can_be_read() {
        // No network in a sandbox is fine: this only checks the probe does not panic or block.
        let _ = routes();
    }
}
