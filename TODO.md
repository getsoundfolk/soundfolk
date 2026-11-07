# TODO — Listening history, likes & stats

Goal: reliable personal stats (stations and artists whose tracks I like most, and more). Each batch builds on the
previous one.

**Principles**
* The raw ICY title in `listening_history` is never altered: everything else is derived from it and can be recomputed.
* The station is identified by its ID (Radio-Browser UUID or `user:…`), never by its name.
* No manual correction of parsed titles (too complex for now). Stats rely on safe data instead: artist and album
  rankings only count tracks recognised by MusicBrainz, and show the number of unrecognised tracks next to them.

**Target model**
```
listening_history  (raw title + parsed artist / title)  ── track_id (via MusicBrainz) ──▶ tracks ──▶ artists, albums
likes              (history entry, station_id, liked_at)
listening_sessions (station_id, started_at, ended_at)
```

---

## Batch 2 — Liked tracks & title parsing
To do early: the station a track was liked on cannot be recovered later.

- [x] `likes` table: history entry, `station_id` at the time of the like, `liked_at`.
- [x] ♥ on each history row, and in the player next to the title on air.
- [x] "Liked" filter in the History tab.
- [x] Parse the raw title into `artist` + `title` columns, at logging time (Rust):
  - strip a trailing station signature (segment after ` - ` or ` | ` containing a domain, "on air", or a name close to
    the station's, e.g. `- Classic Vinyl on walmradio.com`);
  - `Artist - Title` (ICY convention) if a ` - ` remains, otherwise split on the **last** ` by ` (`Title by Artist`);
  - no separator: everything is the title, artist empty.
- [x] Re-parse existing rows at startup (safe: the raw title is kept).
- [x] Search query cleaned of noise (`(Remastered 2011)`, `[Live]`, `(Radio Edit)`, `/`, `:`, `!`); "Copy title"
  copies `Artist - Title`.
- [x] Display: title in bold, `Artist · Station · time` below.
- [x] Tests on ~20 real raw titles (walmradio format, `Artist - Title`, "Stand By Me by Ben E. King", station
  suffixes, titles without a separator).
- [x] Station favorite from a history row (stream URL resolved via Radio-Browser by UUID on click).
- [x] Text search in the history.

## Batch 3 — Listening sessions
Also to do early: without it, listening time and like rates cannot be computed for past days.

- [x] `listening_sessions` table: `station_id`, `started_at`, `ended_at` (start on play, end on stop, station change,
  error or app exit).
- [x] Close sessions left open by a crash at the next startup (at the last one-minute heartbeat).

## Batch 4 — MusicBrainz enrichment
Can come later without losing anything: it works from the raw titles already stored. Only **liked** tracks are
identified (all the stats need), and only once the setting is turned on in ⚙ (off by default: titles leave the device).

- [x] Look up `artist` + `title` on MusicBrainz (free, no key, 1 request/s, `User-Agent` required).
- [x] `tracks`, `artists`, `albums` tables keyed by MBID (recording, artist, release group — not a specific release,
  so compilations do not skew the stats).
- [x] Release date, tags (genres), artist country.
- [x] Resolution status per track: `parsed` / `matched`.
- [x] Background queue, offline-tolerant (retries when the network is back).

## Batch 5 — Stats screen
- [x] Stations: likes, and **like rate** (likes ÷ tracks heard, likes per listening hour) — raw counts favour the
  station listened to most.
- [x] Artists, albums: tracks liked (MusicBrainz-recognised tracks only, with the unrecognised count shown).
- [x] Discovery: stations that brought new artists (first like of an artist), new artists liked per month, tracks heard
  on several stations.
- [x] Listening profile: time per station, day of week and hour; decades and genres of liked tracks; artist countries.
- [x] Station quality: repetition (same track per week), diversity (distinct artists per 100 tracks), share of
  unrecognised titles (ads, jingles, missing metadata).

Caveat: a title in the history means it was **announced** while listening, not that it was heard in full. Real
listening stats must be crossed with the sessions.

---

## Ideas

**Worth doing**
- [ ] Scrobbling to ListenBrainz (user token pasted in ⚙, no OAuth), then Last.fm. A like becomes a "love". Only
  submit a track after ~half of it or 4 min on the same station (a title is announced, not necessarily heard; use
  the sessions). Off by default, with a text saying what leaves the device.
- [ ] Volume normalisation across stations (loudness differs a lot, switching station jumps the level). No ReplayGain
  on streams: a per-station gain learned while listening and stored in the DB, or a slow AGC on EBU R128 loudness in
  the engine.
- [x] Track notifications while in the background (setting in ⚙, off by default).

**Smaller**
- [ ] Make the tray optional; check GNOME without the AppIndicator extension (closing may hide the window with no
  icon to bring it back).
- [ ] Setting to use the system title bar (window decorations).
- [x] Launch at login (`tauri-plugin-autostart`), hidden in the notification area, without playing.
- [ ] Audio quality Low / Auto / High: pick among a station's streams by bitrate (Radio-Browser often lists several).
- [ ] When a stream fails, fall back to another stream of the same station (a similar station: see
  `docs/10-discovery.md` §6.1).
- [ ] Drop `tauri-plugin-clipboard-manager` (~10–15 crates fewer, mostly `image`, for text only): a small `copy_text`
  command on `arboard` with `default-features = false, features = ["wayland-data-control"]`. Not
  `navigator.clipboard`: WebKitGTK only allows it right after a click in the page, so "Copy" from the native
  right-click menu (`main.ts`) and the recipe fixture copied after a page fetch (`commands.rs`) would be refused.
  Retest on X11 and Wayland. Low priority: build time and binary size only.
- [ ] Clock drift between the station's server and the sound card (desktop engine). Nothing compensates it: at a
  typical 20–100 ppm the connect burst (3–5 s) runs out after ~10–70 h of unbroken listening (a 1 s gap, then
  rebuffering), or the lag builds up until Icecast drops the client. Reconnects (sleep, network change) reset it.
  **Measure first**: log the buffer fill every minute, play two or three stations overnight (one at 44.1 kHz, one
  at 48 kHz). If it moves by more than ~0.5 s/h, or gaps show up with no network cause: replace `rubato::Fft` by
  rubato's high-quality sinc `Async` resampler, its ratio nudged by a few ppm from the buffer fill (not `Slip`,
  the cheap variant). Desktop only: on mobile `AVPlayer` / `ExoPlayer` keep the hardware decoding (battery).
- [ ] Suggest corrections to a station's catalog entry. Done and parked on branch `radio-database-corrections`
  (station sheet → "Suggest a correction": checks stream, website and logo, fills in what it finds, opens GitLab
  with radio-database's JSON file). Parked because the channel is barely processed (checked 2026-11-05): no merge
  request merged since May 2025 (227 open), 37 of 41 issues open, the maintainer commits from other sources; and a
  non-member needs a fork first (GitLab's new-file page answers 404 until then). Resume if the maintainer says
  which channel he reads (his FAQ also gives an e-mail). Lighter variant: keep the checks, then link to
  https://www.radio-browser.info/owners (stream owners' `icy-*` headers, applied within 24 h) and to a prefilled
  issue (`/-/issues/new?issue[title]=…&issue[description]=…` works without a fork).

**From desktop radio player feedback (r/selfhosted, r/linux…)** — not yet in the app:
- [x] Tray menu: play / pause, mute, switch between favorite stations.
- [x] Reconnect after suspend or a network change, however long it lasted (network watcher, see
  `docs/04-audio-engine.md`). Still to check on a real suspend / Wi-Fi → 4G switch.
- [x] Logarithmic volume curve: the gain is the cube of the slider position (`audio::gain`).
- [x] Keyboard shortcuts: Space play / pause, M mute, Ctrl+F / Ctrl+K search, arrows to move between stations
  (main window only; the mini player has none yet).
- [ ] Album art for the track on air (Cover Art Archive, for MusicBrainz-matched tracks), in the player and MPRIS;
  station logo otherwise.
- [x] Export the history (CSV / JSON).
- [ ] Search filters by codec and bitrate (country and tag already exist).
- [x] Follow the system light / dark theme: Settings › Appearance, System / Light / Dark (`src/lib/theme.ts`).
- [x] Mini player (`mini.rs`, `MiniPlayer.svelte`): an always-on-top window that stands in for the main one
  (which closes: one web view at a time). Full (560 × 180: cover, track, volume, ♥, favorites or recent titles,
  previous / play / next favorite) or compact (320 × 88); size and position kept. From the player's header or the
  tray menu; back from its own button, the tray or a window manager close.
- [ ] Sleep timer in the player: stop after 15 / 30 / 60 min, the volume fading out over the last minute.
- [ ] Import favorites and personal stations (M3U, JSON); the M3U export exists (Listen → Export M3U). Personal
  stations kept as they are, Radio-Browser stations matched by UUID, else by stream.
- [ ] Measure memory use and keep a target (< ~50 MB idle while playing) — the main argument against Electron
  players.

**UI pattern to reuse**: ordered sources (drag to reorder, a switch each, a type badge, "a disabled source is never
queried") — for "Find this track on" or recipe sources. If lyrics come one day, LRCLIB plain text is enough (no
playback position on radio, so no reliable sync).
