// SPDX-License-Identifier: MPL-2.0
//! The shared recipe list: every recipe of `getsoundfolk/soundfolk-recipes`, compiled into one
//! `recipes.json` by that repository's CI and published on GitHub Pages. See
//! `docs/07-now-playing.md` and the repository's `ARCHITECTURE.md`.
//!
//! * At startup, the copy cached by the last download is used right away (offline included).
//! * The list is downloaded again when that copy is older than [`MAX_AGE`], with `If-None-Match`:
//!   an unchanged list costs a `304`. Only while web titles are on (⚙ › Privacy): GitHub sees the
//!   listener's IP address, like the station sites do.
//! * A list that does not parse is ignored and the cached one stays; entries this version cannot
//!   read are skipped one by one (see [`soundfolk_nowplaying::check::load_compiled`]).
//!
//! `SOUNDFOLK_RECIPES_URL` replaces the list's address (a fork's list, a local file server).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use soundfolk_nowplaying::check::load_compiled;
use soundfolk_nowplaying::recipe::Recipe;
use stream_download::http::reqwest::{header, Client, StatusCode};

use crate::audio::WebTitles;

/// Where the list is published. `v1` is the list format, not the recipe schema: a breaking change
/// of the list moves it, and older apps keep reading the old one.
pub const DEFAULT_URL: &str = "https://getsoundfolk.github.io/soundfolk-recipes/v1/recipes.json";
/// How old the cached list may get before it is downloaded again.
const MAX_AGE: Duration = Duration::from_secs(24 * 3600);
/// How often to check whether the list is due (and whether web titles were turned on).
const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// First check after startup: lets the first station start without competing for the network.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(30);
/// A larger answer is not a recipe list.
const MAX_SIZE: usize = 8 * 1024 * 1024;

const LIST_FILE: &str = "recipes.json";
/// The `ETag` of the cached list; its modification time is when the list was last checked.
const ETAG_FILE: &str = "recipes.etag";

/// Loads the cached list, then keeps it up to date in the background.
pub fn start(web: Arc<WebTitles>, cache_dir: Option<PathBuf>) {
    let Some(dir) = cache_dir else { return };
    if let Some(recipes) = read_cached(&dir) {
        tracing::info!("shared recipes: {} from the cache", recipes.len());
        web.recipes().set_shared(recipes);
    }
    let url = std::env::var("SOUNDFOLK_RECIPES_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    tauri::async_runtime::spawn(async move {
        let client = match Client::builder()
            .user_agent(concat!(
                "Soundfolk/",
                env!("CARGO_PKG_VERSION"),
                " (+https://github.com/getsoundfolk/soundfolk)"
            ))
            .timeout(TIMEOUT)
            .build()
        {
            Ok(client) => client,
            Err(e) => return tracing::warn!("shared recipes unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if web.is_enabled() && is_due(&dir) {
                match refresh(&client, &url, &dir).await {
                    Ok(Some(recipes)) => {
                        tracing::info!("shared recipes: {} downloaded", recipes.len());
                        web.recipes().set_shared(recipes);
                    }
                    Ok(None) => tracing::debug!("shared recipes: unchanged"),
                    Err(e) => tracing::warn!("shared recipes: {e}"),
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

fn read_cached(dir: &Path) -> Option<Vec<(String, Recipe)>> {
    let text = std::fs::read_to_string(dir.join(LIST_FILE)).ok()?;
    load_compiled(&text)
        .inspect_err(|e| tracing::warn!("cached shared recipes: {e}"))
        .ok()
}

fn is_due(dir: &Path) -> bool {
    let checked = std::fs::metadata(dir.join(ETAG_FILE))
        .and_then(|m| m.modified())
        .ok();
    let cached = dir.join(LIST_FILE).is_file();
    !cached
        || checked
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_none_or(|age| age >= MAX_AGE)
}

/// Downloads the list if it changed. `Ok(None)`: unchanged.
async fn refresh(
    client: &Client,
    url: &str,
    dir: &Path,
) -> Result<Option<Vec<(String, Recipe)>>, String> {
    let etag_path = dir.join(ETAG_FILE);
    let etag = std::fs::read_to_string(&etag_path)
        .ok()
        .filter(|_| dir.join(LIST_FILE).is_file());
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
    if response
        .content_length()
        .is_some_and(|n| n > MAX_SIZE as u64)
    {
        return Err(format!("{url}: too large"));
    }
    let body = response.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_SIZE {
        return Err(format!("{url}: too large"));
    }
    let text = std::str::from_utf8(&body).map_err(|e| format!("{url}: {e}"))?;
    let recipes = load_compiled(text).map_err(|e| format!("{url}: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    // The list first: an ETag must never describe a list that was not saved.
    let tmp = dir.join(format!("{LIST_FILE}.tmp"));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, dir.join(LIST_FILE)))
        .map_err(|e| e.to_string())?;
    std::fs::write(&etag_path, new_etag.unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(Some(recipes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "soundfolk-recipe-list-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_list_is_due_until_it_was_checked_recently() {
        let dir = temp_dir("due");
        assert!(is_due(&dir));
        std::fs::write(dir.join(LIST_FILE), r#"{"schema":1,"recipes":[]}"#).unwrap();
        assert!(is_due(&dir), "never checked");
        std::fs::write(dir.join(ETAG_FILE), "\"abc\"").unwrap();
        assert!(!is_due(&dir));
        assert_eq!(read_cached(&dir).unwrap().len(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Serves `responses` in order, one per connection; returns the address and the requests.
    fn serve(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/recipes.json", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).unwrap();
                    stream.write_all(response.as_bytes()).unwrap();
                    String::from_utf8_lossy(&buf[..n]).to_lowercase()
                })
                .collect()
        });
        (url, handle)
    }

    #[tokio::test]
    async fn downloads_then_revalidates_with_the_etag() {
        let dir = temp_dir("refresh");
        let recipe = soundfolk_nowplaying::recipe::Recipe::from_toml(include_str!(
            "../recipes/stations/frequence7.toml"
        ))
        .unwrap();
        let list =
            soundfolk_nowplaying::check::compile(&[("frequence7".into(), recipe)]).to_string();
        let (url, server) = serve(vec![
            format!("HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{list}", list.len()),
            "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".into(),
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n<html>".into(),
        ]);
        let client = Client::new();
        let first = refresh(&client, &url, &dir).await.unwrap().unwrap();
        assert_eq!(first[0].0, "frequence7");
        assert!(!is_due(&dir));
        assert_eq!(refresh(&client, &url, &dir).await.unwrap(), None);
        // Not a list: refused, the cached one stays.
        assert!(refresh(&client, &url, &dir).await.is_err());
        assert_eq!(read_cached(&dir).unwrap().len(), 1);
        let requests = server.join().unwrap();
        assert!(!requests[0].contains("if-none-match"));
        assert!(
            requests[0].contains("accept-encoding: gzip"),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].contains("if-none-match: \"v1\""),
            "{}",
            requests[1]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_cache_is_ignored() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join(LIST_FILE), "<html>").unwrap();
        assert!(read_cached(&dir).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
