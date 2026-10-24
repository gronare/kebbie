#!/bin/sh
# Puts the kebbie binary at target/release/kebbie: the release's prebuilt one when this checkout is a vX.Y.Z tag, else cargo build --release; a release tag and KEBBIE_CHANNEL=release never build.
set -u
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
binary="$root/target/release/kebbie"
origin_file="$binary.origin"
PATH="${PATH:+$PATH:}$HOME/.cargo/bin"
export PATH

# Users' KELPIE_ names count until they rename them.
KEBBIE_CHANNEL=${KEBBIE_CHANNEL:-${KELPIE_CHANNEL:-}}
KEBBIE_CONFIG=${KEBBIE_CONFIG:-${KELPIE_CONFIG:-}}
KEBBIE_RELEASES_URL=${KEBBIE_RELEASES_URL:-${KELPIE_RELEASES_URL:-}}

say() { printf 'kebbie build: %s\n' "$*" >&2; }

# herdr runs this without KEBBIE_CHANNEL, so the machine config names the channel then.
configured_channel() {
  config=${KEBBIE_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/kebbie/config.toml}
  [ -f "$config" ] || return 0
  awk '/^[[:space:]]*\[/ { section = $0; sub(/#.*/, "", section); gsub(/[[:space:]]/, "", section) } section == "[update]" && /^[[:space:]]*channel[[:space:]]*=/ { sub(/^[^=]*=[[:space:]]*/, ""); quote = substr($0, 1, 1); if (quote != "\"" && quote != "\047") exit; $0 = substr($0, 2); sub(quote ".*", ""); print; exit }' "$config"
}
channel=${KEBBIE_CHANNEL:-$(configured_channel)}

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
  if [ -n "${KEBBIE_RELEASES_URL:-}" ]; then echo "${KEBBIE_RELEASES_URL%/}"; return 0; fi
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
  if [ "$channel" = main ]; then reason="the main channel and --from build from source"; return; fi
  version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
  tag="v$version"
  printf '%s\n' "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' || { reason="version $version is not a release"; return; }
  head=$(git -C "$root" rev-parse HEAD 2>/dev/null) || { reason="$root is not a git checkout"; return; }
  at=$(tag_commit "$tag") || { reason="origin has no tag $tag"; return; }
  [ "$at" = "$head" ] || { reason="the checkout is not at $tag"; return; }
  at_tag=yes
  target=$(triple) || { reason="no prebuilt binary is made for $(uname -s) $(uname -m)"; return; }
  base=$(releases) || { reason="origin is not on GitHub"; return; }
  command -v curl >/dev/null 2>&1 || reason="curl is not installed"
}

download() {
  asset="kebbie-$version-$target.tar.gz"
  work="$root/target/.kebbie-download.$$"
  rm -rf "$work" && mkdir -p "$work" "$root/target/release" || return 1
  trap 'rm -rf "$work"' EXIT
  say "downloading $asset from $base/$tag"
  if [ -n "${KEBBIE_PROGRESS_DIR:-}" ]; then
    size=$(curl -fsSIL --connect-timeout 10 --max-time 30 "$base/$tag/$asset" 2>/dev/null | awk 'tolower($1) == "content-length:" { size = $2 + 0 } END { print size + 0 }')
    printf '%s %s %s\n' "$tag" "${size:-0}" "$work/$asset" > "$KEBBIE_PROGRESS_DIR/download"
  fi
  if ! curl -fsSL --connect-timeout 10 --max-time 300 -o "$work/$asset" "$base/$tag/$asset" || ! curl -fsSL --connect-timeout 10 --max-time 60 -o "$work/SHA256SUMS" "$base/$tag/SHA256SUMS"; then
    say "could not download $asset (no network, or $tag is not published with it yet)"
    unpublished=yes
    return 1
  fi
  want=$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$work/SHA256SUMS")
  got=$(sha256 "$work/$asset")
  if [ -z "$got" ]; then say "neither sha256sum nor shasum is installed to check the download"; return 1; fi
  if [ -z "$want" ] || [ "$want" != "$got" ]; then say "$asset does not match SHA256SUMS, so it is not used"; return 1; fi
  if ! mkdir "$work/unpacked" || ! tar -xzf "$work/$asset" -C "$work/unpacked"; then say "could not unpack $asset"; return 1; fi
  [ -f "$work/unpacked/kebbie" ] || { say "$asset holds no kebbie binary"; return 1; }
  chmod 755 "$work/unpacked/kebbie" && touch "$work/unpacked/kebbie" || return 1
  # Renamed into place, never written into: a running kebbie keeps its file, and macOS kills a binary changed in place.
  mv -f "$work/unpacked/kebbie" "$binary" || return 1
  printf 'downloaded %s %s\n' "$tag" "$target" > "$origin_file"
  rm -rf "$work"
  trap - EXIT
  say "installed the prebuilt kebbie $tag for $target"
}

older() {
  awk -v have="$1" -v need="$2" 'BEGIN { split(have, h, "."); split(need, n, "."); for (i = 1; i <= 3; i++) { if (h[i] + 0 < n[i] + 0) exit 0; if (h[i] + 0 > n[i] + 0) exit 1 } exit 1 }'
}

# Without rustup nothing installs the toolchain rust-toolchain.toml pins, so an older rustc would fail deep in cargo.
check_toolchain() {
  command -v rustup >/dev/null 2>&1 && return 0
  need=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
  have=$(rustc --version 2>/dev/null | awk '{ print $2 }')
  [ -n "$need" ] && [ -n "$have" ] || return 0
  older "${have%%-*}" "$need" || return 0
  short=$(printf '%s\n' "$have" | cut -d . -f 1-2)
  say "Rust $short is too old to build kebbie (it needs $need): install rustup, or switch to the release channel"
  return 1
}

fresh && exit 0

unpublished=no
at_tag=no
check_download
if [ -z "$reason" ]; then
  download && exit 0
  if [ "$unpublished" = yes ]; then
    say "kebbie $version is not published yet; try again in a while"
  else
    say "the release binary of kebbie $version was not installed; try again in a while"
  fi
  exit 1
fi

if [ "$at_tag" = yes ]; then
  say "kebbie $version is a release, which installs only its published binary, and $reason; [update] channel = \"main\" in the machine config builds from source"
  exit 1
fi

if [ "${KEBBIE_CHANNEL:-}" = release ]; then
  case "$reason" in
    "origin has no tag"*) say "kebbie ${version:-?} is not published yet; try again in a while" ;;
    *) say "the release channel installs only published binaries, and $reason; [update] channel = \"main\" in the machine config builds from source" ;;
  esac
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  say "no prebuilt binary fits ($reason), and cargo is not installed to build one. Install Rust (curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh) and run this again, or install a published release: herdr plugin install --ref vX.Y.Z gronare/kebbie"
  exit 1
fi
check_toolchain || exit 1
say "building from source: $reason"
rm -f "$origin_file"
# With KEBBIE_PROGRESS_DIR the update reads cargo's JSON and Compiling lines from there; the end of the log is the error a failed build gives.
if [ -n "${KEBBIE_PROGRESS_DIR:-}" ]; then
  (cd "$root" && cargo build --release --message-format=json-render-diagnostics "$@" > "$KEBBIE_PROGRESS_DIR/cargo.jsonl" 2> "$KEBBIE_PROGRESS_DIR/cargo.log") || { tail -n 30 "$KEBBIE_PROGRESS_DIR/cargo.log" >&2; exit 1; }
else
  (cd "$root" && cargo build --release "$@")
fi
