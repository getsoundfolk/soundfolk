// SPDX-License-Identifier: MPL-2.0
//! What the operating system says about Internet access. It checks it itself (Windows NCSI,
//! NetworkManager's connectivity check), so asking costs no request from Soundfolk.
//!
//! Only used as a **second opinion**: "no Internet access" is shown when the system says so *and*
//! Soundfolk's own check fails too (see [`super::failure`]). The system alone can be wrong: a
//! network NetworkManager does not manage, a corporate proxy that fools Windows' check.
//!
//! * **Linux**: the desktop portal (`org.freedesktop.portal.NetworkMonitor`, also inside
//!   Flatpak), else NetworkManager on the system bus. Without NetworkManager, or with its
//!   connectivity check turned off, the answer is meaningless: nothing is concluded.
//! * **Windows** 10 2004 and later: `GetNetworkConnectivityHint`, looked up at run time (a static
//!   import would stop the app from starting on older versions).
//! * **macOS**: no system API checks Internet access (only that a route exists): nothing.

/// What the system concludes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Connectivity {
    /// No Internet access (local network only, or limited).
    NoInternet,
    /// A captive portal: the network wants the user to sign in.
    Portal,
    Full,
}

/// `None`: the system does not say, or cannot know.
pub(crate) fn system() -> Option<Connectivity> {
    let answer = platform::connectivity();
    tracing::debug!(?answer, "system connectivity");
    answer
}

#[cfg(target_os = "linux")]
mod platform {
    use std::time::Duration;

    use zbus::blocking::connection::Builder;
    use zbus::zvariant::OwnedValue;

    use super::Connectivity;

    /// The bus answers at once; a portal being started may take a moment.
    const TIMEOUT: Duration = Duration::from_secs(2);

    pub(super) fn connectivity() -> Option<Connectivity> {
        portal().or_else(network_manager)
    }

    /// `GetConnectivity`: 1 local only, 2 limited, 3 captive portal, 4 full.
    fn portal() -> Option<Connectivity> {
        let bus = Builder::session()
            .ok()?
            .method_timeout(TIMEOUT)
            .build()
            .ok()?;
        let reply = bus
            .call_method(
                Some("org.freedesktop.portal.Desktop"),
                "/org/freedesktop/portal/desktop",
                Some("org.freedesktop.portal.NetworkMonitor"),
                "GetConnectivity",
                &(),
            )
            .ok()?;
        match reply.body().deserialize::<u32>().ok()? {
            1 | 2 => Some(Connectivity::NoInternet),
            3 => Some(Connectivity::Portal),
            4 => Some(Connectivity::Full),
            _ => None,
        }
    }

    /// `Connectivity`: 0 unknown, 1 none, 2 portal, 3 limited, 4 full.
    fn network_manager() -> Option<Connectivity> {
        let bus = Builder::system()
            .ok()?
            .method_timeout(TIMEOUT)
            .build()
            .ok()?;
        let reply = bus
            .call_method(
                Some("org.freedesktop.NetworkManager"),
                "/org/freedesktop/NetworkManager",
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.freedesktop.NetworkManager", "Connectivity"),
            )
            .ok()?;
        let value: OwnedValue = reply.body().deserialize().ok()?;
        match u32::try_from(value).ok()? {
            1 | 3 => Some(Connectivity::NoInternet),
            2 => Some(Connectivity::Portal),
            4 => Some(Connectivity::Full),
            _ => None,
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::sync::OnceLock;

    use windows_sys::Win32::Networking::WinSock::{
        NetworkConnectivityLevelHintConstrainedInternetAccess,
        NetworkConnectivityLevelHintInternetAccess, NetworkConnectivityLevelHintLocalAccess,
        NetworkConnectivityLevelHintNone, NL_NETWORK_CONNECTIVITY_HINT,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };

    use super::Connectivity;

    type GetHint = unsafe extern "system" fn(*mut NL_NETWORK_CONNECTIVITY_HINT) -> u32;

    /// `GetNetworkConnectivityHint`, if this Windows has it.
    fn get_hint() -> Option<GetHint> {
        static FUNCTION: OnceLock<Option<GetHint>> = OnceLock::new();
        *FUNCTION.get_or_init(|| {
            let dll: Vec<u16> = "iphlpapi.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated name, loaded from System32 only.
            let module = unsafe {
                LoadLibraryExW(
                    dll.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            };
            if module.is_null() {
                return None;
            }
            // SAFETY: a NUL-terminated name; the module stays loaded for the process lifetime.
            let function =
                unsafe { GetProcAddress(module, c"GetNetworkConnectivityHint".as_ptr().cast()) }?;
            // SAFETY: the documented signature of `GetNetworkConnectivityHint`.
            Some(unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, GetHint>(function)
            })
        })
    }

    pub(super) fn connectivity() -> Option<Connectivity> {
        let get_hint = get_hint()?;
        let mut hint = NL_NETWORK_CONNECTIVITY_HINT::default();
        // SAFETY: `hint` is a valid, writable struct of the expected type.
        if unsafe { get_hint(&mut hint) } != 0 {
            return None;
        }
        #[allow(non_upper_case_globals)]
        match hint.ConnectivityLevel {
            NetworkConnectivityLevelHintNone | NetworkConnectivityLevelHintLocalAccess => {
                Some(Connectivity::NoInternet)
            }
            // "Restricted access, e.g. a captive portal": not certainly a portal.
            NetworkConnectivityLevelHintConstrainedInternetAccess => Some(Connectivity::NoInternet),
            NetworkConnectivityLevelHintInternetAccess => Some(Connectivity::Full),
            _ => None,
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    pub(super) fn connectivity() -> Option<super::Connectivity> {
        None
    }
}

#[cfg(test)]
mod tests {
    /// Whatever the machine says, asking must neither panic nor hang.
    #[test]
    fn the_system_answer_can_be_read() {
        let started = std::time::Instant::now();
        println!("system connectivity: {:?}", super::system());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
