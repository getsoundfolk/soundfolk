// SPDX-License-Identifier: MPL-2.0
//! The shared recipe list: every recipe of `getsoundfolk/soundfolk-recipes`, compiled into one
//! `recipes.json` by that repository's CI and published on GitHub Pages. See
//! `docs/08-now-playing.md` and the repository's `ARCHITECTURE.md`.
//!
//! * At startup, the copy cached by the last download is used right away (offline included).
//! * The list is downloaded again when that copy is a day old, with `If-None-Match`: an unchanged
//!   list costs a `304` (see `daily_list.rs`). Only while web titles are on (⚙ › Privacy): GitHub
//!   sees the listener's IP address, like the station sites do.
//! * A list that does not parse is ignored and the cached one stays; entries this version cannot
//!   read are skipped one by one (see [`soundfolk_nowplaying::check::load_compiled`]).
//!
//! `SOUNDFOLK_RECIPES_URL` replaces the list's address (a fork's list, a local file server).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::Client;
use soundfolk_nowplaying::check::load_compiled;
use soundfolk_nowplaying::recipe::Recipe;

use crate::daily_list::{self, Files, CHECK_EVERY, FIRST_CHECK};
use crate::now_playing::WebTitles;

/// Where the list is published. `v1` is the list format, not the recipe schema: a breaking change
/// of the list moves it, and older apps keep reading the old one.
pub const DEFAULT_URL: &str = "https://getsoundfolk.github.io/soundfolk-recipes/v1/recipes.json";

const FILES: Files = Files {
    list: "recipes.json",
    // Its modification time is when the list was last checked.
    etag: "recipes.etag",
};
#[cfg(test)]
const LIST_FILE: &str = FILES.list;
#[cfg(test)]
const ETAG_FILE: &str = FILES.etag;

/// Loads the cached list, then keeps it up to date in the background.
pub fn start(web: Arc<WebTitles>, cache_dir: Option<PathBuf>) {
    let Some(dir) = cache_dir else { return };
    let url = std::env::var("SOUNDFOLK_RECIPES_URL").unwrap_or_else(|_| DEFAULT_URL.to_owned());
    tauri::async_runtime::spawn(async move {
        // Parsed off the main thread, so that the first window does not wait for it
        let cached = {
            let dir = dir.clone();
            crate::state::spawn_blocking(move || read_cached(&dir)).await
        };
        if let Ok(Some(recipes)) = cached {
            tracing::info!("shared recipes: {} from the cache", recipes.len());
            web.recipes().set_shared(recipes);
        }
        let client = match crate::http::client() {
            Ok(client) => client,
            Err(e) => return tracing::warn!("shared recipes unavailable: {e}"),
        };
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if web.is_enabled() && is_due(&dir) {
                match refresh(client, &url, &dir).await {
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

fn parse(text: &str) -> Result<Vec<(String, Recipe)>, String> {
    load_compiled(text).map_err(|e| e.to_string())
}

fn read_cached(dir: &Path) -> Option<Vec<(String, Recipe)>> {
    daily_list::read_cached(dir, FILES, parse)
}

fn is_due(dir: &Path) -> bool {
    daily_list::is_due(dir, FILES)
}

/// Downloads the list if it changed. `Ok(None)`: unchanged.
async fn refresh(
    client: &Client,
    url: &str,
    dir: &Path,
) -> Result<Option<Vec<(String, Recipe)>>, String> {
    daily_list::refresh(client, url, dir, FILES, parse).await
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
