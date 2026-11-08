# Project analysis

Audit of the whole project (Rust back end, `nowplaying` crate, Svelte front end, tooling), made on
2026-11-06 and redone on 2026-11-08 (version 0.2.1, ~70 commits later: mini player, share links,
station sheet, tray, update notice, macOS and Windows builds, logos on disk, Surprise me). No
feature was added. Tick an item when its fix lands on `main`.

Overall the project is still in good shape: clippy (`-D warnings`), `cargo fmt --check`,
svelte-check (172 files, 0 problems), ESLint, Prettier and `npm audit` pass; 288 Rust tests. CI is
solid (actions pinned by SHA, cargo-deny, zizmor, DCO, attestations), the production CSP is still
strict, poisoned locks are handled, every `unsafe` block (new Windows and macOS FFI in
`connectivity.rs` and `network.rs`) carries a `SAFETY` comment, all migrations run in a
transaction, and there is no SQL injection.

The new code mostly brought back two kinds of problems already fixed once: database work on audio
or async threads (1.1–1.2) and swallowed errors (1.8). Section 5 lists what this second audit found.

Items marked ✔ were checked twice by hand during the audit.

## 1. Good practices

- [x] **1.1 ✔ Blocking calls in `async` commands.** `play` waited on `recv()` while the sound
  card opened, holding a `std::sync::Mutex`; the whole SQLite database goes through one
  `Mutex<Connection>` used from Tokio threads.
  [commands.rs](src-tauri/src/commands.rs), [audio/mod.rs](src-tauri/src/audio/mod.rs),
  [db/mod.rs](src-tauri/src/db/mod.rs). Done in `3f1412f`: `spawn_blocking` for database and
  engine work, `PlayReply` awaited outside the engine lock. **New code brings it back, see 5.1
  and 5.4.**
- [x] **1.2 The decoder thread wrote to SQLite** (station level saved every 60 s).
  [loudness.rs](src-tauri/src/audio/loudness.rs). Done in `3f1412f`: `WriteBehind` sends writes
  to a `level-writer` thread. **The player listeners still write from the decoder, see 5.1.**
- [x] **1.3 Blocking file reads in the web title tasks.**
  [web_source.rs](src-tauri/src/audio/web_source.rs). Done in `3f1412f`: the recipe text is
  kept when the set is loaded.
- [x] **1.4 ✔ `"csp": null`** while the webview shows remote data (names, favicons, covers).
  [tauri.conf.json](src-tauri/tauri.conf.json). Done in `1a3c336`. Rechecked on 2026-11-08:
  `script-src`/`style-src 'self'`, `object-src`/`frame-src 'none'`; the opener allows only
  `http(s)://*`; the deep-link plugin gets no webview permission.
- [x] **1.5 ✔ Ad hoc migrations.** [db/mod.rs](src-tauri/src/db/mod.rs). Done in `c717012`:
  `MIGRATIONS` run in order, each in a transaction with the `user_version` bump. Rechecked:
  7 migrations, 5–7 additive and idempotent.
- [x] **1.6 Missing indexes** on `listening_history(station_id)` and `likes(history_id)`. Done in
  `fb93c21`.
- [x] **1.7 `AppError::Audio(String)` is a catch-all.** [error.rs](src-tauri/src/error.rs). Done
  in `5df6670`: an `Other` variant and a `kind` field.
- [x] **1.8 One global `player.error`.** Done in `de0a353`: one error per area in
  [errors.svelte.ts](src/lib/errors.svelte.ts), shown by `ErrorBanner`. **New components swallow
  errors again, see 5.5.**
- [x] **1.9 `clear_listening_history` makes two writes without a transaction.** Done in
  `6af415c`. Rechecked: the new multi-row writes (`add_favorite`, `reorder_favorites`,
  `log_structured_track`, `set_track_parts`, `save_track_match`) all use a transaction.
- [x] **1.10 CSV export doesn't neutralize cells starting with `= + - @`.** Done in `e649127`.
- [x] **1.11 Accessibility:** arrow keys on `tablist`, `radiogroup` and the ⋯ menu; focus return.
  Done in `897fec1`. Rechecked: favorites reordering has a keyboard path (↑/↓ and an
  `aria-live` region); the three sheets are native `<dialog>` opened with `showModal()`.
- [x] **1.12 Hard-coded colors.** Done in `43e29a9`: `--color-*` tokens in `app.css`. 7 `#fff`
  and ~12 `rgb()` came back, see 5.9.
- [x] **1.13 Unhandled promises.** Done in `a83851d` and `72d2e42`. A few came back, see 5.9.

Minor:

- [x] `pause()` races between reading the state and writing it. Done in `a6d4061`.
- [x] `Cancel::on_cancel` keeps only one hook. Done in `1724c5e` (but hooks now pile up, see 5.8).
- [x] The failure diagnosis ignores cancellation. Done in `5725a22`.
- [x] Daylight saving time is ignored in the statistics. Done in `845d558`.

## 2. Architecture

All still open on 2026-11-08, and most got bigger.

- [ ] **2.1 `commands.rs` acts as a service layer** (1,215 lines). It still defines `OnAir`,
  `AudioState`, `start_station` and `schedule_click`, plus now `CatalogState`, `WebTitlesState`,
  `CoversState`, the `blocking`/`with_db` helpers and `save_download`. Nine modules import from
  it (lib, share, curated_lists, favorite_repair, media_controls, updates, notifications, volume,
  covers). The recipe file work (`edit_recipe*`, `share_recipe*`, `recipes_folder`, `open_path`)
  is still there. Proposal: a `player.rs`/`state.rs` carrying the logic, commands reduced to thin
  adapters; recipe logic moved to `recipes.rs`.
- [ ] **2.2 `lib.rs` holds business logic.** Partly done in `71e24e5`: `journalize_title`,
  `track_session` and `history_time` moved to `listening.rs` (lib.rs now 483 lines). Left: a
  ~170-line `setup`, and window management (`main_window_shown`,
  `toggle_main_window`, `show_main_window`, `open_main_window`) and `watch_share_links`.
  Proposal: `windows.rs`.
- [ ] **2.3 `audio/web_source.rs` doesn't belong in `audio/`** (1,171 lines; `audio/mod.rs`
  re-exports `WebTitles`, `TitleStatus`, `RecipeTest`, `WebEvent`). Proposal: a `now_playing/`
  module.
- [ ] **2.4 Split `decoder.rs`** (1,028 lines). Partly done: `failure.rs` now holds the failure
  classification and the diagnosis. Left: the retry policy (`run` loop, `blame`,
  `wait_before_retry`), the `Watchdog` and the `Converter` are still in `decoder.rs`; no
  `EngineCtx` (`start_session` and `reopen_lost_output` take 7 parameters, `engine_loop` 5).
- [ ] **2.5 A shared `http` module.** 9 `Client::builder()` in app code (radio_browser,
  musicbrainz, web_source, failure, playlist, decoder, covers/search, covers/logos, daily_list),
  3 User-Agent spellings (`Soundfolk/x`, `Soundfolk/x ( https://… )`, `Soundfolk/x (+https://…)`)
  plus recipe-check's own. Clients are rebuilt per decoder session, per probe, per Internet
  check, per playlist and per `refresh_curated_lists`; `daily_list::client()` is built 5 times.
  17 imports of `stream_download::http::reqwest` although `reqwest` is a direct dependency. The
  module should also own the public-only rule (5.2) and the body size cap (5.10).
- [ ] **2.6 Scattered conventions** (worse):
  - 17 `SETTING_*` constants in 9 files → a typed settings module;
  - event names defined in 7 Rust files (`volume.rs` uses a bare `"player:muted"`) and 5 TS files;
  - `"player:station"` and `StationChange` still defined twice (`favorite_repair.rs`,
    `media_controls.rs`), and the two payloads now differ (only one has `skipped`).
- [ ] **2.7 Hand-written TypeScript types:** `api.ts` declares 40 types by hand for 83 `invoke`
  calls, plus 13 event-name literals. Proposal: `tauri-specta`.
- [ ] **2.8 `SettingsPage.svelte` (836 lines):** 7 near-identical toggle handlers and the switch
  markup copied 7 times → a `toggleSetting` helper and a `SettingSwitch` component; split into
  Privacy, Services, Data. Stores are still changed from outside (`history.version++` from
  `SettingsPage` and `stationNames.svelte.ts`, `ui.personalChanged++` from `SharedSheet`) →
  methods such as `history.cleared()`, `history.renamed()`.
- [ ] **2.9 `MiniPlayer` duplicates `PlayerControls`** (new): status message, like button, skip
  button, volume slider with an inline speaker SVG (against 07 §4, "icons from Icon.svelte"),
  `level`/`percent`; the `refreshLatest` effect is copied in `App.svelte` and `MiniPlayer`.
  `fromFavorite` is written twice, `warningOf` is repeated in `StationSheet`,
  `OrderedSources.svelte` re-implements the `Reorder` class. Proposal: a `playerStatus` derived
  in `onAir`, `LikeButton` and `VolumeIcon` components, shared helpers in `stations.ts`.
- [ ] **2.10 `StationSheet.svelte` (593 lines)** mixes details, renaming and recipe tooling
  (edit, test, share). Proposal: a `RecipeSection.svelte`.

## 3. Dead or obsolete code

All done on 2026-11-06 (`report_station` removed, `RecipeBook` dropped, `Adapter::enriches` used,
`Match` shared, `RadioBrowser` derives `Default`, per-item `dead_code` allowances, narrowed
visibility, `STATION_NAME` joins migrated away). Rechecked: the only `#[allow(dead_code)]` left are
per-platform `cfg_attr` in `media_controls.rs` and `connectivity.rs`; no TODO/FIXME in the code.

New small leftovers, all done in `b496321`:

- [x] Two copies of `is_uuid` ([share.rs](src-tauri/src/share.rs),
  [catalog/radio_browser.rs](src-tauri/src/catalog/radio_browser.rs)): one, in the catalog.
- [x] The "first unused file name" loop written twice in `commands.rs`: one `numbered(base)`.
- [x] `StationRef { id: station_id.filter(is_catalog_id), .. }` built 4 times: `catalog_ref`.

## 4. Other improvements

- [x] **4.1 Tests:**
  - [x] no front-end tests: vitest (5.0.5) runs with `npm test` and in CI since `16d1f09`, first
    on the wheel's steps (`wheel.ts`), `moved` (`order.ts`), `pickRandom`, `hostOf`,
    `checkCustom`, `displayTitle`;
  - [x] `lib.rs` has no tests: its history logic moved to `listening.rs`, with 4 tests;
  - [x] `web_source.rs` had 4 tests: 9 since `b443a5b`, `fetch` against a local server included;
  - [x] the reconnection loop was only covered by `#[ignore]` live tests: since `b443a5b` a test
    runs it without sound card nor network (local server, buffer emptied by the test);
  - [x] new modules: `tray.rs` (labels, RGBA → ARGB), `covers/logos.rs` (size cap, pruning),
    `share.rs` (`is_app_link`, logins, links that read back) have tests now;
  - [x] **two `recipes.rs` tests assumed exactly one shipped recipe**. Done in `cc75783`: counted
    from `BUNDLED`.
- [x] **4.2 Page encoding.** Done in `270c6b3`.
- [x] **4.3 Repeated work:**
  - [x] `ignore` patterns compiled once; cover sources kept in memory (`ee3bf33`);
  - [x] the `extract` regex and CSS selectors were compiled on every read: kept per run since
    `b0db767`.
- [x] **4.4 Duplicated "artist – title".** Done in `4c9d170` for the main paths; the last joins
  (`extract.rs`, `adapters.rs`, `MiniPlayer.svelte`) in `b0db767`, the `share.rs` comment in
  `ffe047c`. (`onAir.track`, "Title – Artist", is a display choice, not this join.)
- [x] **4.5 Busy waits.** Done in `fe5f06c`. The two new ones (the idle engine waking every
  second, Surprise me polling the state every 200 ms) are gone since `3a41f34`.
- [x] **4.6 Front end** (parallel startup, lazy search, day labels). Done in `f1f6ce5`.
- [x] **4.7 Gaps with [06-ui-principles.md](docs/06-ui-principles.md)**, done in `acea4c4`:
  - [x] codec and bitrate in search result rows: rows show the country only;
  - [x] raw Radio-Browser tags shown as genres: gone from rows until tags map to genres;
  - [x] no i18n layer: 06 §7 now says English-only for now, a translation layer being a project
    of its own;
  - [x] tabs inside tabs: 06 §1 now allows History's All / Liked / Stats as the one second level;
  - [x] "Unknown station" in `HistoryPanel.svelte`: nothing is shown;
  - [x] radii back on the 07 §5 scale.
- [x] **4.8 Tooling** (single version, edition 2024, `rustfmt.toml`). Done in `58a729b`; Rust
  1.95 minimum since `7c671c0`.

## 5. Found on 2026-11-08

### Bugs and security

- [x] **5.1 ✔ The player listeners write to SQLite from the decoder and async threads**
  (brings back 1.1/1.2). `Status` calls its listeners under its own lock
  ([audio/mod.rs](src-tauri/src/audio/mod.rs) `replace`, `clear_now_playing`). The state
  listener runs `track_session` (`start_session`/`end_session`), the title listener
  `journalize_title` (several writes) and `station_name` ([lib.rs](src-tauri/src/lib.rs),
  `setup`). They are called from the decoder thread (`reporter.title`, `audible`,
  `stream_tags`), from the 2-worker `audio-net` runtime (`web_source.rs`), and from
  `pause_stream`/`resume_stream` while the engine `std::Mutex` is held. The decoder then waits
  whenever an export or stats query holds the `Mutex<Connection>`. Proposal: a history writer
  thread fed by a channel, like `WriteBehind`. Done in `71e24e5`: a `listening` thread
  ([listening.rs](src-tauri/src/listening.rs)) writes titles, sessions and the heartbeat in order,
  then announces the title; checked on a running build (MPRIS Play/Stop).
- [x] **5.2 ✔ Logo and cover downloads can reach the local network.**
  [covers/logos.rs](src-tauri/src/covers/logos.rs) builds its client without
  `public_net::restrict`, and `covers.rs` uses it for notification covers. Favicons come from
  Radio-Browser (anyone can write there) and covers from station pages, so a URL can make the app
  GET the listener's router or localhost, which docs/16 §6.2 forbids for streams. Proposal:
  `public_net::restrict` on the client and `check()` on the URL, as `decoder.rs` and
  `playlist.rs` do. Done in `e93d98f`.
- [x] **5.3 ✔ A database read error lifts the public-only rule.** `start_station`
  ([commands.rs](src-tauri/src/commands.rs)) does
  `personal_station_from_link(id).unwrap_or(false)`: if the read fails, a station received in a
  link may reach the local network. Done in `e93d98f`: it fails closed, with a warning.
- [x] **5.4 Other blocking SQLite in async code:** the MusicBrainz worker loop (`get_bool`,
  `next_track_to_identify`), `updates.rs`, `save_cover` in `covers.rs`, `set_mini_compact`
  (`mini.rs`), `count_click_if_still_on_air` (`commands.rs`). Proposal: `with_db` /
  `spawn_blocking`. Done: `touch_session` moved to the `listening` thread in `71e24e5`, the
  others go through `with_db` / `blocking` since `e6811bb`.
- [x] **5.5 ✔ Swallowed errors in new components** (brings back 1.8):
  - `SharedSheet.svelte` turns every `getStation` failure into `null`, so a network error reads
    "N of its stations are no longer in the catalog" and the `listError` catch never fires;
  - `ListRow.svelte` and `StationBrowser.svelte` call `lists.toggleFavorite(...).catch(() => {})`:
    a failed list ♡ says nothing (`ListScreen.svelte` handles it properly);
  - the mini window reports to the `history` area, which it doesn't show;
  - `StationSheet` Play and ★ report to `errors.player`, drawn behind the modal sheet.
  Done in `ffb07dd`: list ★ failures and the mini window report in the player area; the sheet
  shows the player's error; a failed read in `SharedSheet` is an error.
- [x] **5.6 Share links** ([share.rs](src-tauri/src/share.rs)):
  - `personal_entry` accepts `https://radio.fr@evil.example/x`, the fake-domain trick `clean()`
    exists to stop; refuse a user name or password, like `track_services::validate`;
  - `make_share_link` never checks the `Shared` item from the webview: a list over 40 stations
    or non-UUID ids makes a link that `parse` then refuses, and `pack_uuids` silently writes
    nothing for non-hex text, although the doc says a link "always reads back";
  - `read_share_link` has no length cap (only `opened()` caps at 16 KiB), so the stations are
    base64-decoded in full before the 40-station check.
  Done in `ffe047c`: logins refused, every written link read back before it is shown, 16 KiB cap
  in `parse`.
- [x] **5.7 ✔ A volume change clears player errors.** `player.setVolume` goes through `run()`,
  which calls `errors.clear('player')` on every slider input and wheel notch
  ([player.svelte.ts](src/lib/player.svelte.ts)): "Nothing playable came up" vanishes at the
  first volume change. Done in `54cf7ae`: volume, mute and saving the volume keep it.

### Good practices and minor

- [x] **5.8 Cancel hooks piled up during a session.** Done in `3a41f34`: hooks are scoped to
  their attempt (`on_cancel_scoped`), a connection keeping its own for as long as its stream.
- [x] **5.9 Front-end leftovers.** Done in `acea4c4`: `--color-on-art*` / `--color-art-*` tokens
  in the mini player; failed subscriptions shown as a player error; the copy menu, clipboard and
  focus cleanup catch; one `hostOf` that never throws; Space and M in the mini player.
- [ ] **5.10 Unbounded response bodies.** Done in `1524f88` for track searches, MusicBrainz, the
  daily lists and logos (`http::body_at_most`). **Left: `radio_browser.rs`** (`response.bytes()`
  in the mirror loop), which another session was changing at the time.
- [x] **5.11 Logos were never evicted.** Done in `1524f88`: logos not fetched again for three
  months, and `.part` files over an hour old, are deleted at startup.
- [x] **5.12 `clear_history` kept `listening_sessions`.** Done in `88651b0`: sessions go in the
  same transaction, and the confirmation lists everything deleted (tracks, listening time,
  Constant volume levels; likes kept).
- [x] **5.13 Small consistency points.** Done in `a87fa9b`: one transaction for the two
  click-counting settings, a default for an unreadable web titles setting, `Database::favorite`,
  the `lists:updated` emit logged, the release page parsed. Not a fix: no media controls on
  Windows (`SystemMedia` is an empty stub), a feature gap.

## Suggested order

1. ~~Security, user-visible bugs, database off the audio threads, sections 3, 4 and 5~~: done,
   except the `radio_browser.rs` body cap (5.10).
2. `http` module (2.5): `body_at_most` and the public-only rule are its first pieces; one User-Agent
   and shared clients are left.
3. The rest of section 2, one module at a time; tauri-specta (2.7) on its own.
