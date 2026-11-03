# Soundfolk — Linux MVP

> Find and play any web radio. Know what’s playing, keep track of what you heard.

Tauri v2 + Svelte 5 + Rust audio engine (`stream-download` → `symphonia` → `cpal`) + SQLite (`rusqlite`).
Specs in [`docs/`](docs/).

## Prerequisites (Debian/Ubuntu)

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev build-essential pkg-config
```

Plus Rust (stable ≥ 1.85) and Node ≥ 20. `libasound2-dev` is required by `cpal` (ALSA; under
PipeWire/PulseAudio, the `pipewire-alsa` / `pulseaudio-alsa` compatibility layer is enough).

## Run

```bash
npm install
npm run tauri dev
```

Search for a station (e.g. `rdwa`) then click ▶, or add a stream in the **My stations** tab (direct URL, `.m3u` or `.pls`).
Detailed engine logs: `RUST_LOG=soundfolk_lib=debug npm run tauri dev`.

## Structure

| Path | Role |
|---|---|
| `src-tauri/src/audio/` | `mod.rs` facade + engine thread · `decoder.rs` network/decoding/stream test · `playlist.rs` m3u/pls · `icy.rs` metadata · `output.rs` cpal |
| `src-tauri/src/catalog/` | `CatalogSource` + Radio-Browser client (online search, click counting) |
| `src-tauri/src/db/mod.rs` | SQLite WAL: `favorites`, `listening_history`, `station_reports`, `settings`, `personal_stations` |
| `src-tauri/src/commands.rs` | Tauri commands (`play_stream`, `stop_stream`, `pause_stream`, `resume_stream`, `set_volume`, `get_player_state`, `search_stations`, favorites, personal stations, history, reports, listen-counting setting) |
| `src/lib/` | Svelte 5 UI (CSS Modules): player, search, favorites, personal stations, listen-counting notice; shared state in `player.svelte.ts` |

Events emitted to the frontend:
- `player:state` → `{ status: "idle" | "connecting" | "playing" | "paused" }` or `{ status: "error", message }`.
- `player:now-playing` → `{ station, title }` (ICY metadata; each title is also logged in `listening_history`).

## Known MVP limitations

- **HLS (`.m3u8`) is not supported**: HLS stations (roughly 14 % of the most popular ones) are left out of the search results.
- No current title for streams without `icy-metaint` (e.g. Ogg, whose comments are inside the stream).
- An Ogg logical stream change mid-stream causes a reconnection (short silence).
- The catalog is queried online (Radio-Browser API, a single active mirror for now).
- Desktop only: the mobile engine (`AVPlayer` / `ExoPlayer`) will come behind the same `AudioEngine` trait.

## Privacy

- **Listen counting** (on by default, can be turned off in the settings): each time a catalog station is played,
  Soundfolk calls Radio-Browser's `/json/url/{uuid}`. What is sent: the station UUID and, as with any internet
  request, the IP address. Nothing else (no favorites, no history). A notice is shown on the first Play; that
  first click is not counted. Personal stations are never reported.
- **Personal stations**: stored on the device only.
- **Catalog**: searches are sent to the Radio-Browser API (a community service, with no availability guarantee).

## License

The client is released under [MPL-2.0](LICENSE). Contributions follow [CONTRIBUTING.md](CONTRIBUTING.md) (DCO).
The **Soundfolk** name and logo are not covered by this license.
