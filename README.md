# Soundfolk — Linux MVP

> Find and play any web radio. Know what’s playing, keep track of what you heard.

Tauri v2 + Svelte 5 + Rust audio engine (`stream-download` → `symphonia` → `cpal`) + SQLite (`rusqlite`).
Specs in [`docs/`](docs/).

## Install

Linux, Windows and macOS packages are built automatically (see
[`.github/workflows/release.yml`](.github/workflows/release.yml)): each tagged version is attached to its
[release](https://github.com/getsoundfolk/soundfolk/releases), and a build of `main` is published every night
as the **nightly** pre-release. The Linux and Windows packages are 64-bit x86 only (amd64 / x64); the macOS
app is universal (Apple Silicon and Intel).

### Debian/Ubuntu (`.deb`)

```bash
sudo apt install ./Soundfolk_*_amd64.deb
```

The package is built on Ubuntu 22.04, so it installs on **Ubuntu 22.04 or later** and **Debian 12 or later**.
Older releases lack the required glibc and WebKitGTK 4.1 versions.

### Other distributions (AppImage)

```bash
chmod +x Soundfolk_*_amd64.AppImage
./Soundfolk_*_amd64.AppImage
```

The AppImage carries its own libraries except glibc, so it runs on any distribution with **glibc 2.35 or
later**. It needs FUSE 2 (`libfuse2`, or `libfuse2t64` on Ubuntu 24.04); without it, run it with
`--appimage-extract-and-run`. `soundfolk://` links open it once it has been launched one time.

### Windows (installer)

Run `Soundfolk_*_x64-setup.exe`. It installs for the current user, without administrator rights, on
**Windows 10 or later**; it downloads Microsoft WebView2 if Windows lacks it. The installer is not
code-signed yet, so SmartScreen warns before running it: click **More info**, then **Run anyway**.

### macOS (disk image)

Open `Soundfolk_*_universal.dmg` and drag **Soundfolk** into **Applications**. It runs on **macOS 10.13 or
later**. The app is not notarized by Apple yet, so macOS refuses to open it the first time: go to
**System Settings › Privacy & Security** and click **Open Anyway** under the message about Soundfolk.

### Checking a package

To check that a package was built by this repository's CI:

```bash
gh attestation verify Soundfolk_*_amd64.deb -R getsoundfolk/soundfolk
```

## Build from source

### Prerequisites (Debian/Ubuntu)

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev build-essential pkg-config
```

Plus Rust (stable ≥ 1.95) and Node ≥ 20. `libasound2-dev` is required by `cpal` (ALSA; under
PipeWire/PulseAudio, the `pipewire-alsa` / `pulseaudio-alsa` compatibility layer is enough).

### Run

```bash
npm install
npm run tauri dev
```

Search for a station (e.g. `rdwa`) then click ▶, or add a stream in the **My stations** tab (direct URL, `.m3u` or `.pls`).
Detailed engine logs: `RUST_LOG=soundfolk_lib=debug npm run tauri dev`.

## Structure

| Path | Role |
|---|---|
| `src-tauri/src/audio/` | `mod.rs` facade + engine thread · `decoder/` network/decoding/stream test, retries, watchdog, conversion · `playlist.rs` m3u/pls · `icy.rs` metadata · `output.rs` cpal |
| `src-tauri/src/catalog/` | `CatalogSource` + Radio-Browser client (online search, click counting) |
| `src-tauri/src/db/mod.rs` | SQLite WAL: `favorites`, `listening_history`, `settings`, `personal_stations` |
| `src-tauri/src/commands.rs` | Tauri commands (`play_stream`, `stop_stream`, `pause_stream`, `resume_stream`, `get_player_state`, `search_stations`, favorites, personal stations, history, listen-counting setting): thin adapters |
| `src-tauri/src/player.rs` | Station on air (`AudioState`), `start_station`, click counting, resolving a station by its id |
| `src-tauri/src/state.rs` | Catalog, web titles and covers in Tauri's state; `blocking` / `with_db` for blocking work |
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
