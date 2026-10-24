#!/bin/sh
# herdr installs each update into a new directory; this link gives configs a stable path.
dir="${XDG_DATA_HOME:-$HOME/.local/share}/kebbie"
mkdir -p "$dir" && ln -sfn "$HERDR_PLUGIN_ROOT" "$dir/current"
"$HERDR_PLUGIN_ROOT/target/release/kebbie" lead --revive-board
"$HERDR_PLUGIN_ROOT/target/release/kebbie" keeper --start
