# Installing Soundfolk

Linux, Windows and macOS packages are built automatically (see
[`.github/workflows/release.yml`](../.github/workflows/release.yml)): each tagged version is attached to its
[release](https://github.com/getsoundfolk/soundfolk/releases), and a build of `main` is published every night
as the **nightly** pre-release. The Linux and Windows packages are 64-bit x86 only (amd64 / x64); the macOS
app is universal (Apple Silicon and Intel).

| System                 | File                              | Section                                   |
| ---------------------- | --------------------------------- | ----------------------------------------- |
| Debian, Ubuntu         | `Soundfolk_<version>_amd64.deb`   | [Debian/Ubuntu](#debianubuntu-deb)        |
| Other Linux            | `Soundfolk_<version>_amd64.AppImage` | [Other distributions](#other-distributions-appimage) |
| Windows 10 or later    | `Soundfolk_<version>_x64-setup.exe` | [Windows](#windows-installer)           |
| macOS 10.13 or later   | `Soundfolk_<version>_universal.dmg` | [macOS](#macos-disk-image)              |

## Debian/Ubuntu (`.deb`)

```bash
sudo apt install ./Soundfolk_*_amd64.deb
```

The package is built on Ubuntu 22.04, so it installs on **Ubuntu 22.04 or later** and **Debian 12 or later**.
Older releases lack the required glibc and WebKitGTK 4.1 versions.

From the release after 0.2.3, the package subscribes the system to Soundfolk's APT repository: new versions then
come with the system's other updates (Software Updater, `apt upgrade`). Delete
`/etc/apt/sources.list.d/soundfolk.sources` to stop that; to subscribe before installing, see
[17-apt-repository.md](17-apt-repository.md) §4.

## Other distributions (AppImage)

```bash
chmod +x Soundfolk_*_amd64.AppImage
./Soundfolk_*_amd64.AppImage
```

The AppImage carries its own libraries except glibc, so it runs on any distribution with **glibc 2.35 or
later**. It needs FUSE 2 (`libfuse2`, or `libfuse2t64` on Ubuntu 24.04); without it, run it with
`--appimage-extract-and-run`. `soundfolk://` links open it once it has been launched one time.

## Windows (installer)

Run `Soundfolk_*_x64-setup.exe`. It installs for the current user, without administrator rights, on
**Windows 10 or later**; it downloads Microsoft WebView2 if Windows lacks it. The installer is not
code-signed yet, so SmartScreen warns before running it: click **More info**, then **Run anyway**.

## macOS (disk image)

Open `Soundfolk_*_universal.dmg` and drag **Soundfolk** into **Applications**. It runs on **macOS 10.13 or
later**. The app is not notarized by Apple yet, so macOS refuses to open it the first time (the disk image's
window says so too):

1. Open **Soundfolk** from Applications, then close the message (**Done**, or **Cancel** before macOS 15;
   not **Move to Trash**).
2. Go to **System Settings › Privacy & Security** (before macOS 13: **System Preferences › Security &
   Privacy**), click **Open Anyway** under the message about Soundfolk, and confirm with your password.

Later launches and updates open it directly. From a terminal, `xattr -dr com.apple.quarantine
/Applications/Soundfolk.app` does the same in one step.

## Updates

The `.deb` is updated by the system (above). On Windows, macOS and from an AppImage, Soundfolk checks once a day
for a new version and offers **Install and restart** in ⚙; it can be turned off in ⚙ › Privacy
([19-in-app-updates.md](19-in-app-updates.md)).

## Checking a package

To check that a package was built by this repository's CI:

```bash
gh attestation verify Soundfolk_*_amd64.deb -R getsoundfolk/soundfolk
```

## Building from source

### Prerequisites (Debian/Ubuntu)

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libasound2-dev build-essential pkg-config
```

Plus Rust (stable ≥ 1.95) and Node 22.13 or later (22, 24 or 26+: what Vite, Vitest and ESLint
require; `engines` in `package.json`). On Windows and macOS, see Tauri's
[prerequisites](https://v2.tauri.app/start/prerequisites/). `libasound2-dev` is required by `cpal` (ALSA; under
PipeWire/PulseAudio, the `pipewire-alsa` / `pulseaudio-alsa` compatibility layer is enough).

### Run

```bash
npm install
npm run tauri dev
```

Search for a station (e.g. `rdwa`) then click ▶, or add a stream in the **My stations** tab (direct URL, `.m3u`
or `.pls`). Detailed engine logs: `RUST_LOG=soundfolk_lib=debug npm run tauri dev`.
