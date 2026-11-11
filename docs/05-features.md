# 🚀 Feature Catalog

This document lists the functional and technical features of **Soundfolk** (`soundfolk.fm`), from the local MVP
onwards.

---

## 1. 🎵 Audio core & player
* **Continuous streaming (ICY / HTTP):** smooth playback of web radio streams over standard broadcast protocols.
* **Live metadata:** the artist and title currently on air, recorded in the listening history. They come from the
  stream (ICY, or Vorbis comments in Ogg streams) or, for streams that announce nothing, from the station's web page
  (recipes) or its streaming server's status page (Icecast, Shoutcast); the next track is shown when the page announces
  it. Reading websites can be turned off in ⚙ (see [08-now-playing.md](08-now-playing.md)).
* **Resilient network buffer:** absorbs network micro-cuts; automatic reconnection if the stream drops.
* **Fast start:** measured at roughly half a second to the first decoded sound on an Icecast stream (network-bound).
* **Background playback:**
  * Desktop: 100 % Rust engine (`symphonia` + `cpal`).
  * Mobile (future): native `AVPlayer` (iOS) and `ExoPlayer/Media3` (Android).
* **Launch at login (desktop):** off by default, turned on in ⚙. Soundfolk then starts in the notification area,
  without its window (and so without its web view) and without playing; the last station is loaded, so Play in the
  tray menu or on the media keys starts it. If no notification area icon can show, the window opens after all.
* **Next / Previous favorite (desktop):** in the tray menu and on the media keys. A favorite proven not to broadcast
  before its first sound (404, server refusing connections… while Internet works) is skipped, since whoever zaps
  from there is not looking at the window; the window then lists what was skipped and why. Without proof (no network,
  no Internet) the player waits as usual; a favorite gone from the catalog stops there so that the window can offer
  to remove it; after a full turn without sound, the last error is shown. A station chosen in the window is never
  skipped.
* **Tray menu (desktop):** show / hide the window, mini player, Play / Pause, Mute / Unmute, Next / Previous
  favorite, Quit. Left click shows or hides the window, middle click plays or pauses. The mute is held by the
  backend (`volume.rs`), so the tray, the window and the mini player always agree; moving the slider unmutes.
* **Keyboard shortcuts (main window):** Space plays or pauses, M mutes, Ctrl+F or Ctrl+K go to the search
  (Discover), Up / Down move between the stations of a list, keeping the same button of the row. None of them takes a
  key from a text field, and Space leaves a focused button, tab or slider its own meaning (`src/lib/shortcuts.ts`).
* **Track notifications (desktop):** off by default, turned on in ⚙. Each new track in the history (not a repeated
  title, nor the station's name between tracks) shows a desktop notification while the window does not have the
  focus: title, then artist and station. On Linux each one replaces the previous one and is transient, so a long
  listen does not fill the notification list.
* **Playback resumption (mobile, future):** the last station stays in the system media controls (Android Quick
  Settings and lock screen, Android 13+ through Media3's `onPlaybackResumption`; iOS Now Playing) after the app is
  closed, and Play there starts it without opening the app. On by default; can be turned off in the settings, effective
  immediately.

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
  again or add it to favorites. The history can be cleared at any time, after a confirmation that lists what goes:
  the tracks, the listening time behind the stats and the levels learned by Constant volume; likes are kept.
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
* **Personal stations:** your own streams, named by you, kept alongside catalog stations (marked "personal").

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
* **New versions (desktop, on by default):** once a day, Soundfolk downloads `latest.json` from the latest GitHub
  Release (the version number and its release page; GitHub sees the IP address). When it is newer, a dot on ⚙ and a
  line in Settings › About link to the release page, with **Skip** for that version. Nothing is downloaded or
  installed: the listener updates the way they installed. Once Soundfolk's APT repository is set up (17), a `.deb`
  subscribes the system to it, and the line then says that Software Updater will install it. Never checked by a nightly, a
  Flatpak or Snap, or a build whose packager set `SOUNDFOLK_NO_UPDATE_CHECK`, since their package manager updates
  them. Turned off in ⚙ › Privacy.
* **No telemetry** beyond that.
