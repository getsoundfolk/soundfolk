// SPDX-License-Identifier: MPL-2.0
//! Station logos, kept in the cache folder: a station played before shows its logo at once, with
//! no network, and the logo is fetched again only once a month (stations seldom change it).
//!
//! The window gets the logo inline (`data:` address), the desktop's media controls as a file.
//! Only images are kept: an error page served instead of the favicon is dropped.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use base64::Engine;
use reqwest::{Client, Url};

use crate::http::public_net;

/// Bigger than this, a favicon is not a logo worth keeping.
const MAX_BYTES: usize = 1024 * 1024;
/// A kept logo older than this is fetched again (and still shown meanwhile).
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// A logo not fetched again for this long belongs to a station not played for two months at
/// least: it is deleted at startup, so the folder does not grow with every station ever tried.
const FORGOTTEN_AFTER: Duration = Duration::from_secs(3 * 30 * 24 * 3600);
/// A half-written download this old was cut short.
const PARTIAL_AFTER: Duration = Duration::from_secs(3600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Image formats kept, as found by their first bytes: (extension, media type).
const FORMATS: [(&str, &str); 7] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
];

/// A station's logo, ready to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Logo {
    /// `data:` address, for the window.
    pub inline: String,
    /// `file://` address, for the media controls.
    pub file: Option<String>,
}

/// The format of an image, from its first bytes; `None` when it is not an image kept.
fn format_of(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    let index = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        0
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        1
    } else if bytes.starts_with(b"GIF8") {
        2
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        3
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        4
    } else if bytes.starts_with(b"BM") {
        5
    } else if is_svg(bytes) {
        6
    } else {
        return None;
    };
    Some(FORMATS[index])
}

/// An SVG document: its `<svg` tag near the top, after an optional XML prolog.
fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    (text.starts_with("<?xml") || text.starts_with("<svg") || text.starts_with("<!--"))
        && text.contains("<svg")
}

/// A catalog ID usable as a file name (Radio-Browser UUIDs).
fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Deletes the logos not fetched again for [`FORGOTTEN_AFTER`], and downloads left half-written.
fn prune(dir: &std::path::Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| now.duration_since(at).ok());
        // A `.part` is a download being written, or one an exit cut short: only an old one goes.
        let partial = path.extension().is_some_and(|ext| ext == "part");
        let limit = if partial {
            PARTIAL_AFTER
        } else {
            FORGOTTEN_AFTER
        };
        if age.is_some_and(|age| age > limit) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::debug!(removed, "old station logos deleted");
    }
}

fn logo_of(bytes: &[u8], media_type: &str, file: Option<String>) -> Logo {
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    Logo {
        inline: format!("data:{media_type};base64,{data}"),
        file,
    }
}

#[derive(Clone)]
pub struct LogoCache {
    /// `None`: no cache folder (logos are then fetched each launch).
    dir: Option<PathBuf>,
    client: Option<Client>,
}

impl LogoCache {
    pub fn new(dir: Option<PathBuf>) -> Self {
        // Favicons come from Radio-Browser and covers from station pages: both stay on the public
        // Internet, as catalog streams do (docs/16-sharing.md §6.2).
        let client = crate::http::public_client()
            .inspect_err(|e| tracing::warn!("no HTTP client for station logos: {e}"))
            .ok()
            .cloned();
        if let Some(dir) = dir.clone() {
            let pruned = std::thread::Builder::new()
                .name("logo-pruner".into())
                .spawn(move || prune(&dir, SystemTime::now()));
            if let Err(e) = pruned {
                tracing::debug!("old station logos not pruned: {e}");
            }
        }
        Self { dir, client }
    }

    /// Path of a station's logo in a given format.
    fn path(&self, id: &str, ext: &str) -> Option<PathBuf> {
        let dir = self.dir.as_ref().filter(|_| is_safe_id(id))?;
        Some(dir.join(format!("{id}.{ext}")))
    }

    /// The logo kept for a station, and whether it is due to be fetched again.
    pub fn cached(&self, id: &str) -> Option<(Logo, bool)> {
        FORMATS.iter().find_map(|(ext, _)| {
            let path = self.path(id, ext)?;
            let bytes = std::fs::read(&path).ok()?;
            let (_, media_type) = format_of(&bytes)?;
            let age = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok());
            let stale = age.is_none_or(|age| age > MAX_AGE);
            let file = Url::from_file_path(&path).ok().map(String::from);
            Some((logo_of(&bytes, media_type, file), stale))
        })
    }

    /// Downloads a logo and keeps it for the station (written from a blocking thread).
    pub async fn fetch(&self, id: &str, url: &str) -> Result<Logo, String> {
        let (bytes, ext, media_type) = self.download(url, MAX_BYTES).await?;
        let (cache, id) = (self.clone(), id.to_owned());
        tokio::task::spawn_blocking(move || {
            let file = cache.store(&id, ext, &bytes);
            logo_of(&bytes, media_type, file)
        })
        .await
        .map_err(|e| format!("logo not kept: {e}"))
    }

    /// Downloads an image of at most `max` bytes: its bytes, extension and media type.
    pub async fn download(
        &self,
        url: &str,
        max: usize,
    ) -> Result<(Vec<u8>, &'static str, &'static str), String> {
        // The client checks names once resolved, and redirects; an IP in the address is never
        // resolved, so it is checked here.
        let url = Url::parse(url).map_err(|e| e.to_string())?;
        public_net::check(&url)?;
        self.fetch_unchecked(url, max).await
    }

    /// [`LogoCache::download`] once the address is checked.
    async fn fetch_unchecked(
        &self,
        url: Url,
        max: usize,
    ) -> Result<(Vec<u8>, &'static str, &'static str), String> {
        let client = self.client.as_ref().ok_or("no HTTP client")?;
        let response = client
            .get(url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let bytes = crate::http::body_at_most(response, max).await?;
        let (ext, media_type) = format_of(&bytes).ok_or("not an image")?;
        Ok((bytes, ext, media_type))
    }

    /// Writes a logo to the cache folder (replacing one of another format); its `file://` address.
    fn store(&self, id: &str, ext: &str, bytes: &[u8]) -> Option<String> {
        let path = self.path(id, ext)?;
        let tmp = path.with_extension("part");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, bytes))
            .and_then(|()| std::fs::rename(&tmp, &path));
        if let Err(e) = written {
            tracing::debug!("station logo not kept: {e}");
            let _ = std::fs::remove_file(&tmp);
            return None;
        }
        self.forget_except(id, Some(ext));
        Url::from_file_path(&path).ok().map(String::from)
    }

    /// The station has no logo any more: the kept one goes.
    pub fn forget(&self, id: &str) {
        self.forget_except(id, None);
    }

    fn forget_except(&self, id: &str, keep: Option<&str>) {
        for (ext, _) in FORMATS.iter().filter(|(ext, _)| Some(*ext) != keep) {
            if let Some(path) = self.path(id, ext) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn only_images_are_kept() {
        assert_eq!(format_of(PNG), Some(("png", "image/png")));
        assert_eq!(format_of(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap().0, "jpg");
        assert_eq!(format_of(b"RIFF\0\0\0\0WEBPVP8 ").unwrap().0, "webp");
        assert_eq!(format_of(&[0, 0, 1, 0, 1, 0]).unwrap().0, "ico");
        assert_eq!(
            format_of(b"\xef\xbb\xbf<?xml version=\"1.0\"?>\n<svg xmlns=\"x\"/>")
                .unwrap()
                .0,
            "svg"
        );
        assert_eq!(format_of(b"<!DOCTYPE html><html>404</html>"), None);
        assert_eq!(format_of(b"<?xml version=\"1.0\"?><error/>"), None);
        assert_eq!(format_of(b""), None);
    }

    #[tokio::test]
    async fn images_on_the_local_network_are_not_fetched() {
        let cache = LogoCache::new(None);
        for url in [
            "http://127.0.0.1/logo.png",
            "http://192.168.1.1/favicon.ico",
            "http://router/logo.png",
        ] {
            assert_eq!(
                cache.download(url, MAX_BYTES).await.unwrap_err(),
                public_net::LOCAL_NETWORK,
                "{url}"
            );
        }
    }

    #[test]
    fn logos_of_stations_not_played_for_long_are_deleted() {
        let dir =
            std::env::temp_dir().join(format!("soundfolk-logos-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["kept.png", "old.png", "writing.part", "cut.part"] {
            std::fs::write(dir.join(name), PNG).unwrap();
        }
        let age = |name: &str, age: Duration| {
            std::fs::File::options()
                .write(true)
                .open(dir.join(name))
                .unwrap()
                .set_modified(SystemTime::now() - age)
                .unwrap();
        };
        age("old.png", FORGOTTEN_AFTER + Duration::from_secs(60));
        age("cut.part", PARTIAL_AFTER + Duration::from_secs(60));
        prune(&dir, SystemTime::now());
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["kept.png", "writing.part"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn images_too_big_are_not_kept() {
        // The size rule is `http::body_at_most`'s, tested there; here, that it applies.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // A local server: only the size is tested, so the public-network rule is left out.
        let cache = LogoCache {
            dir: None,
            client: Some(Client::new()),
        };
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2000000\r\n\r\n")
                .await;
        });
        let url = format!("http://{address}/logo.png").parse().unwrap();
        let result = cache.fetch_unchecked(url, MAX_BYTES).await;
        assert!(result.is_err());
    }

    #[test]
    fn ids_are_file_names() {
        assert!(is_safe_id("96062a7b-0601-11e8-ae97-52543be04c81"));
        assert!(!is_safe_id("../../etc/passwd"));
        assert!(!is_safe_id("a/b"));
        assert!(!is_safe_id(""));
    }

    #[test]
    fn kept_logos_come_back() {
        let dir = std::env::temp_dir().join(format!("soundfolk-logos-{}", std::process::id()));
        let cache = LogoCache::new(Some(dir.clone()));
        let id = "96062a7b-0601-11e8-ae97-52543be04c81";
        assert_eq!(cache.cached(id), None);

        let file = cache.store(id, "png", PNG).unwrap();
        let (logo, stale) = cache.cached(id).unwrap();
        assert!(!stale);
        assert_eq!(logo.file.as_deref(), Some(file.as_str()));
        assert!(
            logo.inline.starts_with("data:image/png;base64,"),
            "{}",
            logo.inline
        );

        // A new logo in another format replaces the old one
        cache.store(id, "gif", b"GIF89a").unwrap();
        assert!(!dir.join(format!("{id}.png")).exists());
        assert!(cache
            .cached(id)
            .unwrap()
            .0
            .inline
            .starts_with("data:image/gif"));

        cache.forget(id);
        assert_eq!(cache.cached(id), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Against a real logo: `cargo test -- --ignored live_logo`.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_logo_is_kept() {
        let dir = std::env::temp_dir().join(format!("soundfolk-live-logos-{}", std::process::id()));
        let cache = LogoCache::new(Some(dir.clone()));
        let id = "7b6aca3d-dfa3-47dd-9f57-820b04c3f339";
        let url = "https://upload.wikimedia.org/wikipedia/commons/thumb/1/16/FIP_logo_2021.svg/250px-FIP_logo_2021.svg.png";
        let logo = cache.fetch(id, url).await.unwrap();
        assert!(logo.inline.starts_with("data:image/png;base64,"));
        assert_eq!(cache.cached(id).map(|(l, _)| l), Some(logo));
        let _ = std::fs::remove_dir_all(dir);
    }
}
