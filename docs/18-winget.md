# 📦 winget

How Soundfolk installs and updates in one command on Windows: a package in winget's catalogue
(`microsoft/winget-pkgs`), which `winget` (App Installer, built into Windows 10 and 11) reads. Windows part of
16 §7.3, as the APT repository (17) is for Debian and Ubuntu.

**Status: the release job is ready; the package is not in the catalogue yet.** The first version is submitted by
hand (§3); until then, and without the token, the job says so and stops.

## 1. What the listener sees
* **Install:** `winget install Soundfolk.Soundfolk`. winget downloads the installer of the GitHub Release, checks
  its SHA-256 against the manifest, and runs it silently: the same per-user install as the README's, no
  administrator rights.
* **Update:** `winget upgrade Soundfolk.Soundfolk`, or `winget upgrade --all`. winget also sees a copy installed
  from the `.exe` by hand: it reads the same Apps & Features entry.
* **SmartScreen does not ask.** winget runs the installer itself, so "Run anyway" goes away, though the installer
  is still not code-signed (TODO). Microsoft's checks scan each submitted installer before merging it.
* **A delay.** Microsoft's bots and moderators review every pull request: a version reaches winget hours to a
  few days after the release.

## 2. How a version gets there
* `release.yml`, job **Submit to winget**, after the release is published, tags only, never a nightly.
* It runs Komac (pinned by version and checksum), `komac update Soundfolk.Soundfolk --version … --urls … --submit`:
  Komac downloads the installer, writes the three manifests (version, default locale, installer) from it and from
  the GitHub Release, pushes them to the token owner's fork of `winget-pkgs` and opens the pull request.
* What does not change between versions (description, licence, tags, scope `user`) comes from the previous
  manifest in the catalogue: change it there, in the next pull request or by hand.
* Nothing else to keep in step: the installer's file name (`Soundfolk_<version>_x64-setup.exe`) is the only link
  between the job and the bundler.

## 3. Setup
Done by hand by a maintainer.

**The token.** Komac needs a classic token with the `public_repo` scope: a fine-grained token cannot open a pull
request on a repository of another organisation. Make it on a GitHub account that will own the fork of
`winget-pkgs` (a maintainer's, or a bot account kept for this), with an expiry date, then:
```bash
gh secret set WINGET_TOKEN --env winget --repo getsoundfolk/soundfolk
```
The environment `winget` of `getsoundfolk/soundfolk` allows deployments from tags `v*` only, as `apt` does.
When the token expires, the job fails at the pull request: make a new one and set it again.

**The first version.** `komac update` only adds a version to a package that exists. The first one is made with
`komac new` on the latest release, with the token in `GITHUB_TOKEN`:
```bash
komac new Soundfolk.Soundfolk --version 0.2.3 \
  --urls https://github.com/getsoundfolk/soundfolk/releases/download/v0.2.3/Soundfolk_0.2.3_x64-setup.exe \
  --publisher Soundfolk --package-name Soundfolk --moniker soundfolk --license MPL-2.0 \
  --publisher-url https://github.com/getsoundfolk --package-url https://github.com/getsoundfolk/soundfolk \
  --license-url https://github.com/getsoundfolk/soundfolk/blob/main/LICENSE \
  --short-description "Discover and listen to web radios from around the world"
```
It asks the rest in the terminal: the long description (`longDescription` of `tauri.conf.json`), tags (`radio`,
`internet-radio`, `music`), scope `user`. It fills in the installer's details itself. Then follow the pull request
until it is merged: the bots comment when a check fails. From the next tag on, the job takes over.

**After the first merge:** add `winget install Soundfolk.Soundfolk` to the README's Windows section, and the
line to the share page's Get Soundfolk (16 §5.3).

## 4. Still open
* **Publisher "github" in Apps & Features.** `tauri.conf.json` sets no `bundle.publisher`, so the NSIS installer
  takes the second part of the identifier, `github`. winget matches the installed app by its Apps & Features key
  (`Soundfolk`), not by publisher, so it works. Setting `publisher` would fix the label, but the installer also
  keeps the install folder under `HKCU\Software\<publisher>\Soundfolk`: test an upgrade from 0.2.3 on Windows
  before changing it.
* **The app's own update notice** (Settings › About) still sends Windows users to the release page. It could say
  `winget upgrade Soundfolk.Soundfolk` instead, but the app cannot tell how it was installed.
