# Project analysis

Third audit of the whole project (Rust back end, `nowplaying` crate, Svelte front end, tooling, CI
and docs), made on 2026-11-10 on version 0.2.2. No feature was added. Tick an item when its fix
lands on `main`.

The project is in good shape. clippy (`-D warnings`), `cargo fmt --check`, svelte-check (210
files, 0 problems), ESLint and Prettier pass; 304 Rust tests pass (21 live tests ignored) and 12
vitest tests. `npm audit` reports two low advisories with no fix (`deepmerge`, pulled in by
`@sveltejs/vite-plugin-svelte`, build time only). CI runs fmt, clippy and tests on Linux, Windows
and macOS for every pull request, plus typos, SPDX, actionlint, zizmor, DCO and cargo-machete;
the bindings test fails when `bindings.ts` is stale.

The main finding is the same pattern the last two audits fixed elsewhere: **SQLite work in code
that runs under the player's `Status` lock or on the async runtime** (1.1, 1.4). Commands are clean
(they all go through `blocking`/`with_db`); the regressions are in listeners and spawned tasks.
The other notable points are a web title task that outlives a failed session (1.2) and web title
probes that are not held to the public-only rule (1.3).

Items marked ✔ were checked twice by hand during the audit.

## Earlier audits

The audits of 2026-11-06 and 2026-11-08 (blocking calls, CSP, migrations, swallowed
errors, the `http` module, typed settings and events, tauri-specta, the split of `commands.rs`,
`lib.rs`, the decoder and the big components) all landed on `main`. Their full list, with the
commits, is in the history of this file (`git log -p docs/99-analysis.md`).

## 1. Bugs and security

- [x] **1.1 ✔ SQLite in the player state listener** (brings back 5.1 of the last audit). The state
  listener ([lib.rs](../src-tauri/src/lib.rs) `start_player`) runs under the `Status` lock, often
  on the decoder thread (`audible`, `failed`), and from `favorite_repair::look_up` with the engine
  `Mutex` held. Two of its steps use the database:
  - `favorite_repair::on_state` → `save_new_address`
    ([favorite_repair.rs](../src-tauri/src/favorite_repair.rs)): `list_favorites` then
    `add_favorite`, a write that also updates `listening_history`;
  - `media_controls::on_state` ([media_controls.rs](../src-tauri/src/media_controls.rs)), when
    skipping a dead favorite: `list_favorites` and `station_name`.

  While an export or a stats query holds `Mutex<Connection>`, the decoder stops filling its
  buffer (the sound drops out) and `pause`, `get_player_state` and the volume wait on `Status`.
  Proposal: send the save to the `listening` thread as a `Job`; for the skip, read the favorites
  inside the `spawn_blocking` that already starts the next one, and show "Skipping…" from what is
  in memory. `favorite_repair::watch` should use `db.favorite(id)` rather than the whole list.
  Done: the new address is saved from a blocking thread (`replace_address`, which reads the one
  favorite with `db.favorite`, as `watch` now does); Next / Previous read the favorites and the
  names the user gave them when pressed, and the skip decides from that copy, with no database
  in the listener.
- [x] **1.2 ✔ After a terminal error, the web titles keep coming.** `decoder/retry.rs` ends with
  `reporter.failed(...)`, which publishes `Error` but does not cancel the session; the `Session`
  lives until the next command ([audio/mod.rs](../src-tauri/src/audio/mod.rs) `EngineCtx::run`).
  The web title task ([now_playing/mod.rs](../src-tauri/src/now_playing/mod.rs) `run`/`follow`) therefore
  keeps polling the recipe page, and `Reporter::web_title` keeps publishing, since
  `update_now_playing` checks only the cancel flag. A recipe station that stops broadcasting at
  night fills the history with titles nobody heard, for hours; the sound output also stays open.
  Proposal: an error ends the session (`failed` cancels it, or the engine drops the session and
  the output); a test that nothing is published after `failed`. Related, smaller:
  `start_session` spawns the web task before `decoder::spawn(...)?` and does not cancel it if
  the decoder thread fails to start.
  Done: `Reporter::failed` publishes the error, then cancels the session; the engine then lets the
  sound output go. A decoder that fails to start cancels the web task.
- [x] **1.3 ✔ Web title probes can reach the local network.** `WebTitles::client` uses
  `http::client()`, not `public_client()`, for `find_server`, `follow` and `icy_details`. The
  probe URLs (`/status-json.xsl`, `/stats`, `/7.html`, `/api/nowplaying`) come from the stream
  address, which anyone can write in Radio-Browser, and the client follows redirects anywhere: a
  catalog entry answering `302 → http://192.168.1.1/…` makes the app GET the listener's router,
  which docs/16 §6.2 forbids. Proposal: `public_client()` and `public_net::check` when the
  session is `public_only`, as the decoder does; the same for recipe URLs from the downloaded list.
  Done: the title task of a `public_only` session uses `public_client()` and checks each address
  (`SessionClient`), recipe pages included.
- [x] **1.4 Database work on Tokio workers** (brings back 5.4):
  - `Covers::resolve` ([covers.rs](../src-tauri/src/covers.rs)) reads the cover sources and
    `cached_cover` synchronously, from the logo task, the end of the lookup task and the async
    command `set_cover_sources`;
  - `mini::open` reads two settings on the main thread ([mini.rs](../src-tauri/src/mini.rs)),
    so the mini window waits for the database lock;
  - `export` reads the whole history and the likes with `u32::MAX` under the connection lock
    ([export.rs](../src-tauri/src/export.rs)), which holds up everything else for seconds on a
    history of several years.

  Proposal: `blocking`/`with_db` around `resolve`; the mini settings read in the command before
  `run_on_main_thread`; the export read in pages, releasing the lock between them.
  Done: `Covers::resolve_later` runs `resolve` on a blocking thread for the three async callers;
  the mini player's size and position are read (`mini::Remembered`) before going to the main
  thread, and its position saved from a blocking thread when it closes; the export reads the
  history 2,000 entries at a time (`Database::history_before`), the lock let go between pages.
  The likes, a short list, are still read at once.
- [x] **1.5 ✔ `is_catalog_id` checks only the length** ([player.rs](../src-tauri/src/player.rs):
  `!starts_with("user:") && len == 36`). A station played from the history without an id uses its
  URL as id; a 36-character URL is then taken for a Radio-Browser UUID: a click is sent for it, it
  is looked up as a recipe station, and a personal LAN station gets `public_only` and is refused.
  The front end answers the same question four ways (`share.svelte.ts` and `StationSheet.svelte`
  with a regex, `api.ts` by length, `StationBrowser.svelte` by prefix). Proposal: the catalog's
  `is_uuid` in Rust, one tested `isCatalogId` in `stations.ts`.
  Done, both. `StationBrowser.svelte` still tests `user:`: there it asks whether a favorite is a
  personal station, another question.
- [x] **1.6 A title of the old station can be logged under the new one.** `start_station` sets
  `on_air` before `engine.play`, and the old session is cancelled only when the engine thread takes
  the command; [listening.rs](../src-tauri/src/listening.rs) reads `on_air` when a title arrives.
  A title of station A arriving in that window is logged and shown as B's. Proposal: the station
  id carried by the session's `Reporter` and sent with each event.
  Done otherwise: `play()` silences the session playing at once (its cancel flag is shared with
  the engine thread), and `start_station` names the new station on air in the same step, under
  the engine lock. Nothing the old station reports afterwards is published, so nothing is logged
  under the new one.
- [x] **1.7 ✔ The History menu shows a success toast after a failure.** `HistoryPanel.svelte`
  wraps `player.toggleFavorite` in `act(…, 'Station added to favorites')`, but `toggleFavorite`
  never rejects: `run()` catches the error and reports it to the player area. Proposal: `run`
  returns whether it succeeded, and the toast shows only then. Done.
- [x] **1.8 Skipped favorites keyed by name.** `PlayerControls.svelte` keys the skipped list by
  `name`, and `Skipped` has no id: a catalog FIP and a personal FIP skipped in a row make a
  duplicate key (`each_key_duplicate`). Proposal: `station_id` in `Skipped`. Done.
- [x] **1.9 `public_net::is_public` misses ranges:** NAT64 `64:ff9b::/96` with a private IPv4
  inside (live on IPv6-only networks), 6to4 `2002::/16`, site-local `fec0::/10`; `192.0.0.0/24`,
  `198.18.0.0/15`, `240.0.0.0/4`. [public_net.rs](../src-tauri/src/http/public_net.rs). Proposal:
  extend the match and its tests. Done, with documentation ranges too; NAT64 and 6to4 addresses
  are as public as the IPv4 address inside.

## 2. Good practices

- [x] **2.1 Blocking file work on the async runtime:** `daily_list::refresh` reads, parses (up to
  8 MiB of JSON), writes and renames inside the async tasks of the recipe list, donations, curated
  lists and updates; `notification_file` and `LogoCache::store` write files from async code.
  Proposal: `spawn_blocking` or `tokio::fs`.
  Done: `daily_list::refresh` reads the ETag, parses and writes on a blocking thread;
  `notification_file` and the logo writes too. Left as is: `is_due` (one `stat`).
- [x] **2.2 The engine thread reads SQLite on every Play** (`WriteBehind::recall`/`guess`,
  [loudness.rs](../src-tauri/src/audio/loudness.rs)): an export delays the station's start and a
  `Stop` queued behind it. Proposal: read the level in `start_station`, already on a blocking
  thread, and pass it in `Command::Play`.
  Done: `start_station` reads the level (`loudness::Recalled`) and passes it in `StationRef`; the
  engine reads the memory only if it was recalled for another station. `WriteBehind` keeps the
  levels written during the run, so a sound output reopened later starts from the latest one
  without reading the database either.
- [x] **2.3 New file names picked with `exists()` then `write`** (`files::save_download`,
  `recipes.rs` new recipe): two exports in the same second can pick the same name and one
  overwrites the other. Proposal: `OpenOptions::create_new` in the `numbered` loop.
  Done: `files::write_new` creates the file with `create_new` and moves on to the next name when
  it is taken; both callers use it.
- [x] **2.4 Swallowed errors:**
  - mini player Next/Previous ([mini.rs](../src-tauri/src/mini.rs)) drops the result: a failed
    skip shows nothing;
  - `lists.svelte.ts`: a failed `getFavoriteLists` silently empties the favorite lists in Listen;
  - `PrivacySettings.svelte`: a failed `getUpdateCheck` leaves the switch on its default;
  - `App.svelte`: a failed `reloadRecipes` on focus silently leaves the edited recipe unapplied.

  Done: `switch_favorite` returns the error, which the skip button already reports; the favorite
  lists report to the station lists, the update check to the settings, the recipe reload to the
  player.
- [x] **2.5 Store fields changed from outside again** (brings back 2.8): `player.recipeNotice =
  null` in `RecipeNotice.svelte`, `player.clickNotice = false` in `ClickNotice.svelte` and
  `PrivacySettings.svelte`. Proposal: `player.dismissRecipeNotice()`, `player.hideClickNotice()`.
  Done, the station sheet too.
- [x] **2.6 `WebTitles::current` is never cleared on Stop:** after Stop, `get_title_status` still
  reports the last station's source, and `recipes_changed` talks about "the station playing".
  Done: the engine calls `WebTitles::stop` on Stop, which forgets the session and sends the idle
  status; the session still running publishes nothing more.
- [x] **2.7 `now_playing::fetch` caps the body by hand,** a second copy of `http::body_at_most`
  without its early `Content-Length` refusal. Done: it calls `body_at_most`.
- [x] **2.8 Reduced motion (07 §7):** `animate:flip` in `OrderedSources.svelte` is a JS animation
  the CSS media query cannot stop; the History toast slides in with a hard-coded `0.15s` and no
  reduced-motion rule. Done: `motion.ts` gives JS animations no duration under reduced motion;
  the toast uses `--duration-fast` and `--ease`, with a reduced-motion rule.
- [x] **2.9 Play/pause drawn outside `Icon.svelte`** (07 §4): `PlayerControls.svelte` has its own
  SVG, and its pause glyph differs from the mini player's. Done: it uses `Icon`.

## 3. Architecture

- [x] **3.1 Large modules.** `now_playing.rs` (1,260 lines): page fetching and charset decoding
  (~120 lines) belong in `http` or a `page.rs`. `audio/mod.rs` (~920 lines of code, ~870 of
  tests): `Cancel`/`ScopedHook` and `Reporter`/`StreamMeta` can each be a file.
  Done: [http/page.rs](../src-tauri/src/http/page.rs) reads a page in its charset with the
  site's clock; [audio/cancel.rs](../src-tauri/src/audio/cancel.rs) and
  [audio/reporter.rs](../src-tauri/src/audio/reporter.rs), each with its tests. The test server
  of the `http` tests is shared (`http::serve`).
- [x] **3.2 Large components.** `StationBrowser.svelte` (494 lines) holds the tab bar, the search,
  favorites with reordering, My stations and the M3U export: a `ListenPanel` and a
  `DiscoverPanel`. `HistoryPanel.svelte` (461 lines) holds the list, the menu with its focus
  handling, the export and the toast: a `TrackMenu`.
  Done: `StationBrowser.svelte` keeps the tabs and the personal stations both panels use;
  `ListenPanel.svelte`, `DiscoverPanel.svelte` and `TrackMenu.svelte` (its styles with it). The
  search moved to `catalogSearch.svelte.ts`, so leaving Discover keeps the text and the results
  as before. The menu listens to the window only while it is open.

## 4. Tests

- [x] **4.1 Back end:**
  - [x] nothing is published after `failed` (1.2);
  - [x] the station of a late title (1.6): `play()` silences the session playing before the
    request leaves; the order in `start_station` itself is not tested;
  - [x] the favorite's new address replaces the old one only in a favorite still there; a skipped
    favorite named as the user knows it, from the copy taken when Next was pressed (1.1);
  - [x] the export read by pages is the whole history (1.4); a playback starts from the level
    recalled before it, and a level learned during the run wins (2.2).
- [x] **4.2 Front end:** pure logic still inside components, without tests: `quality()`
  (StationSheet), `utc()` and the day labels (HistoryPanel), `sharedOf` and `LIKE_A_LINK`
  (share), `target()` wrap-around (a11y), `lists.matching`, `recipeRequestLink`, the `onAir`
  title rules. `quality()` moved to `stations.ts`, `utc()` and `dayLabel()` to `days.ts`, the
  title rules to `onAirTrack.ts`; the stores are tested as they are (vitest loads `.svelte.ts`).

## 5. Minor

- [x] Missing partial index for open sessions: the heartbeat, start and stop run `WHERE ended_at
  IS NULL` on `listening_sessions` every minute, a full scan that grows forever. Migration 8:
  `CREATE INDEX … ON listening_sessions(ended_at) WHERE ended_at IS NULL`.
- [x] `toggle_mute` reads, then locks again to write ([volume.rs](../src-tauri/src/volume.rs)):
  two quick tray toggles can set the same value. Flip under one lock.
- [x] Startup work before the first window: `recipe_list::start` parses the cached list on the
  main thread, and a history re-parse runs in `Database::open` when the parser changes.
- [x] `aria-controls="panel-<tab>"` on all three tabs of `StationBrowser.svelte`, but only the
  selected panel exists.
- [x] The mini player's list toggle combines `aria-pressed` with a label that flips ("Show the
  favorites, pressed"); keep one fixed label.
- [x] `OrderedSources.svelte` announces the position twice (handle label and live region).
- [x] Radii off the 07 §5 scale: `StatsPanel.module.css` (`4px`, twice), `SettingsPage.module.css`
  (`0.25rem`).
- [x] docs/07 §3 allows weights 400/500 and a size scale; the players use 600 and `0.9375rem`.
  One of the two should change. §3 now names the players' exception, which §6 already described.

## 6. Tooling, CI and docs

- [x] **6.1 ✔ A release tag does not wait for CI.** `release.yml` builds and publishes on a `v*`
  tag without checking that `ci.yml` passed for that commit, and runs no tests itself. Proposal:
  the `version` job fails unless the commit's CI run succeeded (`gh api …/check-runs`), or the
  release calls `ci.yml` through `workflow_call`.
- [x] **6.2 Release builds without `--locked`:** the three `npm run tauri -- build` commands let
  cargo re-resolve, against the workflow's own comment ("what ships must come from the lockfiles
  alone"). Add `-- --locked`.
- [x] **6.3 Node version:** README says Node ≥ 20; vitest, vite and ESLint need 22.12+. Say so,
  and add `"engines"` to `package.json`.
- [x] **6.4 README describes an older app:** titled "Linux MVP" though Windows and macOS ship; 4
  tables of 13, 2 events of 13, `player:state` without `reconnecting`; "no title for Ogg
  streams" although Vorbis comments are read now. Proposal: link to docs/02 and `events.rs`
  rather than repeat them.
- [x] **6.5 docs/03-database.md** describes 9 of the 13 tables (`covers`, `station_loudness`,
  `station_names`, `station_reports` missing, or only described elsewhere).
- [x] **6.6 TODO.md** still lists volume normalisation, sharing and album art as open (all done),
  and search filters by codec and bitrate, which 06 §5 decided against.
- [x] Small points: `CONTRIBUTING.md` says actionlint and zizmor run "when a workflow changes" (they
  run on every PR); `libssl-dev` installed in CI and release though TLS is rustls; no PR template
  reminding `git commit -s` and `cargo test bindings`.

## Notably solid

- Cancellation (`Cancel` and `ScopedHook`, `Reporter` refusing to publish once cancelled), well
  tested; the public-only rule on streams, checked at DNS resolution and on every redirect.
- Remote data never reaches a path unchecked (`is_valid_id`, `is_safe_id`); no `unwrap` on network
  data; capabilities minimal and CSP strict; migrations append-only and transactional.
- Late answers dropped consistently in the front end (`requestId`, `current` flags); `player.init`
  subscribes with `Promise.allSettled`; `Reorder` serves both reorderable lists with keyboard and
  live region.
- Release supply chain: no cache in release jobs, pinned tool hashes, provenance attestations,
  signature and architecture checks on macOS; `deny.toml` and Dependabot exceptions each say why.

## Suggested order

1. ~~Off the `Status` lock and the runtime: 1.1, 1.4, 2.1, 2.2, with the tests of 4.1~~: done.
2. ~~Sessions and network: 1.2, 1.3, 1.6, 1.9, then 1.5~~: done.
3. ~~Release safety: 6.1, 6.2~~: done.
4. Front-end bugs and leftovers: ~~1.7, 1.8~~ (done), 2.4, 2.5, 2.8, 2.9.
5. ~~Docs (6.3–6.6), then section 3 one module at a time~~: done.
