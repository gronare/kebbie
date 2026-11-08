#!/bin/sh
# Installs the latest kebbie release as a herdr plugin and runs kebbie setup: curl -fsSL https://kebbie.dev/install.sh | sh
set -eu

repo=gronare/kebbie
min_herdr=0.9.0

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  bold=$(printf '\033[1m'); dim=$(printf '\033[2m'); green=$(printf '\033[32m'); yellow=$(printf '\033[33m'); red=$(printf '\033[31m'); off=$(printf '\033[0m')
else
  bold=""; dim=""; green=""; yellow=""; red=""; off=""
fi

say() { printf '%s\n' "$*"; }
ok() { printf '  %s✓%s %s\n' "$green" "$off" "$*"; }
warn() { printf '  %s!%s %s\n' "$yellow" "$off" "$*"; }
heading() { printf '\n  %s%s%s\n' "$bold" "$*" "$off"; }
row() { printf '    %s%-9s%s %s\n' "$dim" "$1" "$off" "$2"; }
fail() { printf '\n  %s✗%s %s\n\n' "$red" "$off" "$*" >&2; exit 1; }
short() { case "$1" in "$HOME"/*) printf '%s/%s' '~' "${1#"$HOME"/}" ;; *) printf '%s' "$1" ;; esac; }

printf '\n  %skebbie%s %s· install%s\n\n' "$bold" "$off" "$dim" "$off"

missing=""
for tool in git herdr claude jq curl; do
  command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
done
[ -z "$missing" ] || fail "missing:$missing. kebbie needs git, herdr $min_herdr+, Claude Code, jq and curl."

herdr_version=$(herdr --version | awk '{print $2}')
lowest=$(printf '%s\n%s\n' "$min_herdr" "$herdr_version" | sort -t. -k1,1n -k2,2n -k3,3n | head -n 1)
[ "$lowest" = "$min_herdr" ] || fail "herdr $herdr_version is too old, kebbie needs $min_herdr or newer."
ok "git · herdr $herdr_version · Claude Code · jq · curl"

tag=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
tag=${tag##*/}
case "$tag" in v[0-9]*) ;; *) fail "could not read the latest release from github.com/$repo." ;; esac
version=${tag#v}
commit=$(git ls-remote "https://github.com/$repo" "refs/tags/$tag*" 2>/dev/null | sort -k2 -r | head -n 1 | cut -c1-7)

if (exec </dev/tty) 2>/dev/null; then tty=/dev/tty; else tty=""; fi

installed() { herdr plugin list --json 2>/dev/null | jq -r --arg id kebbie '.result.plugins[]? | select(.plugin_id == $id) | "\(.source.kind) \(.plugin_root)"' || true; }

state_home=${XDG_STATE_HOME:-$HOME/.local/state}

case "$(installed)" in local\ *) linked=1 ;; *) linked="" ;; esac

if [ -z "$linked" ]; then
  heading "kebbie $version"
  row from "github.com/$repo, tag $tag${commit:+ ($commit)}"
  row installs "a herdr plugin with a prebuilt binary"
  row adds "the board and the lead as panes, key actions, and a"
  row "" "start hook that brings the lead back when herdr starts"
  row "then" "kebbie setup, which shows everything before it writes"
  row details "github.com/$repo/blob/$tag/herdr-plugin.toml"
  say ""
  if [ -n "$tty" ]; then
    printf '  Install kebbie %s? [Y/n] ' "$version"
    read -r answer <"$tty" || answer=n
    case "$answer" in ""|y|Y|yes|Yes) ;; *) say ""; say "  Nothing installed."; say ""; exit 0 ;; esac
  fi
fi

if [ -n "$linked" ]; then
  heading "kebbie"
  ok "linked from a local checkout, so the install leaves it as it is"
else
  log=$(mktemp)
  if herdr plugin install --yes --ref "$tag" "$repo" >"$log" 2>&1 </dev/null; then
    rm -f "$log"; ok "kebbie $version installed"
  else
    cat "$log" >&2; rm -f "$log"; fail "herdr could not install kebbie $version."
  fi
fi

root=$(installed | cut -d' ' -f2-)
if [ ! -x "$root/target/release/kebbie" ]; then
  plugins=$(dirname -- "$(dirname -- "$(herdr plugin config-dir kebbie)")")
  root=$(ls -dt "$plugins"/github-installations/kebbie-*/*/checkout 2>/dev/null | head -n 1)
fi
[ -x "$root/target/release/kebbie" ] || fail "herdr reports no kebbie binary under ${root:-the plugin}."
case "$(cat "$root/target/release/kebbie.origin" 2>/dev/null)" in
  *" verified") ok "its attestation verified with gh: built by kebbie's release workflow at $tag" ;;
  *" unverified") warn "the binary's origin was not verified: install the GitHub CLI (gh) and log in, and kebbie checks the next release it downloads" ;;
esac

data="${XDG_DATA_HOME:-$HOME/.local/share}/kebbie"
mkdir -p "$data" && ln -sfn "$root" "$data/current"

kebbie="$data/current/target/release/kebbie"
# A kebbie that ran before has a keeper, a board and Claude sessions on the old binary; update --finish moves them over.
if [ -d "$state_home/kebbie" ]; then
  heading "Moving running sessions to kebbie $version"
  "$kebbie" update --finish </dev/null || warn "the running board and sessions stay on the old kebbie until you restart them: kebbie restart, and L on the board for the lead"
fi

printf '\n  %s→ kebbie setup%s\n\n' "$dim" "$off"
if [ -n "$tty" ]; then "$kebbie" setup <"$tty"; else "$kebbie" setup; fi || fail "kebbie setup stopped before it was done. kebbie is installed; run setup again in this terminal: $(short "$kebbie") setup"
