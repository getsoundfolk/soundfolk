# 🏛️ Technical Architecture

## 1. Stack overview
Soundfolk strictly decouples the user interface, the performance-critical logic and the data.

```
┌───────────────────────────────────────────────┐
│         Svelte 5 frontend + CSS Modules       │
│            (UI, reactive state, runes)        │
└──────────────────────▲────────────────────────┘
                       │ Tauri IPC (commands & events)
┌──────────────────────▼────────────────────────┐
│                 Rust backend                  │
│ ┌──────────────┐ ┌───────────┐ ┌────────────┐ │
│ │ Audio engine │ │  Catalog  │ │   Local    │ │
│ │ (Symphonia / │ │ (Catalog- │ │  database  │ │
│ │  CPAL)       │ │  Source)  │ │ (rusqlite) │ │
│ └──────┬───────┘ └─────┬─────┘ └────────────┘ │
└────────┼───────────────┼──────────────────────┘
         │               │
   HTTP/ICY streams   Radio-Browser API
```

## 2. Layers

### Frontend (Svelte 5)
* **Runes** (`$state`, `$derived`) for fine-grained, modern reactivity. Shared player state lives in
  `src/lib/player.svelte.ts`.
* **CSS Modules** for strict per-component style isolation (no heavy CSS framework).
* All backend calls go through one typed client, `src/lib/api.ts`. Its commands, their types and the
  events' payloads come from `src/lib/bindings.ts`, generated from the Rust code by
  [tauri-specta](https://github.com/specta-rs/tauri-specta) (`src-tauri/src/bindings.rs`).

### Backend (Rust & Tauri v2)
* **Performance & safety:** memory management and intensive I/O without a garbage collector.
* **Audio facade:** the desktop engine (100 % Rust) is isolated behind the `AudioEngine` trait; mobile will provide
  native engines behind the same trait through conditional compilation (`#[cfg(...)]`). See `04-audio-engine.md`.
* **Async commands:** Tauri commands are `async` so that opening an audio device or a network call never freezes the
  window.

### Data & persistence
* **Local SQLite** through `rusqlite` (statically compiled with the `bundled` feature, so there is no system
  dependency), in **WAL** mode for fast, non-blocking reads and writes.
* The local database holds **the user's own data**: favorites, listening history, personal stations and settings.
  It does not hold the catalog, which is queried online (playing a radio requires a network anyway).

## 3. Catalog
* The rest of the app only talks to the `CatalogSource` trait (`search`, `count_click`). Today's implementation is the
  public Radio-Browser API: the mirrors are the addresses of `all.api.radio-browser.info` (a DNS lookup, as the API
  documentation asks), requests carry an explicit `User-Agent`, and the UI delays searches by 300 ms to spare the API.
  A click is sent to one mirror, never retried elsewhere: each mirror counts a listener once a day on its own.
* A station is never silently hidden: stations whose last check failed (which can be a false negative) are shown with
  a warning and ranked after healthy ones. HLS stations, which the engine cannot play yet, are left out of the results; since
  that shortens pages, a search fetches up to three catalog pages to fill one and returns the offset of the next page.
* **Click counting** goes **straight from the client** to Radio-Browser. Their API counts at most one click per IP,
  per station and per day, so an intermediary would make every user look like a single IP. It runs after playback has
  started, never on the critical path, and never for personal stations.

## 4. Privacy
* Click counting sends the station UUID and, as with any HTTP request, the IP address. It is on by default, with a
  notice at the first play and a setting to turn it off. Nothing is sent before the notice has been shown.
* Personal stations never leave the device.
* Catalog searches are sent to the Radio-Browser API.
* The update check (desktop, `updates.rs`) downloads `latest.json` from the latest GitHub Release once a day while
  its setting is on; nightlies and packaged copies never check (05 §4).

## 5. Files on disk
The app identifier is `io.github.getsoundfolk.soundfolk` (Flathub-style, after the `getsoundfolk` GitHub organisation;
never to be changed once released). Folders are named `soundfolk` rather than after it, through
`app.appDirectoriesOverride` in `tauri.conf.json`, so the `app_*_dir` path APIs, Tauri itself and plugins all use them:

| | Settings and recipes (`$CONFIG`) | Database and web view data (`$DATA`, `$LOCALDATA`) | Cache |
|---|---|---|---|
| Linux | `~/.config/soundfolk` | `~/.local/share/soundfolk` | `~/.cache/soundfolk` |
| macOS | `~/Library/Application Support/soundfolk` | same | `~/Library/Caches/soundfolk` |
| Windows | `%APPDATA%\soundfolk` | `%APPDATA%\soundfolk`, `%LOCALAPPDATA%\soundfolk` | `%LOCALAPPDATA%\soundfolk` |

* `soundfolk.db` (SQLite, WAL) is in the data folder; local recipes are in `<settings>/recipes`, next to
  `recipe.schema.json`.
* On macOS the web view keeps its data where WebKit decides (per bundle identifier), not in these folders.
* Under Flatpak these folders are inside the sandbox (`~/.var/app/io.github.getsoundfolk.soundfolk/…`).

## 6. Memory
The target is **under 50 MB while playing with the window closed**: an app left in the notification area should
cost less than a browser tab, and much less than an Electron player. With the window open, WebKit's processes come
on top; that cost is accepted while the user looks at the app.

`.github/scripts/measure-memory.sh [-w seconds] [pid]` adds up the app process and every process it started. The
figure kept is **PSS + Swap** of the whole tree: RSS counts each shared library again in every process, and swapped
pages are still the app's memory.

Measured on 2026-11-11 (release build of `1a34174`, Linux x86_64, X11, WebKitGTK 4.1), playing an AAC stream at
64 kbit/s, after one minute of playback, mean over two minutes (24 samples, peak within 0.2 MB):

| Situation | soundfolk | Web process | Network process | **Total (PSS)** | RSS total |
|---|---|---|---|---|---|
| Launched at login (`--hidden`), window never opened | 41.3 | — | — | **41.3 MiB** ✅ | 77 MiB |
| Window open | 65.6 | 77.6 | 12.0 | **155.1 MiB** | 429 MiB |
| Window closed after being open | 66.9 | — | — | **68.5 MiB** ❌ | 174 MiB |

* Closing the window frees the web process (`windows.rs`), and ten seconds after the last window is gone
  `memory.rs` stops WebKit's network process (12 MB; WebKit starts a new one with the next web view) and trims the
  heap. That took this line from 79.4 to 68.5 MiB; opening and closing the window three more times adds ~3 MiB.
* What the app process still keeps after a window, compared with a launch at login (+26 MiB): ~9 MiB of Mesa
  (`libLLVM`, `libgallium`) loaded for WebKit's GL rendering and never unloaded, ~6 MiB of WebKit and JavaScriptCore
  code pages, ~10 MiB of heap still in use (`malloc_trim` gives back less than 1 MiB). Code pages are clean: the
  kernel can drop them under pressure. Only a launch at login meets the target.
* **On Linux the web views are drawn without the GPU** (`windows::software_rendering`, WebKit's hardware
  acceleration policy set to *never*): ~22 MiB less while a window is open, nothing once it is closed (Mesa is
  loaded anyway). Measured back to back, in the same conditions: 156–157 MiB open instead of 179, 77 MiB closed
  either way. No cost measured at rest, and none seen by hand (scrolling a long search, station sheets, the mini
  player); the interface barely animates. `SOUNDFOLK_GPU_RENDERING=1` gives the GPU back, for a user whose display
  suffers from it. The policy is set on the web view rather than through `WEBKIT_DISABLE_COMPOSITING_MODE`, which
  every program the app starts would inherit.
* PSS splits a shared library between the processes that map it: the same run reads ~5 MiB more when no other
  WebKitGTK program is running (46 instead of 41 MiB launched at login, all of it `libwebkit2gtk` pages). Compare
  figures measured in the same conditions, back to back.
* How it was measured, without touching the instance in use: an isolated run under `dbus-run-session`, with
  `XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` pointing at a copy of the data, a minimal
  `org.kde.StatusNotifierWatcher` on that bus (without one the window opens even with `--hidden`), playback started
  through MPRIS (`org.mpris.MediaPlayer2.Player.Play` plays the last station) and the window closed with
  `wmctrl -c`.
