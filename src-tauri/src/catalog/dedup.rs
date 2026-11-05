// SPDX-License-Identifier: MPL-2.0
//! Duplicate stations in search results.
//!
//! Radio-Browser holds the same station several times: added by different people, once per
//! country, once per format (AAC 64k and MP3 128k). About one search result in six was a
//! duplicate (catalog dump of 2026-11-05: "France Inter" gave 18 results for 5 stations).
//! Two entries are the same station when they share any of these keys:
//! - the stream, its URL without scheme, `www.`, trailing slash, or the parameters that track the
//!   listener or carry an access token (same sound, whatever the names say: "RMC FR" and
//!   "BFM Avec RMC"). Only the comparison ignores them: the station plays its URL as given;
//! - the name and the website together ("RTL 2" on `icecast.rtl2.fr` and "RTL2" on
//!   `icecast.rtl.fr`). The name is compared without case, accents, punctuation or spaces: in the
//!   2026-11-05 dump, the 391 groups that ignoring spaces joins are all one station each
//!   ("Radio 98.4" and "Radio 984", "SomaFM" and "Soma FM");
//! - the name, the stream's file name when it holds the name, and the country ("Fip Jazz" plays
//!   `fipjazz-hifi.aac` from `direct.fipradio.fr` and from `icecast.radiofrance.fr`). Without the
//!   country, three "Super FM" (Lithuania, Serbia, Poland) would be one: each plays a `superfm.mp3`.
//!
//! The first entry of a group is kept, so the list must already be in display order, except that a
//! favorite always stays: the user chose that copy, and their history is tied to its ID.

use std::collections::HashSet;

use url::Url;

use super::name_query::fold;
use super::Station;

/// Query parameters that identify the listener, carry an access token or bust a cache: two URLs
/// differing by these play the same stream. Lowercase; `aw_0_` is a prefix (AdsWizz). Seen on
/// identical stream paths in the catalog. `sid` is handled apart: a number picks a SHOUTcast stream.
const IGNORED_PARAMS: &[&str] = &[
    // Listener tracking and cache busting.
    "_", "___cb", "_art", "aggregator", "amsparams", "ar-distributor", "awparams", "cb", "direct",
    "dist", "dlid", "flid", "gdpr", "lat", "listenerid", "lon", "maxservers", "nocache",
    "playsessionid", "rand", "ref", "refresh", "rp_source", "sbmid", "source", "tgt",
    "upd-scheme", "us_privacy", "uuid",
    // Access tokens.
    "md5", "partnertok", "rj-tok", "rj-ttl", "token", "tvf",
];

fn ignored_param(key: &str, value: &str) -> bool {
    let key = key.to_ascii_lowercase();
    IGNORED_PARAMS.contains(&key.as_str())
        || key.starts_with("aw_0_")
        || (key == "sid" && !value.chars().all(|c| c.is_ascii_digit()))
}

/// Drops the entries that repeat an earlier one or a favorite (see the module). `favorites` are
/// station IDs; a favorite is never dropped, even next to another favorite of the same group.
pub fn dedup(stations: &mut Vec<Station>, favorites: &HashSet<String>) {
    let keys = |s: &Station| -> Vec<String> {
        [stream_key(&s.url), name_key(&s.name, s.homepage.as_deref()), file_key(s)]
            .into_iter()
            .flatten()
            .collect()
    };
    // Groups that hold a favorite are taken by it, wherever it stands in the list.
    let mut seen: HashSet<String> =
        stations.iter().filter(|s| favorites.contains(&s.id)).flat_map(keys).collect();
    stations.retain(|s| {
        if favorites.contains(&s.id) {
            return true;
        }
        let keys = keys(s);
        let repeated = keys.iter().any(|k| seen.contains(k));
        seen.extend(keys);
        !repeated
    });
}

fn stream_key(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    let mut params: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, v)| !ignored_param(k, v))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    params.sort();
    let query: Vec<String> = params.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
    Some(format!(
        "stream {}:{}{}?{}",
        bare_host(&url)?,
        url.port().map_or_else(String::new, |p| p.to_string()),
        url.path().trim_end_matches('/'),
        query.join("&"),
    ))
}

/// The name without case, accents, punctuation or spaces, with the website; `None` when either is
/// missing (a name alone is not enough: "Radio 1" exists in many countries).
fn name_key(name: &str, homepage: Option<&str>) -> Option<String> {
    let words = squeeze(name);
    let homepage = homepage?.trim();
    let site = Url::parse(homepage).or_else(|_| Url::parse(&format!("http://{homepage}"))).ok()?;
    if words.is_empty() {
        return None;
    }
    Some(format!("name {words} @ {}{}", bare_host(&site)?, site.path().trim_end_matches('/')))
}

/// Audio file extensions: a stream URL ending with one names the stream by its file.
const AUDIO_FILES: &[&str] = &["aac", "flac", "m4a", "mp3", "ogg", "opus"];

/// The name, the stream's file name and the country, when the file name holds the name (a generic
/// `stream.mp3` says nothing about the station). Names shorter than three characters hold too
/// little to match on.
fn file_key(station: &Station) -> Option<String> {
    let url = Url::parse(&station.url).ok()?;
    let file = url.path_segments()?.rev().find(|s| !s.is_empty())?.to_lowercase();
    let (stem, extension) = file.rsplit_once('.')?;
    if !AUDIO_FILES.contains(&extension) {
        return None;
    }
    let name = squeeze(&station.name);
    if name.chars().count() < 3 || !squeeze(stem).contains(&name) {
        return None;
    }
    let country = station.country_code.as_deref().unwrap_or("").to_ascii_uppercase();
    Some(format!("file {name} @ {file} in {country}"))
}

/// Text without case, accents, punctuation or spaces.
fn squeeze(text: &str) -> String {
    fold(text).chars().filter(|c| c.is_alphanumeric()).collect()
}

fn bare_host(url: &Url) -> Option<String> {
    let host = url.host_str().filter(|h| !h.is_empty())?.to_ascii_lowercase();
    Some(host.strip_prefix("www.").map(str::to_owned).unwrap_or(host))
}

#[cfg(test)]
mod tests {
    use super::super::Health;
    use super::*;

    fn station(name: &str, url: &str, homepage: Option<&str>) -> Station {
        Station {
            id: String::new(),
            name: name.into(),
            url: url.into(),
            homepage: homepage.map(str::to_owned),
            favicon: None,
            tags: vec![],
            country: None,
            country_code: None,
            language: None,
            codec: None,
            bitrate: 0,
            votes: 0,
            health: Health::Ok,
        }
    }

    fn kept(mut stations: Vec<Station>) -> Vec<String> {
        dedup(&mut stations, &HashSet::new());
        stations.into_iter().map(|s| s.name).collect()
    }

    #[test]
    fn the_same_stream_is_one_station_whatever_its_name() {
        let names = kept(vec![
            station("RMC FR", "https://audio.bfmtv.com/rmcradio_128.mp3", None),
            station("BFM Avec RMC", "http://www.audio.bfmtv.com/rmcradio_128.mp3/", None),
            station("Radio Armisa", "https://onair.armisa.it/radio.mp3?aggregator=direktlink&listenerid=42", None),
            station("Radio Armisa (2)", "https://onair.armisa.it/radio.mp3", None),
            station("Ads", "https://ads.example/live?aw_0_1st.playerid=x", None),
            station("Ads again", "https://ads.example/live", None),
            station("1LIVE", "https://f111.rndfnk.com/1live/stream.mp3?sid=2Abg1sp80J&token=9Acp&tvf=x4Q5", None),
            station("1LIVE (2)", "https://f111.rndfnk.com/1live/stream.mp3?sid=3ElSIypHce&token=Wiza&tvf=JNC_", None),
        ]);
        assert_eq!(names, ["RMC FR", "Radio Armisa", "Ads", "1LIVE"]);
    }

    #[test]
    fn parameters_that_choose_the_stream_are_kept() {
        let names = kept(vec![
            station("Channel 1", "https://player.example/listen?id=1", None),
            station("Channel 2", "https://player.example/listen?id=2", None),
            station("Other port", "https://player.example:8000/listen?id=1", None),
            station("SHOUTcast 1", "http://sc.example:8000/stream?sid=1", None),
            station("SHOUTcast 2", "http://sc.example:8000/stream?sid=2", None),
        ]);
        assert_eq!(names, ["Channel 1", "Channel 2", "Other port", "SHOUTcast 1", "SHOUTcast 2"]);
    }

    #[test]
    fn the_same_name_on_the_same_website_is_one_station() {
        let names = kept(vec![
            station("RTL 2", "https://icecast.rtl.fr/rtl2-1-44-128", Some("https://www.rtl2.fr/")),
            station("RTL2", "http://icecast.rtl2.fr/rtl2-1-44-128", Some("rtl2.fr")),
            station("Rádio Nova!", "https://a.example/nova.aac", Some("https://nova.fr")),
            station("radio nova", "https://a.example/nova.mp3", Some("http://nova.fr")),
        ]);
        assert_eq!(names, ["RTL 2", "Rádio Nova!"]);
    }

    #[test]
    fn a_name_alone_or_a_website_alone_is_not_enough() {
        let names = kept(vec![
            station("Radio 1", "https://a.example/1", None),
            station("Radio 1", "https://b.example/1", None),
            station("Jazz", "https://c.example/jazz", Some("https://laut.fm")),
            station("Rock", "https://c.example/rock", Some("https://laut.fm")),
            // Names in other scripts keep their letters ("90-е" is not "90").
            station("90-е Россия", "https://d.example/a", Some("https://pcradio.ru")),
            station("Лихие 90-е", "https://d.example/b", Some("https://pcradio.ru")),
        ]);
        assert_eq!(names.len(), 6);
    }

    #[test]
    fn a_favorite_keeps_its_place_in_its_group() {
        let with_id = |id: &str, name: &str| Station { id: id.into(), ..station(name, "https://a.example/live.mp3", None) };
        let mut stations = vec![with_id("popular", "RTL2"), with_id("mine", "RTL2 (fav)"), with_id("other", "RTL2 (3)")];
        dedup(&mut stations, &HashSet::from(["mine".to_string()]));
        let ids: Vec<_> = stations.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["mine"]);

        // Two favorites of the same group: the user chose both.
        let mut stations = vec![with_id("a", "X"), with_id("b", "X"), with_id("c", "X")];
        dedup(&mut stations, &HashSet::from(["b".to_string(), "c".to_string()]));
        let ids: Vec<_> = stations.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
    }

    #[test]
    fn the_same_file_on_two_servers_is_one_station_in_one_country() {
        let in_country = |name: &str, url: &str, country: &str| Station {
            country_code: Some(country.into()),
            ..station(name, url, None)
        };
        let names = kept(vec![
            in_country("FIP Jazz", "http://icecast.radiofrance.fr/fipjazz-hifi.aac", "FR"),
            in_country("Fip Jazz", "http://direct.fipradio.fr/live/fipjazz-hifi.aac", "FR"),
            // Same name and file, another country: another station.
            in_country("Super FM", "https://onair.superfm.rs/superfm.mp3", "RS"),
            in_country("Super FM", "https://stream.super.fm/superfm.mp3", "PL"),
            // A generic file name does not tell the station.
            in_country("Kontrafunk", "https://a.example/radio.mp3", "DE"),
            in_country("Kontrafunk", "https://b.example/radio.mp3", "DE"),
            // Too short a name to look for in a file name.
            in_country("R1", "https://a.example/r1.mp3", "FR"),
            in_country("R1", "https://b.example/r1.mp3", "FR"),
        ]);
        assert_eq!(names, ["FIP Jazz", "Super FM", "Super FM", "Kontrafunk", "Kontrafunk", "R1", "R1"]);
    }
}
