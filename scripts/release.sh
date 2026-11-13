#!/usr/bin/env bash
# Release helper; docs/RELEASE.md is the procedure.
#
#   scripts/release.sh version          the workspace version, from Cargo.toml
#   scripts/release.sh notes VERSION    the release notes: each crate's CHANGELOG section for VERSION
#   scripts/release.sh verify VERSION   every check of releasing VERSION short of publishing
#
# The one version source is `[workspace.package] version` in Cargo.toml; both
# crates inherit it, so they are always released together at one version.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
crates=(git-process git-evidence) # publication order: git-evidence depends on git-process

fail() { echo "release: $*" >&2; exit 1; }

workspace_version() {
  awk '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version *=/ {
      gsub(/^version *= *"|".*$/, ""); print; exit
    }' Cargo.toml
}

# The body of `## [VERSION] - DATE` in a changelog, up to the next `## ` heading.
section() {
  awk -v version="$2" '
    index($0, "## [" version "]") == 1 { found = 1; next }
    found && /^## / { exit }
    found { print }' "$1"
}

notes() {
  local version="$1" crate body
  for crate in "${crates[@]}"; do
    body="$(section "crates/$crate/CHANGELOG.md" "$version")"
    [ -n "$(printf '%s' "$body" | tr -d '[:space:]')" ] \
      || fail "crates/$crate/CHANGELOG.md has no filled '## [$version]' section"
    printf '## %s %s\n%s\n\n' "$crate" "$version" "$body"
  done
}

verify() {
  local version="$1" crate found
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "'$version' is not X.Y.Z"
  found="$(workspace_version)"
  [ "$found" = "$version" ] || fail "Cargo.toml's workspace version is $found, not $version"
  for crate in "${crates[@]}"; do
    found="$(cargo pkgid --locked -p "$crate")"
    [ "${found##*[#@]}" = "$version" ] || fail "$crate is $found, not $version"
    grep -Eq "^## \[$version\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" "crates/$crate/CHANGELOG.md" \
      || fail "crates/$crate/CHANGELOG.md has no '## [$version] - YYYY-MM-DD' heading"
  done
  notes "$version" >/dev/null
  just check
  cargo publish --workspace --dry-run --locked --allow-dirty
  echo "release: $version is ready to tag (v$version)"
}

case "${1:-}" in
  version) workspace_version ;;
  notes) [ $# -eq 2 ] || fail "usage: release.sh notes VERSION"; notes "$2" ;;
  verify) [ $# -eq 2 ] || fail "usage: release.sh verify VERSION"; verify "$2" ;;
  *) fail "usage: release.sh version | notes VERSION | verify VERSION" ;;
esac
