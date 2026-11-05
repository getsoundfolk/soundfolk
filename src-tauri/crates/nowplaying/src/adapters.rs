// SPDX-License-Identifier: MPL-2.0
//! Generic readers for streaming servers, which cover every station they host without a recipe.
//!
//! * **Icecast** publishes `/status-json.xsl` with the title of each mount point.
//! * **Shoutcast** v2 publishes `/stats?sid=1&json=1`, v1 `/7.html`.
//! * **AzuraCast** publishes `/api/nowplaying[/{station}]`: artist and title apart, start, length,
//!   cover, the next track and the live DJ.
//!
//! They sometimes know the title even when the stream carries no inline metadata. A matching URL
//! is not proof: the host keeps an adapter only once its document has the expected shape (and,
//! for AzuraCast, lists the stream). AzuraCast is also read alongside ICY titles, for the
//! details of the track announced ([`Adapter::details`]).

use serde_json::Value;

use jiff::SignedDuration;

use crate::value::{clean_text, start_from_number};
use crate::{ExtractError, RawTrack, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Adapter {
    IcecastStatus,
    ShoutcastV2,
    ShoutcastV1,
    AzuraCast,
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
            Self::AzuraCast => "azuracast",
        }
    }

    /// The document this adapter reads for `stream` (`scheme://host[:port]/…`).
    pub fn probe(self, stream: &str) -> Option<Probe> {
        let origin = origin(stream)?;
        let url = match self {
            Self::IcecastStatus => format!("{origin}/status-json.xsl"),
            Self::ShoutcastV2 => format!("{origin}/stats?sid=1&json=1"),
            Self::ShoutcastV1 => format!("{origin}/7.html"),
            // `/listen/{station}/…` names the station; otherwise read every station of the
            // server and find the stream among their mount points.
            Self::AzuraCast => match azuracast_proxy(stream) {
                Some((Proxy::Station(station), _)) => format!("{origin}/api/nowplaying/{station}"),
                _ => format!("{origin}/api/nowplaying"),
            },
        };
        Some(Probe { adapter: self, url })
    }

    /// Reads the document fetched from [`Adapter::probe`].
    pub fn read(self, stream: &str, document: &str) -> Result<Snapshot, ExtractError> {
        let title = match self {
            Self::IcecastStatus => icecast_title(stream, document)?,
            Self::ShoutcastV2 => shoutcast_v2_title(document)?,
            Self::ShoutcastV1 => shoutcast_v1_title(document)?,
            Self::AzuraCast => return azuracast(stream, document),
        };
        Ok(Snapshot {
            current: title,
            ..Snapshot::default()
        })
    }

    /// Whether the adapter knows more than the stream's own title (artist and title apart,
    /// start, length, cover, next track): worth reading even when the stream announces titles.
    pub fn enriches(self) -> bool {
        self == Self::AzuraCast
    }

    /// The details of the track the stream announces as `heard` (its ICY title), from the
    /// document fetched from [`Adapter::probe`]. `None` when the document does not know that
    /// track, or for adapters that do not [enrich](Adapter::enriches).
    pub fn details(
        self,
        stream: &str,
        document: &str,
        heard: &str,
    ) -> Result<Option<Snapshot>, ExtractError> {
        match self {
            Self::AzuraCast => azuracast_details(stream, document, heard),
            _ => Ok(None),
        }
    }
}

/// Adapters worth trying for a stream, most likely first. `server` is the stream's HTTP
/// `Server` header, when known.
pub fn candidates(stream: &str, server: Option<&str>) -> Vec<Adapter> {
    let server = server.unwrap_or_default().to_ascii_lowercase();
    let mut adapters = if server.contains("icecast") {
        vec![Adapter::IcecastStatus]
    } else if server.contains("shoutcast") {
        vec![Adapter::ShoutcastV2, Adapter::ShoutcastV1]
    } else {
        vec![Adapter::IcecastStatus, Adapter::ShoutcastV2]
    };
    // AzuraCast proxies its Icecast / Shoutcast mounts behind its own web server, under
    // `/listen/{station}/…` or `/radio/{port}/…`: its API knows more than their status pages.
    if azuracast_proxy(stream).is_some() {
        adapters.insert(0, Adapter::AzuraCast);
    }
    adapters
}

fn origin(stream: &str) -> Option<String> {
    let (scheme, rest) = stream.trim().split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    let authority = authority.rsplit('@').next()?; // drop credentials
    Some(format!("{}://{authority}", scheme.to_ascii_lowercase()))
}

fn path_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.find('/').map_or("", |i| &rest[i..]);
    path.split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// How an AzuraCast web server proxies a stream: `/listen/{station}/{mount}` or
/// `/radio/{port}/{mount}`.
#[derive(Debug, PartialEq)]
enum Proxy {
    Station(String),
    Port(u16),
}

/// The proxy prefix of an AzuraCast stream URL, and the mount point after it (`/radio.mp3`).
fn azuracast_proxy(stream: &str) -> Option<(Proxy, String)> {
    let path = path_of(stream);
    let mut parts = path.trim_start_matches('/').splitn(3, '/');
    let (kind, key, mount) = (parts.next()?, parts.next()?, parts.next()?);
    if key.is_empty() || mount.is_empty() {
        return None;
    }
    let proxy = match kind {
        "listen"
            if key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') =>
        {
            Proxy::Station(key.to_owned())
        }
        "radio" => Proxy::Port(key.parse().ok()?),
        _ => return None,
    };
    Some((proxy, format!("/{mount}")))
}

/// The explicit port of a URL (`https://host:8000/…`), if any.
fn port_of(url: &str) -> Option<u16> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let (_, port) = authority
        .rsplit_once(':')
        .filter(|(host, _)| !host.ends_with('['))?;
    port.parse().ok()
}

/// A station entry of `/api/nowplaying`, with the fields only AzuraCast puts together.
fn is_azuracast_station(entry: &Value) -> bool {
    let station = entry.get("station");
    let text =
        |v: Option<&Value>, key: &str| v.and_then(|v| v.get(key)).is_some_and(Value::is_string);
    text(station, "shortcode")
        && text(station, "listen_url")
        && station
            .and_then(|s| s.get("mounts"))
            .is_some_and(Value::is_array)
        && text(entry.get("now_playing").and_then(|n| n.get("song")), "text")
}

/// Whether `stream` is one of the station's streams: a URL it lists (main, mount points, remote
/// relays, HLS), or one of its mount points behind the web server's proxy.
fn azuracast_serves(entry: &Value, stream: &str) -> bool {
    let station = &entry["station"];
    let list = |key: &str| {
        station
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
    };
    let urls = ["listen_url", "hls_url"]
        .into_iter()
        .filter_map(|k| station.get(k)?.as_str())
        .chain(
            list("mounts")
                .chain(list("remotes"))
                .filter_map(|m| m.get("url")?.as_str()),
        );
    let wanted = path_of(stream);
    if urls.clone().any(|u| path_of(u) == wanted) {
        return true;
    }
    let Some((proxy, mount)) = azuracast_proxy(stream) else {
        return false;
    };
    let mounts = list("mounts").filter(|m| {
        let path = m
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
        let url = m.get("url").and_then(Value::as_str);
        path.as_deref() == Some(mount.as_str()) || url.is_some_and(|u| path_of(u) == mount)
    });
    match proxy {
        Proxy::Station(name) => {
            let shortcode = station["shortcode"].as_str().unwrap_or_default();
            shortcode.eq_ignore_ascii_case(&name) && mounts.count() > 0
        }
        // The port is that of the Icecast / Shoutcast server behind the proxy.
        Proxy::Port(port) => mounts
            .filter_map(|m| m.get("url")?.as_str())
            .any(|u| port_of(u) == Some(port)),
    }
}

/// The entry of `/api/nowplaying/{station}` or `/api/nowplaying` that describes `stream`.
/// An error unless the document is AzuraCast's and lists the stream.
fn azuracast_entry<'a>(stream: &str, json: &'a Value) -> Result<&'a Value, ExtractError> {
    let shape = || ExtractError::Shape("not an AzuraCast now-playing document".into());
    let entries: Vec<&Value> = match json {
        Value::Array(entries) => entries.iter().collect(),
        entry => vec![entry],
    };
    if entries.is_empty() || !entries.iter().all(|e| is_azuracast_station(e)) {
        return Err(shape());
    }
    entries
        .into_iter()
        .find(|e| azuracast_serves(e, stream))
        .ok_or_else(|| ExtractError::Shape("this AzuraCast server does not list the stream".into()))
}

fn azuracast_json(document: &str) -> Result<Value, ExtractError> {
    serde_json::from_str(document)
        .map_err(|_| ExtractError::Shape("not an AzuraCast now-playing document".into()))
}

/// What is on air. The next track is left out during a live show: it is what the AutoDJ will
/// play after it, not what the DJ plays.
fn azuracast(stream: &str, document: &str) -> Result<Snapshot, ExtractError> {
    let json = azuracast_json(document)?;
    let entry = azuracast_entry(stream, &json)?;
    if entry.get("is_online").and_then(Value::as_bool) == Some(false) {
        return Ok(Snapshot::default());
    }
    let live = entry
        .get("live")
        .filter(|l| l.get("is_live").and_then(Value::as_bool) == Some(true));
    let show = live
        .and_then(|l| l.get("streamer_name"))
        .and_then(Value::as_str)
        .and_then(clean_text);
    let next = match live {
        Some(_) => Vec::new(),
        None => entry
            .get("playing_next")
            .and_then(azuracast_track)
            .into_iter()
            .collect(),
    };
    Ok(Snapshot {
        current: entry.get("now_playing").and_then(azuracast_track),
        next,
        show,
    })
}

/// How many tracks played before the one on air are searched for the one heard: the API runs
/// ahead of the listener by the stream's buffers.
const HISTORY_SEARCHED: usize = 5;

/// The track the stream announces as `heard`, found on air or among the last ones played.
fn azuracast_details(
    stream: &str,
    document: &str,
    heard: &str,
) -> Result<Option<Snapshot>, ExtractError> {
    let json = azuracast_json(document)?;
    let entry = azuracast_entry(stream, &json)?;
    let heard = normalized(heard);
    if heard.is_empty() {
        return Ok(None);
    }
    let on_air = azuracast(stream, document)?;
    if entry
        .get("now_playing")
        .is_some_and(|t| azuracast_is(t, &heard))
    {
        return Ok(Some(on_air));
    }
    let played = entry
        .get("song_history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    Ok(played
        .take(HISTORY_SEARCHED)
        .find(|t| azuracast_is(t, &heard))
        .map(|t| Snapshot {
            current: azuracast_track(t),
            next: Vec::new(), // what comes after it has already started on the server
            show: on_air.show.clone(),
        }))
}

/// Whether an API track is the one the stream announced (AzuraCast writes its `text`, or
/// `Artist - Title`, as the ICY title).
fn azuracast_is(item: &Value, heard: &str) -> bool {
    let Some(song) = item.get("song") else {
        return false;
    };
    let text = |key: &str| {
        song.get(key)
            .and_then(Value::as_str)
            .map(normalized)
            .unwrap_or_default()
    };
    let (artist, title) = (text("artist"), text("title"));
    let joined = format!("{artist} - {title}");
    text("text") == heard
        || (!title.is_empty() && (joined == heard || (artist.is_empty() && title == heard)))
}

/// Case and spacing ignored.
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Texts AzuraCast puts in place of a track: a remote playlist or stream queued (a news
/// bulletin, a relay), the station off air.
const AZURACAST_PLACEHOLDERS: [&str; 2] = ["remote playlist url", "station offline"];

/// `{"played_at": …, "duration": …, "song": {"artist", "title", "text", "art"}}`; `None` for a
/// placeholder.
fn azuracast_track(item: &Value) -> Option<RawTrack> {
    let song = item.get("song")?;
    let placeholder = song.get("text").and_then(Value::as_str).map(normalized);
    if placeholder.is_some_and(|t| AZURACAST_PLACEHOLDERS.contains(&t.as_str())) {
        return None;
    }
    let text = |key: &str| song.get(key).and_then(Value::as_str).and_then(clean_text);
    let (artist, title) = match (text("artist"), text("title")) {
        (artist, Some(title)) => (artist, title),
        (_, None) => (None, text("text")?),
    };
    let duration = item
        .get("duration")
        .and_then(Value::as_f64)
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(SignedDuration::from_secs_f64);
    Some(RawTrack {
        artist,
        title: Some(title),
        start: item
            .get("played_at")
            .and_then(Value::as_f64)
            .and_then(start_from_number),
        duration,
        cover: text("art"),
    })
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
        .find(|s| {
            s.get("listenurl")
                .and_then(Value::as_str)
                .is_some_and(|u| path_of(u) == wanted)
        })
        .or(if sources.len() == 1 {
            sources.first()
        } else {
            None
        });
    let Some(source) = source else {
        return Ok(None);
    };

    let text = |key: &str| source.get(key).and_then(Value::as_str).and_then(clean_text);
    Ok(match (text("artist"), text("title")) {
        (artist, Some(title)) => Some(RawTrack {
            artist,
            title: Some(title),
            ..RawTrack::default()
        }),
        (Some(artist), None) => Some(RawTrack {
            title: Some(artist),
            ..RawTrack::default()
        }),
        (None, None) => None,
    })
}

fn shoutcast_v2_title(document: &str) -> Result<Option<RawTrack>, ExtractError> {
    let json: Value = serde_json::from_str(document)
        .map_err(|_| ExtractError::Shape("not a Shoutcast v2 status".into()))?;
    let Some(song) = json.get("songtitle") else {
        return Err(ExtractError::Shape("not a Shoutcast v2 status".into()));
    };
    Ok(song.as_str().and_then(clean_text).map(|t| RawTrack {
        title: Some(t),
        ..RawTrack::default()
    }))
}

/// `<body>listeners,status,peak,max,unique,bitrate,Song title</body>` (the title may contain commas).
fn shoutcast_v1_title(document: &str) -> Result<Option<RawTrack>, ExtractError> {
    let shape = || ExtractError::Shape("not a Shoutcast v1 status".into());
    let lower = document.to_ascii_lowercase();
    let start = lower.find("<body>").ok_or_else(shape)? + "<body>".len();
    let end = lower[start..]
        .find("</body>")
        .map_or(document.len(), |e| start + e);
    let fields: Vec<&str> = document[start..end].splitn(7, ',').collect();
    if fields.len() < 7 || !fields[..6].iter().all(|f| f.trim().parse::<i64>().is_ok()) {
        return Err(shape());
    }
    Ok(clean_text(fields[6]).map(|t| RawTrack {
        title: Some(t),
        ..RawTrack::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(snapshot: Snapshot) -> Option<(Option<String>, String)> {
        snapshot.current.map(|t| (t.artist, t.title.unwrap()))
    }

    #[test]
    fn probes_derive_from_the_stream() {
        let probe = Adapter::IcecastStatus
            .probe("https://radio.example.org:8443/live.mp3?x=1")
            .unwrap();
        assert_eq!(probe.url, "https://radio.example.org:8443/status-json.xsl");
        assert_eq!(
            Adapter::ShoutcastV2
                .probe("http://1.2.3.4:8000/;")
                .unwrap()
                .url,
            "http://1.2.3.4:8000/stats?sid=1&json=1"
        );
        assert_eq!(Adapter::IcecastStatus.probe("ftp://x/y"), None);
        assert_eq!(
            candidates("http://x:8000/live", Some("Icecast 2.4.4")),
            vec![Adapter::IcecastStatus]
        );
    }

    #[test]
    fn icecast_picks_the_mount_point() {
        let status = r#"{"icestats": {"source": [
            {"listenurl": "http://radio.example.org:8000/hd", "title": "Other"},
            {"listenurl": "http://radio.example.org:8000/live", "artist": "Air", "title": "Sexy Boy"}]}}"#;
        let snapshot = Adapter::IcecastStatus
            .read("https://radio.example.org/live/", status)
            .unwrap();
        assert_eq!(
            title(snapshot),
            Some((Some("Air".into()), "Sexy Boy".into()))
        );

        let single = r#"{"icestats": {"source": {"listenurl": "http://x:8000/stream", "title": "Moby - Porcelain"}}}"#;
        let snapshot = Adapter::IcecastStatus
            .read("https://cdn.example/other", single)
            .unwrap();
        assert_eq!(title(snapshot), Some((None, "Moby - Porcelain".into())));

        let silent = r#"{"icestats": {"source": {"listenurl": "http://x:8000/stream"}}}"#;
        assert_eq!(
            Adapter::IcecastStatus
                .read("http://x:8000/stream", silent)
                .unwrap(),
            Snapshot::default()
        );
        assert!(Adapter::IcecastStatus
            .read("http://x/", "<html>404</html>")
            .is_err());
        assert!(Adapter::IcecastStatus
            .read("http://x/", r#"{"status": "ok"}"#)
            .is_err());
    }

    #[test]
    fn shoutcast_documents() {
        let v2 = r#"{"currentlisteners": 3, "songtitle": "Air - Sexy Boy", "nextsongtitle": ""}"#;
        assert_eq!(
            title(Adapter::ShoutcastV2.read("", v2).unwrap()),
            Some((None, "Air - Sexy Boy".into()))
        );
        assert!(Adapter::ShoutcastV2
            .read("", r#"{"icestats": {}}"#)
            .is_err());

        let v1 = "<HTML><meta http-equiv=\"Pragma\" content=\"no-cache\"></head><body>12,1,40,100,11,128,Simon, Garfunkel - Mrs. Robinson</body></html>";
        assert_eq!(
            title(Adapter::ShoutcastV1.read("", v1).unwrap()),
            Some((None, "Simon, Garfunkel - Mrs. Robinson".into()))
        );
        assert!(Adapter::ShoutcastV1
            .read("", "<html><body>Not found</body></html>")
            .is_err());
    }

    /// Shaped like a real `/api/nowplaying/{station}` (fields Soundfolk does not read left out).
    const AZURACAST: &str = r#"{
        "station": {"shortcode": "jazz", "listen_url": "https://radio.example.org/listen/jazz/radio.mp3",
            "mounts": [{"path": "/radio.mp3", "url": "https://radio.example.org/listen/jazz/radio.mp3"},
                       {"path": "/hd.aac", "url": "https://radio.example.org:8000/hd.aac"}],
            "remotes": [{"url": "https://relay.example.net/listen/jazz/radio.mp3"}],
            "hls_url": "https://radio.example.org/hls/jazz/live.m3u8"},
        "live": {"is_live": false, "streamer_name": ""},
        "now_playing": {"played_at": 1793828211, "duration": 245, "elapsed": 30,
            "song": {"text": "Air - Sexy Boy", "artist": "Air", "title": "Sexy Boy",
                     "art": "https://radio.example.org/api/station/jazz/art/abc.jpg"}},
        "playing_next": {"played_at": 1793828456, "duration": 180,
            "song": {"text": "Moby - Porcelain", "artist": "", "title": "", "art": ""}},
        "song_history": [
            {"played_at": 1793827960, "duration": 251,
             "song": {"text": "Massive Attack - Teardrop", "artist": "Massive Attack", "title": "Teardrop"}},
            {"played_at": 1793827700, "duration": 260,
             "song": {"text": "Portishead - Roads", "artist": "Portishead", "title": "Roads"}}],
        "is_online": true}"#;

    const STREAM: &str = "https://radio.example.org/listen/jazz/radio.mp3";

    #[test]
    fn azuracast_probes() {
        let probe = Adapter::AzuraCast.probe(STREAM).unwrap();
        assert_eq!(probe.url, "https://radio.example.org/api/nowplaying/jazz");
        let probe = Adapter::AzuraCast
            .probe("https://radio.example.org/radio/8000/radio.mp3")
            .unwrap();
        assert_eq!(probe.url, "https://radio.example.org/api/nowplaying");

        assert_eq!(
            candidates(STREAM, Some("nginx")),
            vec![
                Adapter::AzuraCast,
                Adapter::IcecastStatus,
                Adapter::ShoutcastV2
            ]
        );
        assert_eq!(
            candidates("https://radio.example.org/radio/8010/live", None)[0],
            Adapter::AzuraCast
        );
        for other in [
            "https://radio.example.org/radio/live.mp3",
            "https://radio.example.org/radio/port/live.mp3",
            "https://radio.example.org/listen/",
            "https://radio.example.org/listen/jazz",
        ] {
            assert!(
                !candidates(other, None).contains(&Adapter::AzuraCast),
                "{other}"
            );
        }
        assert!(Adapter::AzuraCast.enriches() && !Adapter::IcecastStatus.enriches());
    }

    #[test]
    fn azuracast_reads_the_station() {
        let snapshot = Adapter::AzuraCast.read(STREAM, AZURACAST).unwrap();
        let current = snapshot.current.unwrap();
        assert_eq!(
            (current.artist.as_deref(), current.title.as_deref()),
            (Some("Air"), Some("Sexy Boy"))
        );
        let start = crate::Timestamp::from_second(1_793_828_211).unwrap();
        assert_eq!(current.start, Some(crate::Start::At(start)));
        assert_eq!(current.duration, Some(SignedDuration::from_secs(245)));
        assert_eq!(
            current.cover.as_deref(),
            Some("https://radio.example.org/api/station/jazz/art/abc.jpg")
        );
        // No artist and title apart: the whole text.
        assert_eq!(snapshot.next[0].artist, None);
        assert_eq!(snapshot.next[0].title.as_deref(), Some("Moby - Porcelain"));
        assert_eq!(snapshot.show, None);

        // Live: the DJ is the show, and the AutoDJ's next track is not announced.
        let live = AZURACAST.replace(
            r#""is_live": false, "streamer_name": """#,
            r#""is_live": true, "streamer_name": "DJ Nova""#,
        );
        let snapshot = Adapter::AzuraCast.read(STREAM, &live).unwrap();
        assert_eq!(snapshot.show.as_deref(), Some("DJ Nova"));
        assert!(snapshot.next.is_empty());

        let offline = AZURACAST.replace(r#""is_online": true"#, r#""is_online": false"#);
        assert_eq!(
            Adapter::AzuraCast.read(STREAM, &offline).unwrap(),
            Snapshot::default()
        );

        // A remote playlist queued next (stereo anime's news): not a track.
        let remote = AZURACAST.replace(
            r#"{"text": "Moby - Porcelain", "artist": "", "title": "", "art": ""}"#,
            r#"{"text": "Remote Playlist URL", "artist": "", "title": "Remote Playlist URL", "art": ""}"#,
        );
        assert!(Adapter::AzuraCast
            .read(STREAM, &remote)
            .unwrap()
            .next
            .is_empty());
    }

    #[test]
    fn azuracast_needs_its_own_shape() {
        for document in [
            "<html>404</html>",
            r#"{"icestats": {}}"#,
            r#"[]"#,
            r#"[{"songtitle": "x"}]"#,
            // Something with a `station` and a `now_playing`, but not AzuraCast's.
            r#"{"station": {"name": "Jazz"}, "now_playing": {"title": "Air - Sexy Boy"}}"#,
        ] {
            assert!(
                Adapter::AzuraCast.read(STREAM, document).is_err(),
                "{document}"
            );
        }
        let no_mounts = AZURACAST.replacen(r#""mounts": ["#, r#""mount_list": ["#, 1);
        assert!(Adapter::AzuraCast.read(STREAM, &no_mounts).is_err());
    }

    #[test]
    fn azuracast_checks_the_stream_is_the_stations() {
        let read = |stream: &str, document: &str| {
            Adapter::AzuraCast
                .read(stream, document)
                .map(|s| s.current.is_some())
        };
        // Listed URLs: main, mount point, relay, HLS (any host, the path decides).
        for stream in [
            STREAM,
            "https://radio.example.org:8000/hd.aac",
            "https://relay.example.net/listen/jazz/radio.mp3",
            "https://cdn.example.com/hls/jazz/live.m3u8",
        ] {
            assert_eq!(read(stream, AZURACAST), Ok(true), "{stream}");
        }
        // Behind the proxy: a mount point of the station named, or of the server on that port.
        assert_eq!(
            read("https://radio.example.org/listen/JAZZ/hd.aac", AZURACAST),
            Ok(true)
        );
        assert_eq!(
            read("https://radio.example.org/radio/8000/hd.aac", AZURACAST),
            Ok(true)
        );
        // Another station, a mount point it does not have, another port.
        for stream in [
            "https://radio.example.org/listen/rock/radio.mp3",
            "https://radio.example.org/listen/jazz/low.mp3",
            "https://radio.example.org/radio/8010/hd.aac",
            "https://radio.example.org/live.mp3",
        ] {
            assert!(read(stream, AZURACAST).is_err(), "{stream}");
        }
    }

    #[test]
    fn azuracast_finds_the_stream_among_stations() {
        let other = AZURACAST
            .replace("jazz", "rock")
            .replace("8000", "8010")
            .replace("Sexy Boy", "Other");
        let all = format!("[{other}, {AZURACAST}]");
        let snapshot = Adapter::AzuraCast
            .read("https://radio.example.org/radio/8000/hd.aac", &all)
            .unwrap();
        assert_eq!(snapshot.current.unwrap().title.as_deref(), Some("Sexy Boy"));
        let snapshot = Adapter::AzuraCast
            .read("https://radio.example.org/radio/8010/hd.aac", &all)
            .unwrap();
        assert_eq!(snapshot.current.unwrap().title.as_deref(), Some("Other"));
        // A single station is not taken for granted.
        assert!(Adapter::AzuraCast
            .read("https://radio.example.org/radio/9000/x", &all)
            .is_err());
        assert!(Adapter::AzuraCast
            .read(
                "https://radio.example.org/radio/9000/x",
                &format!("[{AZURACAST}]")
            )
            .is_err());
    }

    #[test]
    fn azuracast_details_of_the_track_heard() {
        let details = |heard: &str| {
            Adapter::AzuraCast
                .details(STREAM, AZURACAST, heard)
                .unwrap()
        };
        // On air: everything, next track included; case and spaces do not matter.
        let on_air = details("air  -  SEXY boy").unwrap();
        assert_eq!(
            on_air.current.as_ref().unwrap().artist.as_deref(),
            Some("Air")
        );
        assert_eq!(on_air.next.len(), 1);
        // The API runs ahead: the track heard is one already played on the server.
        let behind = details("Massive Attack - Teardrop").unwrap();
        let current = behind.current.unwrap();
        assert_eq!(
            (current.artist.as_deref(), current.title.as_deref()),
            (Some("Massive Attack"), Some("Teardrop"))
        );
        assert_eq!(current.duration, Some(SignedDuration::from_secs(251)));
        assert!(behind.next.is_empty());
        // Unknown to the API (a jingle, a live show it does not see): nothing.
        assert_eq!(details("Jingle"), None);
        assert_eq!(details(""), None);
        // Not AzuraCast, or not this stream: an error, as for `read`.
        assert!(Adapter::AzuraCast
            .details(STREAM, r#"{"icestats": {}}"#, "Air - Sexy Boy")
            .is_err());
        assert!(Adapter::AzuraCast
            .details("https://x/listen/rock/a.mp3", AZURACAST, "Air - Sexy Boy")
            .is_err());
        assert_eq!(
            Adapter::IcecastStatus.details(STREAM, "{}", "Air - Sexy Boy"),
            Ok(None)
        );
    }

    #[test]
    fn ports() {
        assert_eq!(port_of("https://radio.example.org:8000/hd.aac"), Some(8000));
        assert_eq!(port_of("https://user:pw@radio.example.org/hd.aac"), None);
        assert_eq!(port_of("http://[::1]:8010/x"), Some(8010));
        assert_eq!(port_of("http://[::1]/x"), None);
    }
}
