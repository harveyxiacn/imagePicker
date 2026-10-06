#!/usr/bin/env bash
# Bump (or verify) the imagePicker version across every manifest.
#
#   scripts/bump-version.sh 0.2.0            # rewrite all manifests, verify, print tag commands
#   scripts/bump-version.sh --check          # verify all manifests agree (CI job: version-consistency)
#   scripts/bump-version.sh --check v0.2.0   # additionally require the version to equal the tag
#   scripts/bump-version.sh --print          # print the current workspace version
#
# Manifests kept in sync:
#   Cargo.toml                                [workspace.package] version
#   Cargo.lock                                version of every workspace crate
#   apps/desktop/src-tauri/tauri.conf.json    top-level "version"
#   apps/desktop/package.json                 "version"
#   web/package.json                          "version"
#   ai-worker/pyproject.toml                  [project] version
#
# This script never commits, tags or pushes. Needs bash + awk + sed (POSIX/GNU/BSD).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CARGO_TOML=Cargo.toml
CARGO_LOCK=Cargo.lock
TAURI_CONF=apps/desktop/src-tauri/tauri.conf.json
DESKTOP_PKG=apps/desktop/package.json
WEB_PKG=web/package.json
PYPROJECT=ai-worker/pyproject.toml

SEMVER_RE='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'

# --- readers ---------------------------------------------------------------
# version inside a TOML section: toml_version <file> <section header>
toml_version() {
  awk -v want="$2" '
    /^\[/ { sec = $0 }
    sec == want && /^version[ ]*=/ { gsub(/^[^"]*"|".*$/, ""); print; exit }
  ' "$1"
}
get_cargo_toml() { toml_version "$CARGO_TOML" "[workspace.package]"; }
get_pyproject() { toml_version "$PYPROJECT" "[project]"; }
get_json() { # top-level (2-space indented) "version" key
  sed -n 's/^  "version":[ ]*"\([^"]*\)".*/\1/p' "$1" | head -n 1
}
# Names of all workspace member crates, one per line.
workspace_crates() {
  sed -n 's/^members = \[\(.*\)\]/\1/p' "$CARGO_TOML" | tr ',' '\n' | tr -d ' "[]' | while read -r dir; do
    if [ -n "$dir" ]; then sed -n 's/^name = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -n 1; fi
  done
}
lock_versions() { # "crate version" for each workspace crate present in Cargo.lock
  local crates; crates="$(workspace_crates)"
  awk -v crates="$crates" '
    BEGIN { n = split(crates, a, "\n"); for (i = 1; i <= n; i++) want[a[i]] = 1 }
    /^\[\[package\]\]/ { name = ""; next }
    /^name = / { t = $0; gsub(/name = "|"/, "", t); name = t; next }
    /^version = / { t = $0; gsub(/version = "|"/, "", t); if (name in want) print name, t }
  ' "$CARGO_LOCK"
}

# --- check -----------------------------------------------------------------
check() {
  local expected="${1:-}" ok=1 ref f crate ver
  declare -A seen=(
    ["$CARGO_TOML"]="$(get_cargo_toml)"
    ["$TAURI_CONF"]="$(get_json "$TAURI_CONF")"
    ["$DESKTOP_PKG"]="$(get_json "$DESKTOP_PKG")"
    ["$WEB_PKG"]="$(get_json "$WEB_PKG")"
    ["$PYPROJECT"]="$(get_pyproject)"
  )
  ref="${seen[$CARGO_TOML]}"
  if [ -z "$ref" ]; then echo "error: cannot read version from $CARGO_TOML" >&2; return 1; fi
  expected="${expected#v}"
  for f in "$CARGO_TOML" "$TAURI_CONF" "$DESKTOP_PKG" "$WEB_PKG" "$PYPROJECT"; do
    printf '  %-45s %s\n' "$f" "${seen[$f]:-<missing>}"
    if [ "${seen[$f]}" != "$ref" ]; then ok=0; fi
  done
  while read -r crate ver; do
    if [ -z "$crate" ]; then continue; fi
    if [ "$ver" != "$ref" ]; then
      printf '  %-45s %s (crate %s)\n' "$CARGO_LOCK" "$ver" "$crate"
      ok=0
    fi
  done < <(lock_versions)
  if ! [[ "$ref" =~ $SEMVER_RE ]]; then echo "error: '$ref' is not valid SemVer" >&2; ok=0; fi
  if [ -n "$expected" ] && [ "$expected" != "$ref" ]; then
    echo "error: manifests say $ref but expected $expected (tag/argument)" >&2; ok=0
  fi
  if [ "$ok" -eq 1 ]; then echo "OK: all manifests at $ref"; return 0; fi
  echo "error: version mismatch between manifests" >&2
  return 1
}

# --- writers ---------------------------------------------------------------
replace_toml() { # replace_toml <file> <section> <version>
  awk -v want="$2" -v v="$3" '
    /^\[/ { sec = $0 }
    sec == want && !done && /^version[ ]*=/ { print "version = \"" v "\""; done = 1; next }
    { print }
  ' "$1" > "$1.tmp" && mv "$1.tmp" "$1"
}
replace_json() { # replace_json <file> <version>  (first top-level "version" only)
  awk -v v="$2" '
    !done && /^  "version":/ { sub(/"version":[ ]*"[^"]*"/, "\"version\": \"" v "\""); done = 1 }
    { print }
  ' "$1" > "$1.tmp" && mv "$1.tmp" "$1"
}
bump() {
  local v="$1" crates
  replace_toml "$CARGO_TOML" "[workspace.package]" "$v"
  replace_json "$TAURI_CONF" "$v"
  replace_json "$DESKTOP_PKG" "$v"
  replace_json "$WEB_PKG" "$v"
  replace_toml "$PYPROJECT" "[project]" "$v"
  # Cargo.lock: only workspace crates (third-party entries stay untouched).
  crates="$(workspace_crates)"
  awk -v crates="$crates" -v v="$v" '
    BEGIN { n = split(crates, a, "\n"); for (i = 1; i <= n; i++) want[a[i]] = 1 }
    /^\[\[package\]\]/ { name = "" }
    /^name = / { t = $0; gsub(/name = "|"/, "", t); name = t }
    /^version = / && (name in want) { print "version = \"" v "\""; next }
    { print }
  ' "$CARGO_LOCK" > "$CARGO_LOCK.tmp" && mv "$CARGO_LOCK.tmp" "$CARGO_LOCK"
}

# --- main ------------------------------------------------------------------
case "${1:-}" in
  ""|-h|--help)
    sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
  --check) shift; check "${1:-}"; exit $? ;;
  --print) get_cargo_toml; exit 0 ;;
esac

NEW="${1#v}"
if ! [[ "$NEW" =~ $SEMVER_RE ]]; then
  echo "error: '$NEW' is not a valid SemVer version (expected X.Y.Z[-pre])" >&2; exit 2
fi
# WiX (MSI) only accepts a numeric pre-release identifier (<= 65535).
if [[ "$NEW" == *-* ]] && ! [[ "${NEW#*-}" =~ ^[0-9]+$ ]]; then
  echo "warning: pre-release '${NEW#*-}' is not numeric; the Windows MSI bundle will fail for this version" >&2
  echo "         (use e.g. 0.2.0-1, or build only NSIS for pre-releases)." >&2
fi
OLD="$(get_cargo_toml)"
echo "Bumping $OLD -> $NEW"
bump "$NEW"
check "$NEW"

cat <<MSG

Next steps (nothing was committed, tagged or pushed):

  git cliff --tag v$NEW --output CHANGELOG.md          # optional: refresh the changelog
  git add -A Cargo.toml Cargo.lock apps/desktop web/package.json ai-worker/pyproject.toml CHANGELOG.md
  git commit -m "chore(release): v$NEW"
  git tag -a v$NEW -m "imagePicker v$NEW"
  git push origin main v$NEW                           # the tag push triggers .github/workflows/release.yml
MSG
