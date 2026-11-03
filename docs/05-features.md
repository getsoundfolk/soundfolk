# 🚀 Feature Catalog

This document lists the functional and technical features of **Soundfolk** (`soundfolk.fm`), from the local MVP
onwards.

---

## 1. 🎵 Audio core & player
* **Continuous streaming (ICY / HTTP):** smooth playback of web radio streams over standard broadcast protocols.
* **Live metadata:** the artist and title currently on air, extracted from ICY metadata and recorded in the listening
  history.
* **Resilient network buffer:** absorbs network micro-cuts; automatic reconnection if the stream drops.
* **Fast start:** measured at roughly half a second to the first decoded sound on an Icecast stream (network-bound).
* **Background playback:**
  * Desktop: 100 % Rust engine (`symphonia` + `cpal`).
  * Mobile (future): native `AVPlayer` (iOS) and `ExoPlayer/Media3` (Android).

---

## 2. 🔍 Catalog & discovery
* **Global coverage via Radio-Browser:** worldwide catalog (independent and commercial web radios), searched online by
  name, with a short typing delay to spare the API.
* **Honest station health:** stations whose last check failed stay visible with a warning (the check can be a false
  negative) and are ranked after healthy ones. HLS stations are not shown until they are supported.
* **Add a station by URL:** direct stream, `.m3u` or `.pls`. The stream is tested before it is saved. Private to the
  device by default; nothing is sent anywhere.

---

## 3. ⭐ Personal curation
* **Favorites:** quick pinning of preferred stations, stored locally in SQLite.
* **Listening history:** automatic, timestamped record of the tracks played on air, so a discovery is never lost. It
  is the first tab, grouped by day, filterable by title, artist or station, and records the station's name with each
  title. Titles that only repeat the station's name are skipped. Each raw title is split into artist and title, after
  removing a station signature such as `- Classic Vinyl on walmradio.com`. From each track: find it on YouTube, YouTube
  Music, Spotify, Deezer, Apple Music, Bandcamp or Discogs (a plain search link with the cleaned artist and title,
  opened in the browser), copy `Artist - Title` (also by double-click), copy a ready-to-share line, play the station
  again or add it to favorites. The history can be cleared at any time; likes are kept.
* **Liked tracks:** ♡ on each history row and next to the title on air. A track is liked once, whatever station plays it
  again later, and remembers the station it was liked on. The "Liked" view lists them.
* **Listening sessions:** when each station was actually playing, so listening time is known (a title in the history
  only says it was announced).
* **Stats** (History tab, over 7 days, 30 days or all time):
  * stations ranked by likes or by **likes per hour of listening** (fairer: the station listened to most is not
    always the one liked most), with the share of announced tracks liked;
  * artists, albums, genres, decades and artists' countries of liked tracks — only tracks identified on MusicBrainz
    are counted, and the number left out is shown;
  * where artists are discovered (station of each artist's first like), new artists liked per month, tracks heard on
    several stations;
  * listening time by hour of the day and by day of the week;
  * station profiles: repetition (plays per distinct track), diversity (distinct artists per 100 tracks) and the share
    of titles without an artist (ads, jingles, missing metadata).
* **Personal stations:** your own streams, kept alongside catalog stations (marked "personal").

---

## 4. 🔒 Privacy
* **Listening count (on by default):** when a catalog station starts playing, Soundfolk tells Radio-Browser, which uses
  it for popularity rankings. It sends the station UUID and, as with any request, the IP address — nothing else. A short
  notice appears at the first play (that first play is not counted), and the setting can be turned off at any time.
  Personal stations are never reported.
* **Track identification (off by default):** when turned on in ⚙, the artist and title of **liked** tracks — never the
  rest of the history — are looked up on MusicBrainz, the open music database, to get the canonical artist, album,
  release date, genres and artist country used by the stats. As with any request, the IP address is visible to
  MusicBrainz. One request per second at most; lookups wait while offline and resume later.
* **No telemetry** beyond that.
