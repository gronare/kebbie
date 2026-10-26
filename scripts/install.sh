#!/bin/sh
# Installs the latest kebbie release as a herdr plugin and runs kebbie setup: curl -fsSL https://kebbie.dev/install.sh | sh
set -eu

repo=gronare/kebbie
min_herdr=0.9.0

say() { printf '%s\n' "$*"; }
fail() { printf 'kebbie install: %s\n' "$*" >&2; exit 1; }

missing=""
for tool in git herdr claude jq curl; do
  command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
[ -z "$missing" ] || fail "missing:$missing. kebbie needs git, herdr $min_herdr+, Claude Code, jq and curl."

herdr_version=$(herdr --version | awk '{print $2}')
lowest=$(printf '%s\n%s\n' "$min_herdr" "$herdr_version" | sort -t. -k1,1n -k2,2n -k3,3n | head -n 1)
[ "$lowest" = "$min_herdr" ] || fail "herdr $herdr_version is too old, kebbie needs $min_herdr or newer."

tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
tag=${tag##*/}
case "$tag" in v[0-9]*) ;; *) fail "could not read the latest release from github.com/$repo." ;; esac

if (exec </dev/tty) 2>/dev/null; then tty=/dev/tty; else tty=""; fi

installed() { herdr plugin list --json 2>/dev/null | jq -r '.result.plugins[]? | select(.plugin_id == "kebbie") | "\(.source.kind) \(.plugin_root)"' || true; }

case "$(installed)" in
  local\ *) say "kebbie is linked from a local checkout, so the install leaves it as it is." ;;
  *)
    say "Installing kebbie $tag as a herdr plugin."
    if [ -n "$tty" ]; then herdr plugin install --ref "$tag" "$repo" <"$tty"; else herdr plugin install --yes --ref "$tag" "$repo"; fi
    ;;
esac

root=$(installed | cut -d' ' -f2-)
if [ ! -x "$root/target/release/kebbie" ]; then
  plugins=$(dirname -- "$(dirname -- "$(herdr plugin config-dir kebbie)")")
  root=$(ls -dt "$plugins"/github-installations/kebbie-*/*/checkout 2>/dev/null | head -n 1)
fi
[ -x "$root/target/release/kebbie" ] || fail "herdr reports no kebbie binary under ${root:-the plugin}."

data="${XDG_DATA_HOME:-$HOME/.local/share}/kebbie"
mkdir -p "$data" && ln -sfn "$root" "$data/current"

if [ -n "$tty" ]; then "$root/target/release/kebbie" setup <"$tty"; else "$root/target/release/kebbie" setup; fi

say ""
say "Press prefix+space in herdr and tell the lead what you want done."
