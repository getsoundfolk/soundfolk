// SPDX-License-Identifier: MPL-2.0
//! Share links (docs/16-sharing.md §2): what a link carries, read and written in one place.
//!
//! A link is data from a stranger: every field is cleaned and checked here (§6.1) before anything
//! else sees it, and a link breaking a limit is refused whole, never cut.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::State;
use unicode_normalization::UnicodeNormalization;
use url::Url;

use crate::catalog::radio_browser::is_uuid;
use crate::catalog::Station;
use crate::commands::{with_db, CatalogState};
use crate::db::Database;
use crate::error::AppResult;

/// Where links point until `soundfolk.fm` exists (§2.1).
const LINK_BASE: &str = "https://getsoundfolk.github.io";
/// Hosts whose links are read (§8): the current one and the one to come.
const HOSTS: [&str; 2] = ["getsoundfolk.github.io", "soundfolk.fm"];
/// The app's own scheme (§2.3).
const SCHEME: &str = "soundfolk";

/// Limits of §2.2, in characters of the cleaned text.
const MAX_LIST_TITLE: usize = 40;
const MAX_STATION_NAME: usize = 80;
const MAX_STREAM_URL: usize = 2000;
const MAX_TRACK: usize = 200;
const MAX_FROM: usize = 40;
const MAX_LIST_STATIONS: usize = 40;
/// A published list's id: its file name in soundfolk-stations (15 §6.1).
const MAX_LIST_ID: usize = 64;
/// Longest text read as a link, in bytes, pasted or opened from outside.
const MAX_LINK: usize = 16 * 1024;

/// What a link carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shared {
    /// A catalog station, by its Radio-Browser UUID.
    Station { id: String },
    /// A station added by hand: its name and stream, as the sender had them.
    PersonalStation { name: String, url: String },
    /// A list published in soundfolk-stations.
    List { id: String },
    /// A listener's own list: catalog stations, then the ones added by hand.
    PersonalList {
        title: String,
        stations: Vec<String>,
        personal: Vec<PersonalEntry>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonalEntry {
    pub name: String,
    pub url: String,
}

/// A link: what it carries, the track on air when it was sent, and who sent it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedLink {
    #[serde(flatten)]
    pub item: Shared,
    /// `artist - title`, what was on air when the link was made.
    pub track: Option<String>,
    /// The sender's name, as they typed it: never a verified identity.
    pub from: Option<String>,
}

/// Why a text is not a share link, in words for the listener.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LinkError {
    /// Not a Soundfolk link at all: the text is something else (a search, say).
    #[error("This is not a Soundfolk link.")]
    NotALink,
    /// A Soundfolk link that cannot be read.
    #[error("This link is incomplete: {0}.")]
    Invalid(&'static str),
    /// Made by a newer Soundfolk, in a format this one does not know.
    #[error("This link was made by a newer Soundfolk: update it to open the link.")]
    Newer,
}

// ── Reading ──────────────────────────────────────────────────────────────────

/// Reads a pasted or opened text: an `https` link on a known host, a `soundfolk://` link, or a
/// bare station UUID.
pub fn parse(text: &str) -> Result<SharedLink, LinkError> {
    let text = text.trim();
    if text.len() > MAX_LINK {
        return Err(LinkError::Invalid("the link is too long"));
    }
    if is_uuid(text) {
        return Ok(SharedLink {
            item: Shared::Station {
                id: text.to_ascii_lowercase(),
            },
            track: None,
            from: None,
        });
    }
    let url = Url::parse(text).map_err(|_| LinkError::NotALink)?;
    // `https://host/s/#…` and `soundfolk://s/#…`: the kind is the first path segment, or the
    // scheme link's host.
    let mut segments: Vec<String> = url
        .path_segments()
        .map(|s| s.filter(|p| !p.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    match url.scheme() {
        "https" | "http" => {
            let host = url.host_str().unwrap_or("").to_ascii_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host);
            if !HOSTS.contains(&host) {
                return Err(LinkError::NotALink);
            }
        }
        SCHEME => segments.insert(0, url.host_str().unwrap_or("").to_owned()),
        _ => return Err(LinkError::NotALink),
    }
    let fragment = url.fragment().unwrap_or("");
    let fields = Fields::read(fragment)?;
    let item = match segments.first().map(String::as_str) {
        Some("s") if segments.len() == 1 => station(&fields)?,
        Some("l") if segments.len() <= 2 => list(segments.get(1).map(String::as_str), &fields)?,
        _ => return Err(LinkError::NotALink),
    };
    Ok(SharedLink {
        item,
        track: fields.optional("t", MAX_TRACK)?,
        from: fields.optional("f", MAX_FROM)?,
    })
}

/// The fragment's fields: `key=value` pairs, or a lone id.
struct Fields {
    /// The fragment without `=`: an id.
    id: Option<String>,
    pairs: Vec<(String, String)>,
}

impl Fields {
    fn read(fragment: &str) -> Result<Self, LinkError> {
        if fragment.is_empty() {
            return Ok(Fields {
                id: None,
                pairs: Vec::new(),
            });
        }
        if !fragment.contains('=') {
            let id = percent_decode(fragment);
            return Ok(Fields {
                id: Some(id),
                pairs: Vec::new(),
            });
        }
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(fragment.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let fields = Fields { id: None, pairs };
        if fields.get("v").is_some_and(|v| v != "1") {
            return Err(LinkError::Newer);
        }
        Ok(fields)
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.pairs
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The id: the whole fragment, or its `id` field.
    fn id(&self) -> Option<&str> {
        self.id.as_deref().or_else(|| self.get("id"))
    }

    /// A free text field (track, sender): cleaned; absent or blank is `None`, too long refused.
    fn optional(&self, key: &str, max: usize) -> Result<Option<String>, LinkError> {
        match self.get(key).map(clean) {
            None => Ok(None),
            Some(text) if text.is_empty() => Ok(None),
            Some(text) if text.chars().count() > max => {
                Err(LinkError::Invalid("a text is too long"))
            }
            Some(text) => Ok(Some(text)),
        }
    }
}

fn station(fields: &Fields) -> Result<Shared, LinkError> {
    if let Some(id) = fields.id() {
        return if is_uuid(id) {
            Ok(Shared::Station {
                id: id.to_ascii_lowercase(),
            })
        } else {
            Err(LinkError::Invalid("the station's id is not valid"))
        };
    }
    let entry = personal_entry(fields.get("n"), fields.get("u"))?;
    Ok(Shared::PersonalStation {
        name: entry.name,
        url: entry.url,
    })
}

fn list(path_id: Option<&str>, fields: &Fields) -> Result<Shared, LinkError> {
    if let Some(id) = path_id.or_else(|| fields.id()) {
        return if is_list_id(id) {
            Ok(Shared::List { id: id.to_owned() })
        } else {
            Err(LinkError::Invalid("the list's id is not valid"))
        };
    }
    let title = clean(fields.get("n").unwrap_or(""));
    if title.is_empty() {
        return Err(LinkError::Invalid("the list has no title"));
    }
    if title.chars().count() > MAX_LIST_TITLE {
        return Err(LinkError::Invalid("the list's title is too long"));
    }
    let stations = match fields.get("s") {
        None | Some("") => Vec::new(),
        Some(packed) => unpack_uuids(packed)?,
    };
    let mut seen = std::collections::HashSet::new();
    if !stations.iter().all(|id| seen.insert(id.clone())) {
        return Err(LinkError::Invalid("a station is in the list twice"));
    }
    let personal = fields
        .all("p")
        .map(|entry| {
            let (name, url) = entry
                .split_once('\n')
                .ok_or(LinkError::Invalid("a station has no address"))?;
            personal_entry(Some(name), Some(url))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = stations.len() + personal.len();
    if count == 0 {
        return Err(LinkError::Invalid("the list is empty"));
    }
    if count > MAX_LIST_STATIONS {
        return Err(LinkError::Invalid("the list has too many stations"));
    }
    Ok(Shared::PersonalList {
        title,
        stations,
        personal,
    })
}

fn personal_entry(name: Option<&str>, url: Option<&str>) -> Result<PersonalEntry, LinkError> {
    let name = clean(name.unwrap_or(""));
    if name.is_empty() {
        return Err(LinkError::Invalid("the station has no name"));
    }
    if name.chars().count() > MAX_STATION_NAME {
        return Err(LinkError::Invalid("the station's name is too long"));
    }
    let url = url.unwrap_or("").trim();
    if url.is_empty() {
        return Err(LinkError::Invalid("the station has no address"));
    }
    if url.len() > MAX_STREAM_URL {
        return Err(LinkError::Invalid("the station's address is too long"));
    }
    let parsed =
        Url::parse(url).map_err(|_| LinkError::Invalid("the station's address is not valid"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(LinkError::Invalid(
            "the station's address is not a web address",
        ));
    }
    // `https://radio.fr@other.example/` goes to `other.example`: a login would show a fake domain.
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(LinkError::Invalid("the station's address carries a login"));
    }
    Ok(PersonalEntry {
        name,
        url: parsed.to_string(),
    })
}

// ── Writing ──────────────────────────────────────────────────────────────────

/// The `https` link for `item`, with the track and the sender when given. Texts too long for a
/// link (a long track title, a long name) are shortened, so that the link always reads back.
pub fn link(item: &Shared, track: Option<&str>, from: Option<&str>) -> String {
    let extras = |out: &mut url::form_urlencoded::Serializer<'_, String>| {
        if let Some(track) = track
            .map(|t| shorten(t, MAX_TRACK))
            .filter(|t| !t.is_empty())
        {
            out.append_pair("t", &track);
        }
        if let Some(from) = from.map(|f| shorten(f, MAX_FROM)).filter(|f| !f.is_empty()) {
            out.append_pair("f", &from);
        }
    };
    let mut out = url::form_urlencoded::Serializer::new(String::new());
    match item {
        Shared::Station { id } => {
            extras(&mut out);
            let extra = out.finish();
            if extra.is_empty() {
                format!("{LINK_BASE}/s/#{id}")
            } else {
                format!("{LINK_BASE}/s/#id={id}&{extra}")
            }
        }
        Shared::PersonalStation { name, url } => {
            out.append_pair("n", &shorten(name, MAX_STATION_NAME));
            out.append_pair("u", url);
            extras(&mut out);
            format!("{LINK_BASE}/s/#{}", out.finish())
        }
        Shared::List { id } => {
            extras(&mut out);
            let extra = out.finish();
            if extra.is_empty() {
                format!("{LINK_BASE}/l/{id}/")
            } else {
                format!("{LINK_BASE}/l/{id}/#{extra}")
            }
        }
        Shared::PersonalList {
            title,
            stations,
            personal,
        } => {
            out.append_pair("n", &shorten(title, MAX_LIST_TITLE));
            if !stations.is_empty() {
                out.append_pair("s", &pack_uuids(stations));
            }
            for entry in personal {
                let name = shorten(&entry.name, MAX_STATION_NAME);
                out.append_pair("p", &format!("{name}\n{}", entry.url));
            }
            extras(&mut out);
            format!("{LINK_BASE}/l/#{}", out.finish())
        }
    }
}

/// Checks a link made by [`link`] the way the receiver will read it: the item comes from the
/// window, so it gets the checks of a received one (ids, limits, addresses), and a station id
/// that [`pack_uuids`] cannot write is caught instead of sent as another station.
fn reads_back(url: &str, item: &Shared) -> Result<(), LinkError> {
    let back = parse(url)?.item;
    if let (Shared::PersonalList { stations, .. }, Shared::PersonalList { stations: read, .. }) =
        (item, &back)
        && !stations
            .iter()
            .map(|id| id.to_ascii_lowercase())
            .eq(read.iter().cloned())
    {
        return Err(LinkError::Invalid("a station's id is not valid"));
    }
    Ok(())
}

/// The link with the app's scheme in place of its origin (§2.3), for the page's button.
#[cfg(test)]
fn scheme_link(https: &str) -> String {
    https.replacen(&format!("{LINK_BASE}/"), &format!("{SCHEME}://"), 1)
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Event emitted when a `soundfolk://` link was opened (no payload: the window takes it with
/// [`take_opened_link`], also when it opens later).
pub const OPENED_EVENT: &str = "share:opened";

/// The last `soundfolk://` link opened from outside, until the window takes it: only the last
/// one, so a burst of links shows one sheet (§6.3).
#[derive(Default)]
pub struct OpenedLink(std::sync::Mutex<Option<String>>);

/// A `soundfolk://` link short enough to read: the only text taken from the command line.
fn is_app_link(url: &str) -> bool {
    url.starts_with(&format!("{SCHEME}://")) && url.len() <= MAX_LINK
}

/// Keeps a link opened from outside for the window. Only `soundfolk://` links: nothing else from
/// the command line is read. Returns whether it was kept.
pub fn opened(app: &tauri::AppHandle, url: &str) -> bool {
    use tauri::{Emitter, Manager};
    if !is_app_link(url) {
        return false;
    }
    *app.state::<OpenedLink>()
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some(url.to_owned());
    if let Err(e) = app.emit(OPENED_EVENT, ()) {
        tracing::warn!("failed to emit {OPENED_EVENT}: {e}");
    }
    true
}

/// The link opened from outside since the last call, if any.
#[tauri::command]
pub fn take_opened_link(opened: State<'_, OpenedLink>) -> Option<String> {
    opened.0.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// The sender's name put in links (§4.2), empty by default.
const SETTING_FROM: &str = "share_from";

/// Reads a pasted, dropped or opened text. `None`: not a Soundfolk link (a search, say); an
/// error: a Soundfolk link that cannot be read, with the reason.
#[tauri::command]
pub fn read_share_link(text: String) -> AppResult<Option<SharedLink>> {
    match parse(&text) {
        Ok(link) => Ok(Some(link)),
        Err(LinkError::NotALink) => Ok(None),
        Err(e) => Err(e.to_string().into()),
    }
}

/// A link ready to share, and its QR code.
#[derive(Debug, Serialize)]
pub struct ShareOut {
    pub url: String,
    /// What the link carries: a personal station found in the catalog becomes the catalog's.
    pub item: Shared,
    /// Stream addresses of stations added by hand that the link carries (said to the sender,
    /// §4.3).
    pub addresses: usize,
    /// Stations added by hand that were looked up in the catalog by their stream address.
    pub looked_up: usize,
    /// The QR code as an SVG `data:` address, black on white.
    pub qr: Option<String>,
    /// Modules on a side, quiet zone included: the image is shown at a multiple of it.
    pub qr_modules: u32,
}

/// The link for `item`, with the track on air and the sender's name when set. A station added by
/// hand is looked up in the catalog by its stream first (§2.2): found there, the link names the
/// catalog station and carries no address.
#[tauri::command]
pub async fn make_share_link(
    db: State<'_, Database>,
    catalog: State<'_, CatalogState>,
    item: Shared,
    track: Option<String>,
) -> AppResult<ShareOut> {
    let from = with_db(&db, |db| Ok(db.get_str(SETTING_FROM)?)).await?;
    let (item, looked_up) = match item {
        Shared::PersonalStation { name, url } => {
            let found = catalog.0.by_stream(&url).await.ok().flatten();
            let item = match found {
                Some(station) => Shared::Station { id: station.id },
                None => Shared::PersonalStation { name, url },
            };
            (item, 1)
        }
        other => (other, 0),
    };
    let addresses = match &item {
        Shared::PersonalStation { .. } => 1,
        Shared::PersonalList { personal, .. } => personal.len(),
        _ => 0,
    };
    let url = link(&item, track.as_deref(), from.as_deref());
    reads_back(&url, &item).map_err(|e| match e {
        LinkError::Invalid(why) => format!("This cannot be shared: {why}."),
        other => other.to_string(),
    })?;
    let (qr, qr_modules) = match qr(&url) {
        Some((svg, modules)) => (Some(svg), modules),
        None => (None, 0),
    };
    Ok(ShareOut {
        url,
        item,
        addresses,
        looked_up,
        qr,
        qr_modules,
    })
}

/// The catalog station with this stream, offered in place of a personal station received in a
/// link (§3.3).
#[tauri::command]
pub async fn catalog_station_for_stream(
    catalog: State<'_, CatalogState>,
    url: String,
) -> AppResult<Option<Station>> {
    Ok(catalog.0.by_stream(&url).await?)
}

#[tauri::command]
pub async fn get_share_name(db: State<'_, Database>) -> AppResult<String> {
    with_db(&db, |db| Ok(db.get_str(SETTING_FROM)?.unwrap_or_default())).await
}

/// Saves the sender's name, cleaned and cut to what a link carries; returns it.
#[tauri::command]
pub async fn set_share_name(db: State<'_, Database>, name: String) -> AppResult<String> {
    let name = shorten(&name, MAX_FROM);
    with_db(&db, move |db| {
        db.set_str(SETTING_FROM, &name)?;
        Ok(name)
    })
    .await
}

/// The QR code of a link (error correction L: a screen is clean, every module counts), as an SVG
/// `data:` address and its size in modules with the quiet zone.
fn qr(url: &str) -> Option<(String, u32)> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::with_error_correction_level(url, qrcode::EcLevel::L).ok()?;
    let modules = code.width() as u32 + 8;
    let image = code
        .render::<svg::Color<'_>>()
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .quiet_zone(true)
        .module_dimensions(1, 1)
        .build();
    Some((
        format!("data:image/svg+xml;base64,{}", STANDARD.encode(image)),
        modules,
    ))
}

// ── Text ─────────────────────────────────────────────────────────────────────

/// Cleans a text from a link (§6.1): NFC, without control or invisible formatting characters
/// (which could hide text or show a fake domain), whitespace collapsed.
pub fn clean(text: &str) -> String {
    let kept: String = text
        .nfc()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| !c.is_control() && !is_invisible(c))
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Formatting characters that change how text shows without showing themselves.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}' // soft hyphen
        | '\u{061C}' // Arabic letter mark
        | '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' // Hangul fillers
        | '\u{180E}' // Mongolian vowel separator
        | '\u{200B}'..='\u{200F}' // zero-width characters, LRM, RLM
        | '\u{202A}'..='\u{202E}' // bidirectional embeddings and overrides
        | '\u{2060}'..='\u{2064}' // word joiner, invisible operators
        | '\u{2066}'..='\u{2069}' // bidirectional isolates
        | '\u{206A}'..='\u{206F}' // deprecated format characters
        | '\u{FEFF}' // zero-width no-break space
        | '\u{FFF9}'..='\u{FFFB}' // interlinear annotations
        | '\u{E0000}'..='\u{E007F}' // tags
    )
}

/// Cleaned, and cut with an ellipsis when longer than `max` characters.
fn shorten(text: &str, max: usize) -> String {
    let text = clean(text);
    if text.chars().count() <= max {
        return text;
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={text}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn is_list_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_LIST_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// UUIDs as 16 raw bytes each, in base64url without padding (§2.2).
fn pack_uuids(ids: &[String]) -> String {
    let mut bytes = Vec::with_capacity(ids.len() * 16);
    for id in ids {
        let hex: String = id.chars().filter(|c| *c != '-').collect();
        for pair in hex.as_bytes().chunks(2) {
            let pair = std::str::from_utf8(pair).unwrap_or("00");
            bytes.push(u8::from_str_radix(pair, 16).unwrap_or(0));
        }
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unpack_uuids(packed: &str) -> Result<Vec<String>, LinkError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(packed.trim())
        .map_err(|_| LinkError::Invalid("the list's stations cannot be read"))?;
    if bytes.len() % 16 != 0 {
        return Err(LinkError::Invalid("the list's stations cannot be read"));
    }
    Ok(bytes
        .chunks(16)
        .map(|b| {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIP: &str = "96062a7b-0601-11e8-ae97-52543be04c81";
    const OTHER: &str = "78012206-1aa1-11e9-a80b-52543be04c81";

    fn station(id: &str) -> Shared {
        Shared::Station { id: id.into() }
    }

    #[test]
    fn a_catalog_station_reads_from_every_form() {
        for text in [
            format!("https://getsoundfolk.github.io/s/#{FIP}"),
            format!("https://getsoundfolk.github.io/s#{FIP}"),
            format!("https://soundfolk.fm/s/#{FIP}"),
            format!("https://www.soundfolk.fm/s/#id={FIP}"),
            format!("soundfolk://s/#{FIP}"),
            format!("  {FIP}  "),
            format!("https://getsoundfolk.github.io/s/#{}", FIP.to_uppercase()),
        ] {
            let link = parse(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(link.item, station(FIP), "{text}");
        }
    }

    #[test]
    fn other_links_and_texts_are_not_share_links() {
        for text in [
            "fip",
            "https://example.org/s/#96062a7b-0601-11e8-ae97-52543be04c81",
            "https://getsoundfolk.github.io/about/",
            "https://getsoundfolk.github.io/s/extra/#x",
            "ftp://soundfolk.fm/s/#x",
            "",
        ] {
            assert_eq!(parse(text), Err(LinkError::NotALink), "{text}");
        }
    }

    #[test]
    fn broken_links_say_so() {
        for text in [
            "https://getsoundfolk.github.io/s/",
            "https://getsoundfolk.github.io/s/#not-a-uuid",
            "https://getsoundfolk.github.io/s/#n=Radio",
            "https://getsoundfolk.github.io/s/#n=Radio&u=javascript:alert(1)",
            "https://getsoundfolk.github.io/s/#n=Radio&u=file:///etc/passwd",
            "https://getsoundfolk.github.io/s/#u=https://example.org/live",
            "https://getsoundfolk.github.io/l/Bad_Id/",
            "https://getsoundfolk.github.io/l/#n=Empty",
        ] {
            assert!(matches!(parse(text), Err(LinkError::Invalid(_))), "{text}");
        }
        assert_eq!(
            parse(&format!("https://getsoundfolk.github.io/s/#id={FIP}&v=2")),
            Err(LinkError::Newer)
        );
    }

    #[test]
    fn every_kind_reads_back_what_was_written() {
        let items = [
            station(FIP),
            Shared::PersonalStation {
                name: "Radio Cuisine & Co".into(),
                url: "https://stream.example.org/live?format=mp3&x=1".into(),
            },
            Shared::List {
                id: "late-night-jazz".into(),
            },
            Shared::PersonalList {
                title: "Late night".into(),
                stations: vec![FIP.into(), OTHER.into()],
                personal: vec![PersonalEntry {
                    name: "Kitchen radio".into(),
                    url: "http://radio.example.org:8000/live".into(),
                }],
            },
        ];
        for item in items {
            for (track, from) in [
                (None, None),
                (Some("Nina Simone — Sinnerman"), Some("Raph")),
            ] {
                let written = link(&item, track, from);
                let read = parse(&written).unwrap_or_else(|e| panic!("{written}: {e}"));
                assert_eq!(read.item, item, "{written}");
                assert_eq!(read.track.as_deref(), track, "{written}");
                assert_eq!(read.from.as_deref(), from, "{written}");
                // The page's button opens the same thing in the app.
                assert_eq!(parse(&scheme_link(&written)).unwrap(), read, "{written}");
            }
        }
    }

    #[test]
    fn links_have_the_documented_shape() {
        assert_eq!(
            link(&station(FIP), None, None),
            format!("https://getsoundfolk.github.io/s/#{FIP}")
        );
        assert_eq!(
            link(
                &Shared::List {
                    id: "late-night-jazz".into()
                },
                None,
                None
            ),
            "https://getsoundfolk.github.io/l/late-night-jazz/"
        );
        let list = link(
            &Shared::PersonalList {
                title: "x".into(),
                stations: vec![FIP.into(); 1],
                personal: vec![],
            },
            None,
            None,
        );
        // 16 bytes in base64url without padding: 22 characters.
        let packed = list.split("s=").nth(1).unwrap();
        assert_eq!(packed.len(), 22);
    }

    #[test]
    fn texts_are_cleaned() {
        // A right-to-left override would show "moc.elgoog" the other way round.
        assert_eq!(clean("FIP\u{202E}moc.elgoog"), "FIPmoc.elgoog");
        assert_eq!(clean("  Late\u{200B}night \n\t jazz "), "Latenight jazz");
        assert_eq!(clean("Cafe\u{0301}"), "Café");
        let link = parse(&format!(
            "https://getsoundfolk.github.io/s/#id={FIP}&f=%E2%80%AERaph&t=%00Sinnerman"
        ))
        .unwrap();
        assert_eq!(link.from.as_deref(), Some("Raph"));
        assert_eq!(link.track.as_deref(), Some("Sinnerman"));
    }

    #[test]
    fn limits_refuse_the_whole_link() {
        let long = "x".repeat(MAX_TRACK + 1);
        assert!(matches!(
            parse(&format!(
                "https://getsoundfolk.github.io/s/#id={FIP}&t={long}"
            )),
            Err(LinkError::Invalid(_))
        ));
        let many: Vec<String> = (0..=MAX_LIST_STATIONS)
            .map(|i| format!("{i:08x}-0000-0000-0000-000000000000"))
            .collect();
        let written = format!(
            "https://getsoundfolk.github.io/l/#n=Many&s={}",
            pack_uuids(&many)
        );
        assert!(matches!(parse(&written), Err(LinkError::Invalid(_))));
        let twice = format!(
            "https://getsoundfolk.github.io/l/#n=Twice&s={}",
            pack_uuids(&[FIP.into(), FIP.into()])
        );
        assert!(matches!(parse(&twice), Err(LinkError::Invalid(_))));
    }

    #[test]
    fn writing_shortens_what_reading_would_refuse() {
        let track = "y".repeat(500);
        let from = "Someone with a very long name indeed, longer than forty";
        let read = parse(&link(&station(FIP), Some(&track), Some(from))).unwrap();
        assert_eq!(read.track.unwrap().chars().count(), MAX_TRACK);
        assert_eq!(read.from.unwrap().chars().count(), MAX_FROM);
    }

    #[test]
    fn an_address_with_a_login_is_refused() {
        for url in [
            "https://radio.fr@other.example/live",
            "https://user:secret@radio.fr/live",
        ] {
            let written = link(
                &Shared::PersonalStation {
                    name: "Radio".into(),
                    url: url.into(),
                },
                None,
                None,
            );
            assert!(
                matches!(parse(&written), Err(LinkError::Invalid(_))),
                "{url}"
            );
        }
    }

    #[test]
    fn only_app_links_are_taken_from_outside() {
        assert!(is_app_link(&format!("soundfolk://s/#{FIP}")));
        assert!(!is_app_link(&format!(
            "https://getsoundfolk.github.io/s/#{FIP}"
        )));
        assert!(!is_app_link("--hidden"));
        assert!(!is_app_link(&format!(
            "soundfolk://s/#n={}",
            "x".repeat(MAX_LINK)
        )));
    }

    #[test]
    fn a_text_too_long_is_not_read() {
        let long = format!(
            "https://getsoundfolk.github.io/s/#id={FIP}&t={}",
            "y".repeat(MAX_LINK)
        );
        assert_eq!(
            parse(&long),
            Err(LinkError::Invalid("the link is too long"))
        );
    }

    #[test]
    fn what_the_window_asks_to_share_is_checked() {
        let list = |stations: Vec<String>| Shared::PersonalList {
            title: "Mine".into(),
            stations,
            personal: Vec::new(),
        };
        let fine = list(vec![FIP.to_uppercase(), OTHER.into()]);
        assert_eq!(reads_back(&link(&fine, None, None), &fine), Ok(()));
        // 36 characters, not hex: written as another station.
        let not_hex = list(vec!["zzzzzzzz-zzzz-zzzz-zzzz-zzzzzzzzzzzz".into()]);
        assert!(reads_back(&link(&not_hex, None, None), &not_hex).is_err());
        let short = list(vec!["abc".into()]);
        assert!(reads_back(&link(&short, None, None), &short).is_err());
        let many = list(
            (0..=MAX_LIST_STATIONS)
                .map(|i| format!("{i:08x}-0000-0000-0000-000000000000"))
                .collect(),
        );
        assert!(reads_back(&link(&many, None, None), &many).is_err());
        let bad_id = station("not-a-uuid");
        assert!(reads_back(&link(&bad_id, None, None), &bad_id).is_err());
    }
}
