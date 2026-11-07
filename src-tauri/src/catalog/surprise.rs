// SPDX-License-Identifier: MPL-2.0
//! Surprise me (docs/10-discovery.md §4.1): which stations of a random catalog draw may be offered.
//!
//! The catalog cannot say whether a station plays music. Its tags can, roughly: a candidate needs
//! at least one music tag and no talk tag. A station without tags is left out, as is one the
//! catalog's last check found down. Whether the stream really plays is the player's test: the
//! frontend tries the candidates in order until one is heard.

use super::{Health, Station};

/// Below this bitrate the stream is not worth a surprise (0: unknown, kept).
const MIN_BITRATE: u32 = 64;

/// Fragments of tags that mean music. Matched inside each tag, so `jazz` also covers
/// `smooth jazz` and `acid jazz`.
const MUSIC: &[&str] = &[
    "music",
    "musique",
    "musik",
    "música",
    "musica",
    "jazz",
    "blues",
    "soul",
    "funk",
    "rock",
    "pop",
    "metal",
    "punk",
    "indie",
    "alternative",
    "folk",
    "country",
    "classical",
    "classic",
    "baroque",
    "opera",
    "piano",
    "instrumental",
    "soundtrack",
    "electro",
    "techno",
    "house",
    "trance",
    "ambient",
    "chill",
    "lounge",
    "lofi",
    "lo-fi",
    "downtempo",
    "dub",
    "drum and bass",
    "dnb",
    "edm",
    "dance",
    "disco",
    "synth",
    "hip hop",
    "hip-hop",
    "hiphop",
    "rap",
    "r&b",
    "rnb",
    "reggae",
    "ska",
    "latin",
    "salsa",
    "bossa",
    "samba",
    "tango",
    "flamenco",
    "cumbia",
    "world",
    "afro",
    "celtic",
    "chanson",
    "schlager",
    "oldies",
    "hits",
    "top 40",
    "60s",
    "70s",
    "80s",
    "90s",
    "00s",
    "eclectic",
    "gospel",
    "swing",
    "bebop",
    "fusion",
    "experimental",
    "noise",
    "grunge",
    "new wave",
    "k-pop",
    "j-pop",
    "kpop",
    "jpop",
    "bollywood",
    "kizomba",
    "zouk",
];

/// Fragments of tags that mean talk, which win over any music tag: a "news and music" station
/// talks between the songs.
const TALK: &[&str] = &[
    "news",
    "talk",
    "sport",
    "podcast",
    "politic",
    "information",
    "actualit",
    "nachrichten",
    "noticias",
    "comedy",
    "spoken",
    "audiobook",
    "sermon",
    "preach",
    "bible",
    "quran",
    "koran",
    "religio",
    "catholic",
    "church",
    "islam",
    "education",
    "weather",
    "traffic",
    "police",
    "scanner",
    "emergency",
];

/// Whether the station's tags say it plays music and not talk.
pub fn plays_music(station: &Station) -> bool {
    let tags: Vec<String> = station.tags.iter().map(|t| t.to_lowercase()).collect();
    let has = |words: &[&str]| tags.iter().any(|t| words.iter().any(|w| t.contains(w)));
    has(MUSIC) && !has(TALK)
}

/// The candidates of a random draw, in the order to try them: healthy music stations of a
/// decent bitrate, never `exclude` (the station on air). Within the catalog's random order, those
/// with a homepage and a favicon come first: someone looks after the entry.
pub fn candidates(draw: Vec<Station>, exclude: Option<&str>) -> Vec<Station> {
    let mut kept: Vec<Station> = draw
        .into_iter()
        .filter(|s| {
            s.health == Health::Ok
                && (s.bitrate == 0 || s.bitrate >= MIN_BITRATE)
                && Some(s.id.as_str()) != exclude
                && plays_music(s)
        })
        .collect();
    kept.sort_by_key(|s| !(s.homepage.is_some() && s.favicon.is_some()));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: &str, tags: &[&str]) -> Station {
        Station {
            id: id.to_owned(),
            name: id.to_owned(),
            url: format!("http://{id}.example/stream"),
            homepage: None,
            favicon: None,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            country: None,
            country_code: None,
            language: None,
            codec: None,
            bitrate: 128,
            votes: 0,
            health: Health::Ok,
        }
    }

    #[test]
    fn music_tags_are_needed_and_talk_tags_win() {
        assert!(plays_music(&station("a", &["Smooth Jazz"])));
        assert!(plays_music(&station("b", &["80s", "hits"])));
        assert!(!plays_music(&station("c", &[])));
        assert!(!plays_music(&station("d", &["news", "talk"])));
        assert!(!plays_music(&station("e", &["news", "pop"])));
        assert!(!plays_music(&station("f", &["catholic", "music"])));
        assert!(!plays_music(&station("g", &["local", "community"])));
    }

    #[test]
    fn candidates_skip_the_unfit_and_put_maintained_entries_first() {
        let mut down = station("down", &["rock"]);
        down.health = Health::Unreachable { last_ok: None };
        let mut low = station("low", &["rock"]);
        low.bitrate = 32;
        let mut unknown = station("unknown", &["rock"]);
        unknown.bitrate = 0;
        let mut kept = station("kept", &["rock"]);
        kept.homepage = Some("https://kept.example".into());
        kept.favicon = Some("https://kept.example/icon.png".into());
        let draw = vec![
            station("plain", &["rock"]),
            down,
            low,
            station("talk", &["talk"]),
            station("on-air", &["rock"]),
            unknown,
            kept,
        ];

        let ids: Vec<String> = candidates(draw, Some("on-air"))
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["kept", "plain", "unknown"]);
    }
}
