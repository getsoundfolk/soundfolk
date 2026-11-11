#!/usr/bin/env bash
# SPDX-License-Identifier: MPL-2.0
#
# Adds a .deb to Soundfolk's APT repository and signs the repository again
# (docs/17-apt-repository.md). The repository is a folder served as is (GitHub Pages):
#
#   pool/main/s/soundfolk/Soundfolk_<version>_amd64.deb   the last KEEP versions
#   dists/stable/main/binary-amd64/Packages[.gz]
#   dists/stable/{Release,InRelease,Release.gpg}          signed with SIGNING_KEY
#   soundfolk-archive-keyring.gpg                         the public key, for a manual setup
#
# Everything under dists/ is rebuilt from the packages in pool/: nothing is kept between runs.
#
# Usage: update-apt-repo.sh <repository folder> <package.deb> <public keyring>
# Environment: SIGNING_KEY, the fingerprint of a secret key in GnuPG's keyring;
#              KEEP, how many versions stay in the pool (default 3).
set -euo pipefail

repo=$1
deb=$2
keyring=$3
: "${SIGNING_KEY:?the fingerprint of the signing key}"
keep=${KEEP:-3}

package=$(dpkg-deb --field "$deb" Package)
if [ "$package" != soundfolk ]; then
  echo "$deb: package $package, not soundfolk" >&2
  exit 1
fi

pool=pool/main/s/soundfolk
mkdir -p "$repo/$pool"
cp "$deb" "$repo/$pool/"

# The oldest versions leave the pool, as dpkg orders them.
mapfile -t packages < <(
  for file in "$repo/$pool"/*.deb; do
    printf '%s\t%s\n' "$(dpkg-deb --field "$file" Version)" "$file"
  done | sort -t $'\t' -k1,1 -V | cut -f2
)
for ((i = 0; i < ${#packages[@]} - keep; i++)); do
  echo "dropping ${packages[i]##*/}"
  rm -f "${packages[i]}"
done

cd "$repo"
rm -rf dists
binary=dists/stable/main/binary-amd64
mkdir -p "$binary"
apt-ftparchive packages "$pool" > "$binary/Packages"
gzip -9 --keep --no-name "$binary/Packages"
apt-ftparchive \
  -o APT::FTPArchive::Release::Origin=Soundfolk \
  -o APT::FTPArchive::Release::Label=Soundfolk \
  -o APT::FTPArchive::Release::Suite=stable \
  -o APT::FTPArchive::Release::Codename=stable \
  -o APT::FTPArchive::Release::Architectures=amd64 \
  -o APT::FTPArchive::Release::Components=main \
  -o "APT::FTPArchive::Release::Description=Soundfolk, a desktop player for web radios" \
  release dists/stable > dists/stable/Release.tmp
mv dists/stable/Release.tmp dists/stable/Release

gpg --batch --yes --local-user "$SIGNING_KEY" --clearsign \
  --output dists/stable/InRelease dists/stable/Release
gpg --batch --yes --local-user "$SIGNING_KEY" --armor --detach-sign \
  --output dists/stable/Release.gpg dists/stable/Release

cd - > /dev/null
cp "$keyring" "$repo/soundfolk-archive-keyring.gpg"
# Served as is: no Jekyll build on GitHub Pages.
touch "$repo/.nojekyll"

echo "repository: $(cd "$repo" && ls $pool)"
