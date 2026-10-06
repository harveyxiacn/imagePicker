#!/usr/bin/env bash
# Refresh the AUR packages for a published release: pkgver, pkgrel, sha256sums, .SRCINFO.
#
#   packaging/aur/update.sh 0.2.0
#
# Run it AFTER the GitHub Release for v<version> is published (it downloads release assets to hash them).
# Then review `git diff packaging/aur`, copy each package directory into its AUR git repo, and push there.
#
# Env:
#   UPDATE_SKIP_DOWNLOAD=1   do not download anything; write dummy zero hashes (used to test this script)
#   REPO=owner/name          override the GitHub repository (default harveyxiacn/imagePicker)
set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ] || [ "$VERSION" = "-h" ] || [ "$VERSION" = "--help" ]; then
  sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 1
fi
VERSION="${VERSION#v}"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
  echo "error: '$VERSION' is not a SemVer version" >&2; exit 2
fi

REPO="${REPO:-harveyxiacn/imagePicker}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TAG="v$VERSION"
PKGVER="${VERSION//-/_}" # pkgver may not contain '-'
DEB="imagePicker_${VERSION}_amd64.deb"
REL="https://github.com/$REPO/releases/download/$TAG"
ZERO="$(printf '0%.0s' $(seq 1 64))"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

fetch_sha() { # fetch_sha <url> -> sha256 of the downloaded file
  if [ "${UPDATE_SKIP_DOWNLOAD:-0}" = 1 ]; then echo "$ZERO"; return; fi
  curl -fsSL --retry 3 -o "$TMP/dl" "$1"
  sha256sum "$TMP/dl" | cut -d' ' -f1
}

# deb hash: prefer the published SHA256SUMS.txt (also proves the file exists), else hash the download.
deb_sha=""
if [ "${UPDATE_SKIP_DOWNLOAD:-0}" = 1 ]; then
  deb_sha="$ZERO"
elif curl -fsSL --retry 3 -o "$TMP/SHA256SUMS.txt" "$REL/SHA256SUMS.txt"; then
  deb_sha="$(awk -v f="$DEB" '$2 == f || $2 == "*" f { print $1 }' "$TMP/SHA256SUMS.txt")"
fi
[ -n "$deb_sha" ] || deb_sha="$(fetch_sha "$REL/$DEB")"
lic_sha="$(fetch_sha "https://raw.githubusercontent.com/$REPO/$TAG/LICENSE")"
src_sha="$(fetch_sha "https://github.com/$REPO/archive/refs/tags/$TAG.tar.gz")"

echo "imagepicker-bin: deb=$deb_sha license=$lic_sha"
echo "imagepicker:     source=$src_sha"

update_pkgbuild() { # update_pkgbuild <dir> <sha>...
  local dir="$1"; shift
  local pb="$dir/PKGBUILD" sums="" first=1 s
  for s in "$@"; do
    if [ $first -eq 1 ]; then sums="sha256sums=('$s'"; first=0
    else sums="$sums"$'\n'"            '$s'"; fi
  done
  sums="$sums)"
  awk -v ver="$PKGVER" -v sums="$sums" '
    /^pkgver=/ { print "pkgver=" ver; next }
    /^pkgrel=/ { print "pkgrel=1"; next }
    /^sha256sums=\(/ { print sums; skip = 1 }
    skip { if ($0 ~ /\)[[:space:]]*$/) skip = 0; next }
    { print }
  ' "$pb" > "$pb.tmp" && mv "$pb.tmp" "$pb"
}

update_srcinfo() { # update_srcinfo <dir> <sha>...
  local dir="$1"; shift
  if command -v makepkg > /dev/null 2>&1; then
    (cd "$dir" && makepkg --printsrcinfo > .SRCINFO)
    return
  fi
  # No makepkg (not on Arch): patch the existing .SRCINFO in place.
  local si="$dir/.SRCINFO" old shas
  old="$(awk '$1 == "pkgver" { print $3; exit }' "$si")"
  shas="$*"
  awk -v old="$old" -v oldraw="${old//_/-}" -v new="$PKGVER" -v newraw="$VERSION" -v shas="$shas" '
    BEGIN { n = split(shas, a, " "); i = 0 }
    /^\tpkgver = / { print "\tpkgver = " new; next }
    /^\tpkgrel = / { print "\tpkgrel = 1"; next }
    /^\tsha256sums = / { i++; print "\tsha256sums = " a[i]; next }
    /^\tsource = / {
      gsub("/v" oldraw, "/v" newraw)             # tag in URLs
      gsub("_" oldraw "_", "_" newraw "_")       # deb file name inside the URL
      gsub("-" old "\\.", "-" new ".")           # local names: imagepicker-0.1.0.tar.gz, imagepicker-bin-0.1.0.deb
      gsub("LICENSE-" old "::", "LICENSE-" new "::")
    }
    { print }
  ' "$si" > "$si.tmp" && mv "$si.tmp" "$si"
}

update_pkgbuild "$HERE/imagepicker-bin" "$deb_sha" "$lic_sha"
update_pkgbuild "$HERE/imagepicker" "$src_sha"
update_srcinfo "$HERE/imagepicker-bin" "$deb_sha" "$lic_sha"
update_srcinfo "$HERE/imagepicker" "$src_sha"

echo "Updated to $VERSION (pkgver=$PKGVER). Next: review the diff, then on an Arch box run in each package dir:"
echo "  makepkg -f && namcap PKGBUILD *.pkg.tar.zst"
echo "and push each directory to ssh://aur@aur.archlinux.org/<pkgname>.git"
