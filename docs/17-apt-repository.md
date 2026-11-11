# 📦 APT repository

How a `.deb` installed once keeps itself up to date on Debian and Ubuntu: Soundfolk's own APT repository, which
Software Updater (or `apt upgrade`) reads like any other. First step of 16 §7.3 that needs no third party.

**Status: set up on 2026-11-11, filled from the first release after 0.2.3.** Packages up to 0.2.3 do not
subscribe: their listeners install the next `.deb` once by hand, or subscribe with §4.

## 1. What the listener sees
* **They install the `.deb` once**, as today. The package subscribes the system to the repository: from then on,
  new versions come with the system's other updates. Nothing to type.
* **The update notice says so.** When the system is subscribed, the line in Settings › About (05 §4) adds
  "Software Updater will install it" under the new version, instead of sending them to download a package.
* **They can leave.** Deleting `/etc/apt/sources.list.d/soundfolk.sources` stops the updates; later upgrades of
  the package do not put it back. Removing the package removes the source with it (its key goes too, so APT would
  fail on every update otherwise). Installing it again subscribes again.
* **Or subscribe first**, before installing (§4).

## 2. The repository
* Served by GitHub Pages from the `main` branch of `getsoundfolk/apt`, at `https://getsoundfolk.github.io/apt`.
* One suite, `stable`, one component, `main`, amd64 only: the `.deb` is built on Ubuntu 22.04, so one package
  serves every supported release (README).
* Tagged releases only, never a nightly. The last three versions stay in the pool, so that a listener can go back
  one or two with `apt install soundfolk=<version>`.
* Signed with Soundfolk's archive key. The `.deb` carries its public part in
  `/usr/share/keyrings/soundfolk-archive-keyring.gpg`, and the source names it with `Signed-By`: the key vouches for
  this repository only, not for the system's other sources.

## 3. How it is built
* `release.yml`, job **Update the APT repository**, after the release is published: it takes the tagged `.deb`,
  runs `.github/scripts/update-apt-repo.sh`, and pushes the result to `getsoundfolk/apt`.
* The script rebuilds `dists/` from the packages in `pool/` with `apt-ftparchive` (no state kept between runs),
  drops the oldest versions as dpkg orders them, and signs `InRelease` and `Release.gpg`.
* The job pushes one commit without history over the last: 12 MB per version would otherwise pile up in the
  repository's history.
* It runs in the protected `apt` environment, the only place the signing key and the push key live. It refuses to
  sign with a key that is not the one in `src-tauri/packaging/apt/soundfolk-archive-keyring.gpg`.
* The package side is in `src-tauri/packaging/apt/`: the source file (installed as a template under
  `/usr/share/soundfolk/apt/`), `postinst.sh` that copies it to `/etc/apt/sources.list.d/` once, and `postrm.sh`
  that removes it with the package.

## 4. Subscribing without the `.deb`
```bash
sudo curl -fsSLo /usr/share/keyrings/soundfolk-archive-keyring.gpg \
  https://getsoundfolk.github.io/apt/soundfolk-archive-keyring.gpg
sudo curl -fsSLo /etc/apt/sources.list.d/soundfolk.sources \
  https://raw.githubusercontent.com/getsoundfolk/soundfolk/main/src-tauri/packaging/apt/soundfolk.sources
sudo apt update && sudo apt install soundfolk
```
The same two files as the package's, at the same paths: installing the package afterwards changes nothing.

## 5. Keys and setup
Done by hand by a maintainer: the private keys never pass through anyone else.

**What exists** (set up on 2026-11-11):
* The archive key `2EC2FA553148DB13358A3CE2C51311C5C23F5326`, "Soundfolk archive signing key" (no address: the
  project has none yet). Its primary key only certifies, never expires, and stays offline with the maintainer,
  protected by a passphrase, with its revocation certificate. Its signing subkey
  `A38D76ED050D3FF917001913867B03FF64B88537` expires on **2029-11-10**: the CI warns six months ahead and fails
  one month ahead (`.github/scripts/check-apt-key.sh`, job Repository of `ci.yml`).
* The public keyring in `src-tauri/packaging/apt/soundfolk-archive-keyring.gpg`, shipped by the `.deb`.
* The environment `apt` of `getsoundfolk/soundfolk`, deployment limited to tags `v*`, with `APT_SIGNING_KEY` (the
  subkey alone, without passphrase: the job signs unattended) and `APT_DEPLOY_KEY`.
* The repository `getsoundfolk/apt`, Pages from `main`, and its deploy key "soundfolk release" with write access
  (the organisation allows deploy keys for this).

**Making them again**, in a terminal (GnuPG asks for passphrases), from the repository's root:
```bash
export GPG_TTY=$(tty) GNUPGHOME="$(mktemp -d)"
gpg --quick-gen-key "Soundfolk archive signing key" ed25519 cert never
FINGERPRINT=$(gpg --list-keys --with-colons | awk -F: '/^fpr/{print $10; exit}')
gpg --quick-add-key "$FINGERPRINT" ed25519 sign 3y
gpg --export "$FINGERPRINT" > src-tauri/packaging/apt/soundfolk-archive-keyring.gpg
gpg --armor --export-secret-keys "$FINGERPRINT" > ~/soundfolk-archive-primary.asc          # offline backup
cp "$GNUPGHOME/openpgp-revocs.d/$FINGERPRINT.rev" ~/soundfolk-archive-revocation.rev       # offline too
# The subkey without passphrase, in a second keyring (the primary keeps its own):
export CI_HOME="$(mktemp -d)"
gpg --armor --export-secret-subkeys "$FINGERPRINT" > "$CI_HOME/sub.asc"
GNUPGHOME="$CI_HOME" gpg --pinentry-mode loopback --import "$CI_HOME/sub.asc"
# `gpg --passwd` stops at the absent primary: ask the agent for the subkey's keygrip instead.
GRIP=$(GNUPGHOME="$CI_HOME" gpg --with-keygrip -K "$FINGERPRINT" | awk '/^ssb/ { s = 1 } s && /Keygrip/ { print $3; exit }')
GNUPGHOME="$CI_HOME" gpg-connect-agent "PASSWD $GRIP" /bye                         # new passphrase: empty
GNUPGHOME="$CI_HOME" gpg --armor --export-secret-subkeys "$FINGERPRINT" \
  | gh secret set APT_SIGNING_KEY --env apt --repo getsoundfolk/soundfolk
rm -rf "$CI_HOME" "$GNUPGHOME"
```
The deploy key: `ssh-keygen -t ed25519 -N '' -f apt-deploy`, then `gh repo deploy-key add apt-deploy.pub --repo
getsoundfolk/apt --allow-write` and `gh secret set APT_DEPLOY_KEY --env apt < apt-deploy`.

**Renewing the subkey**, months before it expires: import the backup, add a new signing subkey, commit the
exported public keyring, and publish a release: still signed with the old subkey, it brings the new one to every
subscribed system. Only then replace `APT_SIGNING_KEY` with the new subkey. Once the old one has expired, no
release can reach those systems any more: their listeners have to reinstall the `.deb` by hand.

**If a key is lost or leaks.** The backup holds the subkey too: a lost secret is made again from it. A lost backup
breaks nothing until the subkey expires: make a new key, ship both public keys in one release, then sign with
the new one. A leaked subkey: publish at once a release whose keyring revokes it and carries a new one. Without
any key the systems trust, the repository can be rebuilt, but every subscribed listener reinstalls the `.deb`.
