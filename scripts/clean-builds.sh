#!/bin/sh
# Removes what builds leave under target/ except target/release, which holds the kebbie this checkout installs: scripts/clean-builds.sh [<checkout>]
set -eu
root=${1:-$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)}
target="$root/target"
{ [ -d "$target" ] && [ ! -L "$target" ]; } || exit 0
before=$(du -sk "$target" | cut -f 1)
for entry in "$target"/*; do
  case "${entry##*/}" in release | CACHEDIR.TAG | '*') continue ;; esac
  rm -rf "$entry"
done
after=$(du -sk "$target" | cut -f 1)
echo "freed $(((before - after) / 1024)) MB of build output under target/; target/release stays"
