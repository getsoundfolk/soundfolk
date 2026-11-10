// SPDX-License-Identifier: MPL-2.0
//! A list published on the web that the app keeps a copy of: downloaded again when the copy is a day
//! old, with `If-None-Match` (an unchanged list costs a `304`), and kept for offline use. Used by the
//! shared recipes (`recipe_list.rs`) and the station data of `soundfolk-stations` (`donations.rs`).
//!
//! A list that does not parse is refused and the cached one stays: an `ETag` never describes a list
//! that was not saved.

use std::path::Path;
use std::time::{Duration, SystemTime};

use reqwest::{header, Client, StatusCode};

/// How old the cached list may get before it is downloaded again.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// How often to check whether the list is due.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// First check after startup: lets the first station start without competing for the network.
pub const FIRST_CHECK: Duration = Duration::from_secs(10);
/// A larger answer is not one of our lists.
const MAX_SIZE: usize = 8 * 1024 * 1024;

/// Where a list is cached: `<dir>/<list>`, and its `ETag` in `<dir>/<etag>`, whose modification time
/// is when the list was last checked.
#[derive(Debug, Clone, Copy)]
pub struct Files {
    pub list: &'static str,
    pub etag: &'static str,
}

/// The cached list, if there is one and it parses.
pub fn read_cached<T>(
    dir: &Path,
    files: Files,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Option<T> {
    let text = std::fs::read_to_string(dir.join(files.list)).ok()?;
    parse(&text)
        .inspect_err(|e| tracing::warn!("cached {}: {e}", files.list))
        .ok()
}

/// Whether the list should be downloaded again: never downloaded, or checked more than
/// [`MAX_AGE`] ago.
pub fn is_due(dir: &Path, files: Files) -> bool {
    let checked = std::fs::metadata(dir.join(files.etag))
        .and_then(|m| m.modified())
        .ok();
    let cached = dir.join(files.list).is_file();
    !cached
        || checked
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_none_or(|age| age >= MAX_AGE)
}

/// Downloads the list if it changed, and caches it once it parses. `Ok(None)`: unchanged.
pub async fn refresh<T>(
    client: &Client,
    url: &str,
    dir: &Path,
    files: Files,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let etag_path = dir.join(files.etag);
    let etag = std::fs::read_to_string(&etag_path)
        .ok()
        .filter(|_| dir.join(files.list).is_file());
    let mut request = client.get(url);
    if let Some(etag) = etag.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED {
        // Rewritten to note when the list was checked.
        std::fs::write(&etag_path, etag.unwrap_or_default()).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("{url}: HTTP {status}"));
    }
    let new_etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = crate::http::body_at_most(response, MAX_SIZE)
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let text = std::str::from_utf8(&body).map_err(|e| format!("{url}: {e}"))?;
    let list = parse(text).map_err(|e| format!("{url}: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // The list first: an ETag must never describe a list that was not saved.
    let tmp = dir.join(format!("{}.tmp", files.list));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, dir.join(files.list)))
        .map_err(|e| e.to_string())?;
    std::fs::write(&etag_path, new_etag.unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(Some(list))
}
