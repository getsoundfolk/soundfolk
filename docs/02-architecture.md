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
* All backend calls go through one typed client, `src/lib/api.ts`.

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
  public Radio-Browser API: the mirror is discovered through `/json/servers`, requests carry an explicit
  `User-Agent`, and the UI delays searches by 300 ms to spare the API.
* A station is never silently hidden: stations whose last check failed (which can be a false negative) are shown with
  a warning and ranked after healthy ones; HLS stations are shown but disabled until the engine can play them.
* **Click counting** goes **straight from the client** to Radio-Browser. Their API counts at most one click per IP,
  per station and per day, so an intermediary would make every user look like a single IP. It runs after playback has
  started, never on the critical path, and never for personal stations.

## 4. Privacy
* Click counting sends the station UUID and, as with any HTTP request, the IP address. It is on by default, with a
  notice at the first play and a setting to turn it off. Nothing is sent before the notice has been shown.
* Personal stations never leave the device.
* Catalog searches are sent to the Radio-Browser API.
