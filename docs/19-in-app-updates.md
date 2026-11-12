# 🔄 In-app updates

How a copy of Soundfolk installed from the macOS disk image, the Windows installer or an AppImage installs the next
version itself, in one click: Tauri's updater (`tauri-plugin-updater`), fed by the `latest.json` the update check
already reads (05 §4). The `.deb` keeps its APT repository (17); Flatpak and Snap keep their stores.

**Status: built, waiting for the updater key (§4).** Until the key is set up, releases list no package and the
notice links to the release page, as before.

## 1. What the listener sees
* **The same notice**, in Settings › About: the new version, **What's new** and **Skip**, plus **Install and
  restart** when this copy can install it (§2).
* **One click.** The app downloads the package (about 12 MB), checks its signature, replaces itself and starts
  again. "Downloading the new version…" shows meanwhile; playback goes on until the restart.
* **macOS:** the app downloaded by the app itself carries no quarantine mark, so Gatekeeper does not ask again:
  only the very first launch, from the disk image, needs Open Anyway ([install.md](install.md)).
* **Windows:** the installer runs with a progress bar only (`installMode: passive`), for the current user like the
  first install, and starts the app again. SmartScreen does not step in: the package was not downloaded by a
  browser.
* **If it fails** (no network, a full disk, an app that cannot replace itself), the error shows like any other and
  the app goes on as it was. The release page stays one click away.

## 2. Which copies
`Release::installable` (`src-tauri/src/updates.rs`) is true only when all of these hold:
* The copy checks for updates at all (not a nightly, a Flatpak, a Snap or a packager's build: 05 §4).
* It is a kind the updater replaces: the bundler marks each package's binary with its kind, and the app reads it
  (`tauri::utils::platform::bundle_type`). An app bundle (`app`, macOS), the NSIS installer (`nsis`), an AppImage
  (`appimage`). Never a `.deb` or an `.rpm`, even though the updater could: the system's package manager owns
  those files. A development build has no mark.
* `tauri.conf.json` holds the public key (`plugins.updater.pubkey`).
* `latest.json` lists a package for it, under the name the updater looks for: `{os}-{arch}-{kind}`, so
  `darwin-aarch64-app`, `darwin-x86_64-app`, `windows-x86_64-nsis`, `linux-x86_64-appimage`.

A value of `installable` in `latest.json` is ignored, as `through_apt` is: the app decides.

## 3. How a release carries them
* **The packages are the ones people download.** The AppImage and the Windows installer as they are; for macOS,
  `Soundfolk_<version>_universal.app.tar.gz`, the app taken out of the disk image (job **Build the macOS disk
  image**), which the updater unpacks in place of the installed one. One universal app serves both entries.
* **Signed apart from the builds.** Job **Sign the in-app updates** (`release.yml`), on tags only, after the three
  builds and before publishing: it signs each package with the Tauri CLI, bound to the version
  (`--app-version`), then checks each signature with `minisign` against the public key in `tauri.conf.json`, and
  that it names this version. It runs in the protected `updater` environment, the only place the private key
  lives; the builds and the nightlies never see it.
* **`latest.json`** (job **Publish**) keeps `version` and `url`, and gains `platforms`: for each signed package,
  its download address in the release and its signature. The `.sig` files are uploaded with the packages.
* **The app checks** (`requireSignedVersion`): the signature must verify with the key it was built with, and name
  the version `latest.json` announces. An older package, or a `latest.json` changed on the way, is refused.
* The updater reads `latest.json` again itself when the listener clicks, from the address in `tauri.conf.json`
  (`SOUNDFOLK_UPDATE_URL` changes only the notice), with its own HTTP client: our own service, over HTTPS, and
  what it downloads is verified before anything is installed.

## 4. The key
Done by hand by a maintainer: the private key never passes through anyone else.

**Making it**, in a terminal, from the repository's root (the CLI asks for a password; keep it):
```bash
npx tauri signer generate -w ~/soundfolk-updater.key
```
It writes the private key, encrypted with that password, to `~/soundfolk-updater.key`, and the public key to
`~/soundfolk-updater.key.pub`: that one goes into `tauri.conf.json`, `plugins.updater.pubkey`.

**Giving it to the release workflow**, in the protected environment `updater`, deployments limited to `v*` tags:
```bash
gh api -X PUT repos/getsoundfolk/soundfolk/environments/updater \
  -F 'deployment_branch_policy[protected_branches]=false' \
  -F 'deployment_branch_policy[custom_branch_policies]=true'
gh api -X POST repos/getsoundfolk/soundfolk/environments/updater/deployment-branch-policies \
  -f name='v*' -f type=tag
gh secret set TAURI_SIGNING_PRIVATE_KEY --env updater --repo getsoundfolk/soundfolk < ~/soundfolk-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env updater --repo getsoundfolk/soundfolk
```
Then copy `~/soundfolk-updater.key` and its password off the computer, in two places, and delete the key from the
home folder.

**If it is lost.** The installed copies trust only this key: without it, no release can reach them through the
updater any more. Make a new key, ship its public key in a release, and the copies that install that release
by hand (from the release page, as before the updater) trust the new one. The `.deb`, Flatpak and Snap are not
affected.

**If it leaks.** Someone holding it and the password could have an installed copy accept a package of theirs, if
they can also serve it a `latest.json`, which they cannot through GitHub without the project's account. Replace
the key at once: a release signed with the old key, carrying the new public key, moves every copy that updates to
the new one; then change the secrets.

**Changing it on purpose** works the same way: one release still signed with the old key carries the new public
key; the next ones are signed with the new key.
