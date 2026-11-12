<div align="center">

<img src=".github/logo.svg" alt="" width="96" height="96">

# Soundfolk

**Find and play any web radio. Know what’s playing, keep track of what you heard.**

A free desktop app for Linux, Windows and macOS, with the tens of thousands of stations of the
[Radio-Browser](https://www.radio-browser.info) catalog. No account, no ads, no tracking.

[![Latest release](https://img.shields.io/github/v/release/getsoundfolk/soundfolk?label=release)](https://github.com/getsoundfolk/soundfolk/releases/latest)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-informational)](docs/install.md)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue)](LICENSE)
[![CI](https://github.com/getsoundfolk/soundfolk/actions/workflows/ci.yml/badge.svg)](https://github.com/getsoundfolk/soundfolk/actions/workflows/ci.yml)

**[Download](https://getsoundfolk.github.io/download/)** ·
[Website](https://getsoundfolk.github.io) ·
[Privacy](#privacy) ·
[Contribute](#contributing)

</div>

<!--
  Screenshot to add: docs/screenshots/main.png (the player with a filled history, about 1600 px wide).
  <p align="center"><img src="docs/screenshots/main.png" alt="Soundfolk playing a station, with the tracks heard today" width="800"></p>
-->

## Download

| System                 | Package                                                                    |
| ---------------------- | -------------------------------------------------------------------------- |
| Debian 12+, Ubuntu 22.04+ | `.deb`, updated with the system afterwards                             |
| Other Linux (glibc 2.35+) | `.AppImage`                                                            |
| Windows 10+            | `-setup.exe`, per-user, no administrator rights                            |
| macOS 10.13+           | `.dmg`, universal (Apple Silicon and Intel)                                |

Get them from the [latest release](https://github.com/getsoundfolk/soundfolk/releases/latest). The Windows
installer is not code-signed and the macOS app not notarized yet, so the first launch needs one extra click:
[docs/install.md](docs/install.md) shows which, and how to check that a package was built by this repository.

## Why Soundfolk

- **Every station, one search away.** Type a name, a city or a genre; stations that work come first. Not sure
  what to play? **Surprise me** picks a music station from anywhere in the world. Your own streams (direct URL,
  `.m3u`, `.pls`) sit next to the catalog's.
- **Know what’s on air**, even when the stream does not say: Soundfolk then reads the title from the station’s
  website, with recipes written by listeners, or from its streaming server’s status page.
- **Never lose a discovery.** Every track you hear is written down, by day, with its station. Like it with ♡,
  find it later, open it on YouTube, Spotify, Deezer, Apple Music, Bandcamp or Discogs in one click.
- **Stats that mean something:** stations ranked by likes per hour of listening, listening time by hour and day,
  and, if you allow it, the artists, genres and decades of what you liked.
- **Constant volume** from one station to the next, and sound in about half a second.
- **At home on the desktop:** mini player, media keys, tray icon, track notifications, launch at login, keyboard
  shortcuts, light and dark themes.
- **Share a station** with the track on air, and browse **lists of stations** put together by listeners.
- **Light:** a Rust audio engine, not a browser tab.

<!--
  Screenshots to add: docs/screenshots/stats.png, docs/screenshots/mini-player.png, docs/screenshots/search.png.
-->

## Privacy

Your favorites, history, likes and stats live in a database on your computer. Soundfolk has no account and no
server of its own, and sends nothing about you without a switch in its settings:

- **Listen counting** (on by default): playing a catalog station tells Radio-Browser its UUID, for popularity
  rankings. Nothing else; never for your own stations. A notice is shown on the first Play, which is not counted.
- **Track identification** (off by default): only liked tracks are looked up on MusicBrainz, for the stats.
- **New versions** (on by default): once a day, the version number is read from the latest GitHub release.
- **Search** goes to the Radio-Browser API, a community service.

No telemetry beyond that. Details: [docs/05-features.md](docs/05-features.md) §4.

## Known limitations

- **HLS (`.m3u8`) streams are not supported yet**, so those stations (about 14 % of the most popular ones) are
  left out of the results.
- Search needs a network: the catalog is queried online.
- Desktop only for now; mobile apps will come behind the same audio engine interface.

## Contributing

You can help without writing code: write a **recipe** that reads the titles of a station’s website, add a
station’s **donation link**, or put together a **list** of stations. See
[the website](https://getsoundfolk.github.io/contribute/).

For code, read [CONTRIBUTING.md](CONTRIBUTING.md) (commits are signed off, DCO). Questions and ideas go to
[Discussions](https://github.com/getsoundfolk/soundfolk/discussions), bugs to
[Issues](https://github.com/getsoundfolk/soundfolk/issues).

```bash
npm install
npm run tauri dev
```

Prerequisites and logs: [docs/install.md § Building from source](docs/install.md#building-from-source).

Tauri v2 + Svelte 5, a Rust audio engine (`stream-download` → `symphonia` → `cpal`) and SQLite (`rusqlite`).

| Path                           | Role                                                                                                    |
| ------------------------------ | ------------------------------------------------------------------------------------------------------- |
| `src-tauri/src/`               | Rust back end: audio engine (`audio/`), catalog (`catalog/`), SQLite (`db/`), Tauri commands (`commands.rs`) |
| `src-tauri/crates/nowplaying/` | Network-free engine reading the title on air from a station's web page, for streams without metadata    |
| `src-tauri/recipes/`           | The stations' recipes for that engine, their test pages and `recipe.schema.json`                        |
| `src/`                         | Svelte 5 front end (CSS Modules); `src/lib/bindings.ts` is generated from the Rust code                 |
| `docs/`                        | Specifications, one file per topic                                                                      |

The layers and how they talk are described in [docs/02-architecture.md](docs/02-architecture.md), the
tables in [docs/03-database.md](docs/03-database.md), and the events sent to the front end, with their
payloads, in [`src-tauri/src/events.rs`](src-tauri/src/events.rs).

## Thanks

The station catalog comes from [Radio-Browser](https://www.radio-browser.info), run by volunteers, and track
identification from [MusicBrainz](https://musicbrainz.org).

## License

The client is released under [MPL-2.0](LICENSE). The **Soundfolk** name and logo are not covered by this license.
