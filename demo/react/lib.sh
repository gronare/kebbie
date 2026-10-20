# Shared by the react scripts: each runs when its stand-in Claude takes a prompt, with the prompt as $1, in that Claude's folder.
D=$KDEMO_DIR
me=$(basename "$PWD")
say() { printf '\n' >> "$D/screens/$me"; printf '%s\n' "$@" >> "$D/screens/$me"; }
mode() { echo "$1" > "$D/modes/$me"; }
more() { printf '%s\n' "$@" >> "$D/screens/$me"; }
