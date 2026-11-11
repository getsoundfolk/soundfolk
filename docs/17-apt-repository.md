# 📦 APT repository

How a `.deb` installed once keeps itself up to date on Debian and Ubuntu: Soundfolk's own APT repository, which
Software Updater (or `apt upgrade`) reads like any other. First step of 16 §7.3 that needs no third party.

**Status: ready, not live.** The workflow job, the script, the package scripts and the app's notice are in place;
the repository waits for the setup of §5. Until then the release job skips it, and the `.deb` does not carry the
key and the source yet: once the public key is committed, `bundle.linux.deb` in `tauri.conf.json` gains
`files` (the keyring and the source template) and `postInstallScript` / `postRemoveScript`
(`packaging/apt/postinst.sh`, `postrm.sh`).

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

## 5. Setting it up (once, by a maintainer)
Done by hand: the keys must never pass through anyone else.

1. **The archive key.** On a trusted machine, in a fresh `GNUPGHOME`: a primary key that only certifies, kept
   offline, and a signing subkey for the CI.
   ```bash
   export GNUPGHOME="$(mktemp -d)"
   gpg --quick-gen-key "Soundfolk archive <maintainer address>" ed25519 cert never
   gpg --quick-add-key <primary fingerprint> ed25519 sign 3y
   gpg --export <primary fingerprint> > src-tauri/packaging/apt/soundfolk-archive-keyring.gpg
   gpg --armor --export-secret-subkeys <primary fingerprint> > signing-subkey.asc   # → APT_SIGNING_KEY
   gpg --armor --export-secret-keys <primary fingerprint> > primary.asc             # → offline backup
   ```
   Commit the public keyring. Keep `primary.asc` offline; delete `signing-subkey.asc` once it is a secret.
2. **The repository** `getsoundfolk/apt`: public, created with a README so that `main` exists. Settings › Pages:
   deploy from the branch `main`, folder `/`.
3. **The push key.** `ssh-keygen -t ed25519 -N '' -f apt-deploy`; `apt-deploy.pub` goes to `getsoundfolk/apt` ›
   Settings › Deploy keys, with write access; `apt-deploy` becomes `APT_DEPLOY_KEY`.
4. **The environment** `apt` in `getsoundfolk/soundfolk` › Settings › Environments: deployment limited to tags
   `v*`, with the secrets `APT_SIGNING_KEY` and `APT_DEPLOY_KEY`.
5. **The first run**: the next tagged release fills the repository. A release already published can be added by
   running the job again from its workflow run.

**Renewing the subkey** before it expires (3 years): add a new signing subkey with the offline primary, commit the
exported public keyring (packages update `/usr/share/keyrings` with it), wait for one release to spread it, then
replace `APT_SIGNING_KEY`. APT refuses a repository signed by an expired key.
