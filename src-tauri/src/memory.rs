// SPDX-License-Identifier: MPL-2.0
//! Memory given back once no window is left (docs/02-architecture.md §6).
//!
//! Closing the last window frees its web process, but on Linux WebKit keeps its network process
//! (~12 MB) for the next web view. A while after the last window is gone, it is asked to quit
//! (WebKit starts a new one with the next web view) and the free part of the heap goes back to the
//! system. Elsewhere, nothing is done.

use std::time::Duration;

use tauri::Manager;

/// Time left for the network process to finish what the web view asked it (a `localStorage`
/// write, for instance) and for the user to open the window again.
const DELAY: Duration = Duration::from_secs(10);

/// Called when a window is destroyed: if none is left a while later, memory is given back.
pub fn after_window_closed(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DELAY).await;
        if !app.webview_windows().is_empty() {
            return;
        }
        crate::state::spawn_blocking(give_back);
    });
}

#[cfg(target_os = "linux")]
fn give_back() {
    for pid in children_named("WebKitNetworkPr") {
        // SAFETY: `kill` takes no pointer. The pid was read from /proc a moment ago as a child of
        // this process: GLib reaping it and the system giving the pid to another process within
        // that moment is not a practical concern.
        if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
            let error = std::io::Error::last_os_error();
            tracing::warn!("WebKit network process {pid} not stopped: {error}");
        }
    }
    #[cfg(target_env = "gnu")]
    // SAFETY: `malloc_trim` only gives free heap pages back to the system; it is thread-safe.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(target_os = "linux"))]
fn give_back() {}

/// The pids of this process's children whose name (`comm`, cut at 15 bytes) is `name`.
#[cfg(target_os = "linux")]
fn children_named(name: &str) -> Vec<libc::pid_t> {
    let me = std::process::id().to_string();
    let entries = match std::fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("processes not listed: {e}");
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid = entry.file_name().to_str()?.parse::<libc::pid_t>().ok()?;
            // A process may end while the list is read: it is simply skipped.
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let (comm, ppid) = parse_stat(&stat)?;
            (comm == name && ppid == me).then_some(pid)
        })
        .collect()
}

/// `comm` and parent pid from a `/proc/<pid>/stat` line: `pid (comm) state ppid …`, where `comm`
/// may itself hold spaces and parentheses.
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<(&str, &str)> {
    let (head, tail) = stat.rsplit_once(')')?;
    let (_, comm) = head.split_once('(')?;
    let mut fields = tail.split_whitespace();
    let _state = fields.next()?;
    Some((comm, fields.next()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines_give_name_and_parent() {
        let line = "4242 (WebKitNetworkPr) S 4100 4100 2959 0 -1 4194560 1 0 0 0";
        assert_eq!(parse_stat(line), Some(("WebKitNetworkPr", "4100")));
        assert_eq!(parse_stat("7 (a) b (c)) R 1 7"), Some(("a) b (c)", "1")));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_child_is_found_by_name() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let found = children_named("sleep");
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(found, vec![child.id() as libc::pid_t]);
    }
}
