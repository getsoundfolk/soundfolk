// SPDX-License-Identifier: MPL-2.0
//! Generic readers for streaming servers, which cover every station they host without a recipe.
//!
//! * **Icecast** publishes `/status-json.xsl` with the title of each mount point.
//! * **Shoutcast** v2 publishes `/stats?sid=1&json=1`, v1 `/7.html`.
//!
//! Both sometimes know the title even when the stream carries no inline metadata. A matching URL
//! is not proof: the host keeps an adapter only once its document has the expected shape.

use serde_json::Value;

use crate::value::clean_text;
use crate::{ExtractError, RawTrack, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Adapter {
    IcecastStatus,
    ShoutcastV2,
    ShoutcastV1,
}

/// A document to read with an adapter.
#[derive(Debug, Clone, PartialEq)]
pub struct Probe {
    pub adapter: Adapter,
    pub url: String,
}

impl Adapter {
    pub fn name(self) -> &'static str {
        match self {
            Self::IcecastStatus => "icecast",
            Self::ShoutcastV2 => "shoutcast-v2",
            Self::ShoutcastV1 => "shoutcast-v1",
        }
    }

    /// The document this adapter reads for `stream` (`scheme://host[:port]/…`).
    pub fn probe(self, stream: &str) -> Option<Probe> {
        let origin = origin(stream)?;
        let url = match self {
            Self::IcecastStatus => format!("{origin}/status-json.xsl"),
            Self::ShoutcastV2 => format!("{origin}/stats?sid=1&json=1"),
            Self::ShoutcastV1 => format!("{origin}/7.html"),
        };
        Some(Probe { adapter: self, url })
    }

    /// Reads the document fetched from [`Adapter::probe`].
    pub fn read(self, stream: &str, document: &str) -> Result<Snapshot, ExtractError> {
        let title = match self {
            Self::IcecastStatus => icecast_title(stream, document)?,
            Self::ShoutcastV2 => shoutcast_v2_title(document)?,
            Self::ShoutcastV1 => shoutcast_v1_title(document)?,
        };
        Ok(Snapshot { current: title, ..Snapshot::default() })
    }
}

/// Adapters worth trying for a stream, most likely first. `server` is the stream's HTTP
/// `Server` header, when known.
pub fn candidates(server: Option<&str>) -> Vec<Adapter> {
    let server = server.unwrap_or_default().to_ascii_lowercase();
    if server.contains("icecast") {
        vec![Adapter::IcecastStatus]
    } else if server.contains("shoutcast") {
        vec![Adapter::ShoutcastV2, Adapter::ShoutcastV1]
    } else {
        vec![Adapter::IcecastStatus, Adapter::ShoutcastV2]
    }
}

fn origin(stream: &str) -> Option<String> {
    let (scheme, rest) = stream.trim().split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().filter(|a| !a.is_empty())?;
    let authority = authority.rsplit('@').next()?; // drop credentials
    Some(format!("{}://{authority}", scheme.to_ascii_lowercase()))
}

fn path_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').to_ascii_lowercase()
}

fn icecast_title(stream: &str, document: &str) -> Result<Option<RawTrack>, ExtractError> {
    let shape = || ExtractError::Shape("not an Icecast status".into());
    let json: Value = serde_json::from_str(document).map_err(|_| shape())?;
    let sources = match json.get("icestats").ok_or_else(shape)?.get("source") {
        Some(Value::Array(sources)) => sources.iter().collect(),
        Some(source @ Value::Object(_)) => vec![source],
        _ => Vec::new(), // no mount point on air
    };
    let wanted = path_of(stream);
    let source = sources
        .iter()
        .find(|s| s.get("listenurl").and_then(Value::as_str).is_some_and(|u| path_of(u) == wanted))
        .or(if sources.len() == 1 { sources.first() } else { None });
    let Some(source) = source else { return Ok(None) };

    let text = |key: &str| source.get(key).and_then(Value::as_str).and_then(clean_text);
    Ok(match (text("artist"), text("title")) {
        (artist, Some(title)) => Some(RawTrack { artist, title: Some(title), ..RawTrack::default() }),
        (Some(artist), None) => Some(RawTrack { title: Some(artist), ..RawTrack::default() }),
        (None, None) => None,
    })
}

fn shoutcast_v2_title(document: &str) -> Result<Option<RawTrack>, ExtractError> {
    let json: Value =
        serde_json::from_str(document).map_err(|_| ExtractError::Shape("not a Shoutcast v2 status".into()))?;
    let Some(song) = json.get("songtitle") else {
        return Err(ExtractError::Shape("not a Shoutcast v2 status".into()));
    };
    Ok(song.as_str().and_then(clean_text).map(|t| RawTrack { title: Some(t), ..RawTrack::default() }))
}

/// `<body>listeners,status,peak,max,unique,bitrate,Song title</body>` (the title may contain commas).
fn shoutcast_v1_title(document: &str) -> Result<Option<RawTrack>, ExtractError> {
    let shape = || ExtractError::Shape("not a Shoutcast v1 status".into());
    let lower = document.to_ascii_lowercase();
    let start = lower.find("<body>").ok_or_else(shape)? + "<body>".len();
    let end = lower[start..].find("</body>").map_or(document.len(), |e| start + e);
    let fields: Vec<&str> = document[start..end].splitn(7, ',').collect();
    if fields.len() < 7 || !fields[..6].iter().all(|f| f.trim().parse::<i64>().is_ok()) {
        return Err(shape());
    }
    Ok(clean_text(fields[6]).map(|t| RawTrack { title: Some(t), ..RawTrack::default() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(snapshot: Snapshot) -> Option<(Option<String>, String)> {
        snapshot.current.map(|t| (t.artist, t.title.unwrap()))
    }

    #[test]
    fn probes_derive_from_the_stream() {
        let probe = Adapter::IcecastStatus.probe("https://radio.example.org:8443/live.mp3?x=1").unwrap();
        assert_eq!(probe.url, "https://radio.example.org:8443/status-json.xsl");
        assert_eq!(
            Adapter::ShoutcastV2.probe("http://1.2.3.4:8000/;").unwrap().url,
            "http://1.2.3.4:8000/stats?sid=1&json=1"
        );
        assert_eq!(Adapter::IcecastStatus.probe("ftp://x/y"), None);
        assert_eq!(candidates(Some("Icecast 2.4.4")), vec![Adapter::IcecastStatus]);
    }

    #[test]
    fn icecast_picks_the_mount_point() {
        let status = r#"{"icestats": {"source": [
            {"listenurl": "http://radio.example.org:8000/hd", "title": "Other"},
            {"listenurl": "http://radio.example.org:8000/live", "artist": "Air", "title": "Sexy Boy"}]}}"#;
        let snapshot = Adapter::IcecastStatus.read("https://radio.example.org/live/", status).unwrap();
        assert_eq!(title(snapshot), Some((Some("Air".into()), "Sexy Boy".into())));

        let single = r#"{"icestats": {"source": {"listenurl": "http://x:8000/stream", "title": "Moby - Porcelain"}}}"#;
        let snapshot = Adapter::IcecastStatus.read("https://cdn.example/other", single).unwrap();
        assert_eq!(title(snapshot), Some((None, "Moby - Porcelain".into())));

        let silent = r#"{"icestats": {"source": {"listenurl": "http://x:8000/stream"}}}"#;
        assert_eq!(Adapter::IcecastStatus.read("http://x:8000/stream", silent).unwrap(), Snapshot::default());
        assert!(Adapter::IcecastStatus.read("http://x/", "<html>404</html>").is_err());
        assert!(Adapter::IcecastStatus.read("http://x/", r#"{"status": "ok"}"#).is_err());
    }

    #[test]
    fn shoutcast_documents() {
        let v2 = r#"{"currentlisteners": 3, "songtitle": "Air - Sexy Boy", "nextsongtitle": ""}"#;
        assert_eq!(title(Adapter::ShoutcastV2.read("", v2).unwrap()), Some((None, "Air - Sexy Boy".into())));
        assert!(Adapter::ShoutcastV2.read("", r#"{"icestats": {}}"#).is_err());

        let v1 = "<HTML><meta http-equiv=\"Pragma\" content=\"no-cache\"></head><body>12,1,40,100,11,128,Simon, Garfunkel - Mrs. Robinson</body></html>";
        assert_eq!(
            title(Adapter::ShoutcastV1.read("", v1).unwrap()),
            Some((None, "Simon, Garfunkel - Mrs. Robinson".into()))
        );
        assert!(Adapter::ShoutcastV1.read("", "<html><body>Not found</body></html>").is_err());
    }
}
