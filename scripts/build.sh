#!/bin/sh
# Puts the kelpie binary at target/release/kelpie: the release's prebuilt one when this checkout is a vX.Y.Z tag, else cargo build --release.
set -u
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
binary="$root/target/release/kelpie"
origin_file="$binary.origin"
failed="$root/target/kelpie-download.failed"
PATH="${PATH:+$PATH:}$HOME/.cargo/bin"
export PATH

say() { printf 'kelpie build: %s\n' "$*" >&2; }

fresh() {
  [ -x "$binary" ] || return 1
  newer=$(cd "$root" && find src roles Cargo.toml Cargo.lock herdr-plugin.toml .claude-plugin/plugin.json -newer "$binary" 2>/dev/null | head -n 1)
  [ -z "$newer" ]
}

triple() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64 | Darwin-aarch64) echo aarch64-apple-darwin ;;
    Darwin-x86_64) echo x86_64-apple-darwin ;;
    Linux-x86_64 | Linux-amd64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64 | Linux-arm64) echo aarch64-unknown-linux-gnu ;;
    *) return 1 ;;
  esac
}

releases() {
  if [ -n "${KELPIE_RELEASES_URL:-}" ]; then echo "${KELPIE_RELEASES_URL%/}"; return 0; fi
  url=$(git -C "$root" remote get-url origin 2>/dev/null) || return 1
  case "$url" in
    https://github.com/*/* | git@github.com:*/* | ssh://git@github.com/*/*) ;;
    *) return 1 ;;
  esac
  repo=${url#*github.com[:/]}
  repo=${repo%.git}
  echo "https://github.com/$repo/releases/download"
}

# The tag's commit, from a local tag or else from origin: herdr's shallow clone fetches no tags.
tag_commit() {
  git -C "$root" rev-parse -q --verify "refs/tags/$1^{commit}" 2>/dev/null && return 0
  refs=$(git -C "$root" ls-remote --tags origin "refs/tags/$1" "refs/tags/$1^{}" 2>/dev/null) || return 1
  peeled=$(printf '%s\n' "$refs" | awk -v ref="refs/tags/$1^{}" '$2 == ref { print $1 }')
  plain=$(printf '%s\n' "$refs" | awk -v ref="refs/tags/$1" '$2 == ref { print $1 }')
  [ -n "$peeled$plain" ] && echo "${peeled:-$plain}"
}

sha256() { { sha256sum "$1" || shasum -a 256 "$1"; } 2>/dev/null | cut -d ' ' -f 1; }

check_download() {
  reason=""
  if [ "${KELPIE_CHANNEL:-}" = main ]; then reason="the main channel builds from source"; return; fi
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
  tag="v$version"
  printf '%s\n' "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' || { reason="version $version is not a release"; return; }
  head=$(git -C "$root" rev-parse HEAD 2>/dev/null) || { reason="$root is not a git checkout"; return; }
  at=$(tag_commit "$tag") || { reason="origin has no tag $tag"; return; }
  [ "$at" = "$head" ] || { reason="the checkout is not at $tag"; return; }
  target=$(triple) || { reason="no prebuilt binary is made for $(uname -s) $(uname -m)"; return; }
  base=$(releases) || { reason="origin is not on GitHub"; return; }
  if [ -n "$(find "$failed" -mmin -10 2>/dev/null)" ]; then reason="a download failed less than 10 minutes ago"; return; fi
  command -v curl >/dev/null 2>&1 || reason="curl is not installed"
}

download() {
  asset="kelpie-$version-$target.tar.gz"
  work="$root/target/.kelpie-download.$$"
  rm -rf "$work" && mkdir -p "$work" "$root/target/release" || return 1
  trap 'rm -rf "$work"' EXIT
  say "downloading $asset from $base/$tag"
  if ! curl -fsSL --connect-timeout 10 --max-time 300 -o "$work/$asset" "$base/$tag/$asset" || ! curl -fsSL --connect-timeout 10 --max-time 60 -o "$work/SHA256SUMS" "$base/$tag/SHA256SUMS"; then
    say "could not download $asset (no network, or $tag is not published with it yet)"
    return 1
  fi
  want=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$work/SHA256SUMS")
  got=$(sha256 "$work/$asset")
  if [ -z "$got" ]; then say "neither sha256sum nor shasum is installed to check the download"; return 1; fi
  if [ -z "$want" ] || [ "$want" != "$got" ]; then say "$asset does not match SHA256SUMS, so it is not used"; return 1; fi
  if ! mkdir "$work/unpacked" || ! tar -xzf "$work/$asset" -C "$work/unpacked"; then say "could not unpack $asset"; return 1; fi
  [ -f "$work/unpacked/kelpie" ] || { say "$asset holds no kelpie binary"; return 1; }
  chmod 755 "$work/unpacked/kelpie" && touch "$work/unpacked/kelpie" || return 1
  # Renamed into place, never written into: a running kelpie keeps its file, and macOS kills a binary changed in place.
  mv -f "$work/unpacked/kelpie" "$binary" || return 1
  printf 'downloaded %s %s\n' "$tag" "$target" > "$origin_file"
  rm -rf "$work"
  trap - EXIT
  say "installed the prebuilt kelpie $tag for $target"
}

fresh && exit 0

check_download
if [ -z "$reason" ]; then
  if download; then
    rm -f "$failed"
    exit 0
  fi
  mkdir -p "$root/target" && : > "$failed"
  reason="the download failed"
fi

if ! command -v cargo >/dev/null 2>&1; then
  say "no prebuilt binary fits ($reason), and cargo is not installed to build one. Install Rust (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh) and run this again, or install a published release: herdr plugin install --ref vX.Y.Z gronare/kelpie"
  exit 1
fi
say "building from source: $reason"
rm -f "$origin_file"
(cd "$root" && cargo build --release "$@")
