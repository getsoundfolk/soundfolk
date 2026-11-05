# 🎶 Now playing

How Soundfolk finds the track on air, including for streams that do not announce it.

## 1. Where the title comes from
For each playback, the first source that gives a title wins:

1. **The stream itself**: ICY `StreamTitle`, or Vorbis comments in Ogg / Opus / FLAC streams (Icecast starts a new
   Ogg chain at each title change; the decoder reads its tags and carries on without reconnecting). As soon as the
   stream announces a title, the web sources below stop.
2. **A local recipe**: one the user wrote, in `<config dir>/recipes/*.toml` (`~/.config/fm.soundfolk.app/recipes`
   on Linux). Read at each playback, so a change applies from the next play.
3. **A bundled recipe**: `src-tauri/recipes/stations/*.toml`, embedded in the app at build time.
4. **The streaming server's status page**, when the stream carries no metadata, or has announced nothing 20 s after
   opening: Icecast `/status-json.xsl`, Shoutcast `/stats?sid=1&json=1` or `/7.html`. The adapter is used only if the
   document has the expected shape; the result is cached per server for the session.

`NowPlaying.source` says which one (`icy`, `stream_tags`, `recipe`, `server`). Recipes and status pages are read only
while the station plays. They can be turned off in ⚙ › Privacy ("Find the track on air on the station's website"):
the station's site sees the listener's IP address.

When the source gives the artist and title apart (recipes, Vorbis comments), `NowPlaying.track` carries them, the
history stores them as given (`listening_history.title_source = 'source'`, never re-parsed) with the real start time,
and the system media controls show them as is. `NowPlaying.next` is the announced next track ("Up next" in the
player).

## 2. Recipes
One TOML file per station, data only (no scripting, one URL). The JSON Schema is
[`src-tauri/recipes/recipe.schema.json`](../src-tauri/recipes/recipe.schema.json) (Taplo uses it through the
`#:schema` line).

```toml
#:schema ../recipe.schema.json
schema = 1
name = "Fréquence 7"
match = { streams = ["live.aurafm.org/Frequence7"] }   # and/or stations = ["<Radio-Browser UUID>"]
fetch = { url = "https://www.frequence7.net/player/" } # every = "30s": only a fallback hint
ignore = ["^Jingle"]                                   # titles that are not tracks

[extract]
type = "html"                 # html (CSS selectors), json (dotted paths) or text (one regex)
item = "li.event_0"           # the element holding the track on air
artist = ".artist"
title = ".title"              # or text = "…" for "Artist - Title" in one value
start = ".played-at"          # Unix time, ISO 8601, or the station's local HH:MM:SS
duration = "@data-duration"   # `@attr` = attribute of the item; `sel@attr` = of a child
next = "li.event_1, li.event_2, li.event_3"

[[test]]                      # replayed by the tests and `recipe-check test`
page = "../fixtures/frequence7.html"
at = "2026-11-04T21:39:08Z"
artist = "BLACK RAINBOWS"
title = "Children Of Fire And Sacrifices"
started_at = "2026-11-04T21:36:51Z"
next_title = "Nino"
```

* `match.streams` are written without the scheme, compared case-insensitively; a trailing `*` matches a prefix.
* Fixtures keep only the useful fragment of the page (third-party content, excluded from the recipes licence).
* A recipe declaring a newer `schema` than the app reads is skipped, not an error.

### `recipe-check`
```bash
cargo run -p soundfolk-nowplaying --bin recipe-check -- test recipes/stations/*.toml
cargo run -p soundfolk-nowplaying --features cli --bin recipe-check -- run recipes/stations/frequence7.toml
cargo run -p soundfolk-nowplaying --bin recipe-check -- compile recipes/stations -o recipes.json
cargo run -p soundfolk-nowplaying --bin recipe-check -- schema > recipes/recipe.schema.json
```
(from `src-tauri/`). `run` without `--page` fetches the live page (needs `--features cli`).

## 3. Engine (`src-tauri/crates/nowplaying`)
A crate with **no I/O**, so a server daemon or WebAssembly can reuse it: the host fetches pages and passes them in.

* `recipe`: format, validation, matching. `extract`: HTML / JSON / text. `adapters`: Icecast, Shoutcast.
* `clock`: local `HH:MM:SS` → instants without knowing the time zone. Real UTC offsets are multiples of 15 min; the
  current track started at most its length before the page's HTTP `Date`, which leaves one offset when the length is
  under 15 min (example: `Date` 21:39:08 UTC, track 22:36:51 → 22:41:27 local ⇒ +1:00). Otherwise the offset is
  learned from the first change seen between two requests. It is checked on every request and learned again when it
  stops fitting (daylight saving time).
* `tracker`: what to show and when, and when to read the page again.

### Scheduling
* **Limits enforced by the engine**, whatever the recipe says: at least 15 s between requests, at most 120 per hour,
  0–5 s of random jitter, backoff on failures (30 s doubling up to 5 min).
* **End of the track**, in order: the announced start of the next track (it includes planned gaps), else start +
  duration; the next request is 5 s after it. Upcoming tracks are **scheduled ahead** and shown on time without a
  request; the request after each change confirms it (live shows, schedule changes). About one request per track.
* **Track running over** (page unchanged after the expected end): 15 s, 30 s, then every 60 s.
* **Nothing known**: every 30 s (or the recipe's hint), slowing by half while nothing changes, up to 60 s.
* The title changes when the listener **hears** the new track: start + 3 s of estimated audio latency. Until then,
  gaps between tracks included, the previous title stays.

## 4. Not done yet
* The learned UTC offset is kept in memory for the app's lifetime, not in the database.
* Time zone hint from the station's country (Radio-Browser) and measured playout clock drift.
* Platform adapters (AzuraCast, RadioKing…), detection from the station's homepage, push (SSE / WebSocket).
* The shared recipe list (`getsoundfolk/soundfolk-recipes`, ODbL) and its remote loading; the element picker.
