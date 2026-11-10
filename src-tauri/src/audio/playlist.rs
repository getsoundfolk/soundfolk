// SPDX-License-Identifier: MPL-2.0
//! Resolves a user-entered URL to the actual audio stream URL.
//!
//! Many radios publish a playlist (`.m3u`, `.pls`) rather than the stream itself:
//! we download it, extract its first entry, and repeat if that is still a playlist.
//! A direct audio stream is recognized by its headers and is never downloaded in full.

use std::time::Duration;

use reqwest::Url;

use crate::http::{self, public_net};

/// A legitimate playlist is tiny; beyond this we stop reading the body.
const MAX_PLAYLIST_BYTES: usize = 64 * 1024;
/// Maximum nested playlists followed (a `.pls` pointing to an `.m3u`...).
const MAX_DEPTH: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(10);

///
/// `public_only`: every address followed must be public (`public_net`): the first, the
/// redirects and the playlists' entries.
pub async fn resolve_stream_url(url: Url, public_only: bool) -> Result<Url, String> {
    let client = if public_only {
        http::public_client()?
    } else {
        http::client()?
    };

    let mut current = url;
    for _ in 0..MAX_DEPTH {
        if public_only {
            public_net::check(&current)?;
        }
        let mut response = client
            .get(current.clone())
            .header(reqwest::header::USER_AGENT, http::STREAM_USER_AGENT)
            .timeout(TIMEOUT)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("Cannot connect: {e}"))?;

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let icy = response
            .headers()
            .keys()
            .any(|k| k.as_str().starts_with("icy-"));
        if icy || is_audio_type(&content_type) {
            return Ok(current); // direct stream: close without reading the body
        }

        // Not an audio type: read a small body to see whether it is a playlist.
        let mut body = Vec::new();
        while body.len() < MAX_PLAYLIST_BYTES {
            match response.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) => return Err(format!("Cannot read stream: {e}")),
            }
        }

        match parse_playlist(&body) {
            Playlist::Entry(entry) => {
                current = current
                    .join(&entry)
                    .map_err(|e| format!("Invalid playlist entry ({entry}): {e}"))?;
                if !matches!(current.scheme(), "http" | "https") {
                    return Err(format!("Unsupported protocol: {}", current.scheme()));
                }
            }
            Playlist::Hls => return Err("HLS streams (.m3u8) are not supported yet".into()),
            // Neither playlist nor text: probably a mislabeled audio stream, the probe will tell.
            Playlist::Binary => return Ok(current),
            Playlist::Empty => return Err("Empty playlist".into()),
        }
    }
    Err("Playlists nested too deeply".into())
}

/// `audio/*` MIME types that are actually playlists.
const PLAYLIST_TYPES: [&str; 4] = [
    "audio/x-mpegurl",
    "audio/mpegurl",
    "audio/x-scpls",
    "audio/scpls",
];

fn is_audio_type(content_type: &str) -> bool {
    let essence = content_type.split(';').next().unwrap_or("").trim();
    if PLAYLIST_TYPES.contains(&essence) {
        return false;
    }
    essence.starts_with("audio/") || essence == "application/ogg" || essence == "application/x-ogg"
}

#[derive(Debug, PartialEq)]
enum Playlist {
    Entry(String),
    Hls,
    Empty,
    Binary,
}

fn parse_playlist(body: &[u8]) -> Playlist {
    // Binary data (an audio stream without a proper MIME type) is not a playlist.
    let Ok(text) = std::str::from_utf8(body) else {
        return Playlist::Binary;
    };
    if body.contains(&0) {
        return Playlist::Binary;
    }
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Playlist::Empty;
    }
    if text.contains("#EXT-X-") {
        return Playlist::Hls;
    }

    let lines = text.lines().map(str::trim);
    if text
        .get(..10)
        .is_some_and(|h| h.eq_ignore_ascii_case("[playlist]"))
    {
        // PLS : `File1=http://…`
        let entry = lines.into_iter().find_map(|l| {
            let (key, value) = l.split_once('=')?;
            (key.trim().to_ascii_lowercase().starts_with("file") && !value.trim().is_empty())
                .then(|| value.trim().to_owned())
        });
        return entry.map_or(Playlist::Empty, Playlist::Entry);
    }

    // M3U: first line that is neither empty nor a comment.
    match lines
        .into_iter()
        .find(|l| !l.is_empty() && !l.starts_with('#'))
    {
        Some(entry) if entry.contains("://") || entry.starts_with('/') || entry.contains('.') => {
            Playlist::Entry(entry.to_owned())
        }
        _ => Playlist::Binary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pls() {
        let pls = b"[playlist]\nNumberOfEntries=2\nFile1=http://a.example:8000/live\nTitle1=Radio\nFile2=http://b.example/x\n";
        assert_eq!(
            parse_playlist(pls),
            Playlist::Entry("http://a.example:8000/live".into())
        );
        // Case and whitespace tolerated.
        assert_eq!(
            parse_playlist(b"[Playlist]\r\n file1 = http://a.example/x \r\n"),
            Playlist::Entry("http://a.example/x".into())
        );
    }

    #[test]
    fn parses_m3u_skipping_comments() {
        let m3u = b"\xef\xbb\xbf#EXTM3U\n#EXTINF:-1,Radio\nhttp://a.example/live.mp3\n";
        assert_eq!(
            parse_playlist(m3u),
            Playlist::Entry("http://a.example/live.mp3".into())
        );
        assert_eq!(
            parse_playlist(b"/stream.mp3\n"),
            Playlist::Entry("/stream.mp3".into())
        );
    }

    #[test]
    fn detects_hls_empty_and_binary() {
        assert_eq!(
            parse_playlist(b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\nseg1.ts\n"),
            Playlist::Hls
        );
        assert_eq!(parse_playlist(b"  \n"), Playlist::Empty);
        assert_eq!(
            parse_playlist(b"[playlist]\nNumberOfEntries=0\n"),
            Playlist::Empty
        );
        assert_eq!(
            parse_playlist(&[0xff, 0xfb, 0x90, 0x00, 0x00]),
            Playlist::Binary
        );
        assert_eq!(parse_playlist(b"#EXTM3U\n#EXTINF:1,x\n"), Playlist::Binary);
    }

    #[test]
    fn audio_types_are_direct_streams_but_playlist_types_are_not() {
        assert!(is_audio_type("audio/mpeg"));
        assert!(is_audio_type("audio/aacp; charset=utf-8"));
        assert!(is_audio_type("application/ogg"));
        assert!(!is_audio_type("audio/x-mpegurl"));
        assert!(!is_audio_type("audio/x-scpls; charset=utf-8"));
        assert!(!is_audio_type("text/html"));
        assert!(!is_audio_type(""));
    }
}
