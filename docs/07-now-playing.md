# 🎶 Now playing

How Soundfolk finds the track on air, including for streams that do not announce it.

## 1. Where the title comes from
For each playback, the first source that gives a title wins:

1. **The stream itself**: ICY `StreamTitle`, or Vorbis comments in Ogg / Opus / FLAC streams (Icecast starts a new
   Ogg chain at each title change; the decoder reads its tags and carries on without reconnecting). As soon as the
   stream announces a title, the web sources below stop.
2. **A local recipe**: one the user wrote, in `<config dir>/recipes/*.toml` (`~/.config/soundfolk/recipes`
   on Linux). See *Writing a recipe in the app* below.
3. **A shared recipe**: the list compiled from [`getsoundfolk/soundfolk-recipes`](https://github.com/getsoundfolk/soundfolk-recipes)
   (ODbL), downloaded once a day. See *The shared list* below.
4. **A bundled recipe**: `src-tauri/recipes/stations/*.toml`, embedded in the app at build time (a snapshot used
   offline before the first download).
5. **The streaming server's status page**, when the stream carries no metadata, or has announced nothing 20 s after
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

* A recipe's **id** is its file name without `.toml`. Layers, highest first: local, shared, bundled. A recipe
  replaces the ones with the same id below it, whatever their `match` says; otherwise the first layer whose recipe
  matches wins. A local file that does not load is reported, and the recipe below it stays in use.

### The shared list
New recipes and fixes go to [`getsoundfolk/soundfolk-recipes`](https://github.com/getsoundfolk/soundfolk-recipes)
(its `README.md` is the contributor guide, `ARCHITECTURE.md` the design). Its CI replays the fixtures with
`recipe-check` built from this repository's `main`, then compiles every recipe, without tests, into
`https://getsoundfolk.github.io/soundfolk-recipes/v1/recipes.json` (`{"schema": 1, "recipes": [{"id": …, …}]}`;
`v1` is the list format, each recipe keeps its own `schema`).

`src-tauri/src/recipe_list.rs`: the cached list (`<cache dir>/recipes/recipes.json`) is used at startup; it is
downloaded again when older than 24 h, with `If-None-Match`, only while web titles are on (GitHub sees the IP
address). A list that does not parse is ignored; entries this version cannot read are skipped one by one. The
station playing moves to its new recipe without interrupting the sound. `SOUNDFOLK_RECIPES_URL` overrides the
address (a fork, a local server). *Edit locally* on a shared recipe writes it as TOML (the list has no comments).

`src-tauri/recipes/stations` stays a copy of a few recipes of the shared list, refreshed by hand before a release.

### Writing a recipe in the app
Everything starts from the station, not from a list of recipes (no visible maintenance, see `06-ui-principles.md`):

* **ⓘ in the player** opens the station sheet. *Track on air* says where the title comes from (the stream, a recipe
  and which one, the server's status page, nothing), the last reading error, and a local file that does not load.
* **Edit locally** copies the recipe in use into the local folder (schema line pointing next to it, fixtures dropped)
  and opens it in the system editor. Without a recipe, **Create a recipe** writes a template with `match` already
  filled in (Radio-Browser UUID and stream) and the station's website as `fetch.url`, and explains the rest. The
  folder also gets `recipe.schema.json` for completion in editors that read `#:schema` (Taplo).
* **Save, then come back to Soundfolk.** The folder is never polled: recipes are parsed once, kept in memory and
  reloaded (only the files whose date or size changed) when the window gets the focus back, when a station starts and
  before a test. The station playing switches to the new recipe without interrupting the sound (the minimum interval
  between requests still holds). A notice then says what the recipe reads, or why the file is not used, or that it
  does not apply to the station playing.
* **Test** reloads the recipes, reads the page now and shows what was extracted: track, start, next track, show, the page's UTC offset,
  or the step that failed.
* ⚙ › Privacy › **My recipes** counts the local files, lists those not used and opens the folder.

### `recipe-check`
```bash
cargo run -p soundfolk-nowplaying --bin recipe-check -- test recipes/stations/*.toml
cargo run -p soundfolk-nowplaying --features cli --bin recipe-check -- run recipes/stations/frequence7.toml
cargo run -p soundfolk-nowplaying --bin recipe-check -- compile recipes/stations -o recipes.json  # ids = file names
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
* **TODO, time zone.** Until then, a page that gives only start times (no duration, no next start) shows titles
  without their real start time until a track change has been seen. The learned UTC offset is kept in memory only.
  To resolve local times from the first fetch, in this order:
  1. the time zone of the station's country (Radio-Browser `countrycode`), for countries with a single one;
  2. an optional IANA `timezone` field in the recipe (e.g. `"Europe/Paris"`, never a fixed offset, which daylight
     saving time breaks), for countries with several; `recipe-check test` checks it against the fixture and
     `recipe-check run` suggests it;
  3. the current inference, as a last resort and to check 1 and 2 on every fetch.
  This also makes storing the learned offset unnecessary.
* **TODO, drift.** A page's times are not when the listener hears the track: the station's playout clock can be off,
  and the delay from studio to speaker is often 5–30 s, while the engine assumes a fixed 3 s (`AUDIO_LATENCY`). To
  measure it:
  * on streams that also send ICY titles, compare their changes with the page's times;
  * otherwise, an optional `delay` field in the recipe, measured by the contributor.
  The time zone does not help here.
* Platform adapters (AzuraCast, RadioKing…), detection from the station's homepage, push (SSE / WebSocket).
* The element picker.
