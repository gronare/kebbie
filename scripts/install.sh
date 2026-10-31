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

installed() { herdr plugin list --json 2>/dev/null | jq -r --arg id "${1:-kebbie}" '.result.plugins[]? | select(.plugin_id == $id) | "\(.source.kind) \(.plugin_root)"' || true; }

state_home=${XDG_STATE_HOME:-$HOME/.local/state}
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
herdr_config=${HERDR_CONFIG_PATH:-$config_home/herdr/config.toml}

# The old keeper writes keeper.log as it exits, which recreates its state dir after the move.
only_logs() { [ -d "$1" ] && [ -z "$(find "$1" -mindepth 1 ! -name '*.log' -print -quit)" ]; }
leftover() { [ -e "$2" ] && only_logs "$1"; }
kelpie_lines='command = "kelpie\.|/kelpie/current/|description = "kelpie|# kelpie.s |a kelpie task'

move_dir() {
  if [ ! -e "$1" ]; then return 0; fi
  if leftover "$1" "$2"; then rm -r "$1"; return 0; fi
  if [ -e "$2" ]; then rm -r "$1"; ok "removed the old $(short "$1"); kebbie runs on $(short "$2")"; return 0; fi
  mv "$1" "$2" && moved=1
}

old_plugin=$(installed kelpie)
left() { [ -e "$1" ] && ! leftover "$1" "$2"; }
if [ -n "$old_plugin" ] || left "$config_home/kelpie" "$config_home/kebbie" || left "$state_home/kelpie" "$state_home/kebbie" || { [ -f "$herdr_config" ] && grep -qE "$kelpie_lines" "$herdr_config"; }; then had_kelpie=1; else had_kelpie=""; fi

move_from_kelpie() {
  [ -n "$had_kelpie" ] || return 0
  heading "Moving kelpie over to kebbie"
  moved=""; removed=""
  move_dir "$config_home/kelpie" "$config_home/kebbie"
  move_dir "$state_home/kelpie" "$state_home/kebbie"
  [ -z "$moved" ] || ok "settings and state now live under kebbie"
  if [ -f "$herdr_config" ] && grep -qE "$kelpie_lines" "$herdr_config"; then
    cp "$herdr_config" "$herdr_config.kelpie-backup"
    sed -e 's/command = "kelpie\./command = "kebbie./' -e 's#/kelpie/current/#/kebbie/current/#' -e 's/description = "kelpie/description = "kebbie/' -e "s/# kelpie's /# kebbie's /" -e 's/a kelpie task/a kebbie task/' "$herdr_config.kelpie-backup" > "$herdr_config"
    herdr server reload-config >/dev/null 2>&1 || true
    ok "herdr keys and sidebar run kebbie   $dim(backup: $(short "$herdr_config").kelpie-backup)$off"
  fi
  case "$old_plugin" in
    local\ *) herdr plugin unlink kelpie >/dev/null 2>&1 && ok "kelpie unlinked from herdr   $dim(its checkout at $(short "${old_plugin#local }") stays)$off" ;;
    ?*) herdr plugin uninstall kelpie >/dev/null 2>&1 && removed=1 ;;
  esac
  if [ -L "$HOME/.local/bin/kelpie" ]; then rm "$HOME/.local/bin/kelpie"; fi
  old_data="${XDG_DATA_HOME:-$HOME/.local/share}/kelpie"
  if [ -d "$old_data" ]; then rm -r "$old_data"; fi
  for id in $(claude plugin list --json 2>/dev/null | jq -r '.[]?.id // empty' | grep '^kelpie@' || true); do
    claude plugin uninstall "$id" >/dev/null 2>&1 && removed=1
  done
  if claude plugin marketplace remove kelpie >/dev/null 2>&1; then removed=1; fi
  [ -z "$removed" ] || ok "old kelpie plugin removed from herdr and Claude Code"
}

case "$(installed)" in local\ *) linked=1 ;; *) linked="" ;; esac

if [ -z "$linked" ]; then
  heading "kebbie $version"
  row from "github.com/$repo, tag $tag${commit:+ ($commit)}"
  row installs "a herdr plugin with a prebuilt binary"
  row adds "the board and the lead as panes, key actions, and a"
  row "" "start hook that brings the lead back when herdr starts"
  [ -z "$had_kelpie" ] || row replaces "kelpie, moving its settings, state and herdr keys over"
  row "then" "kebbie setup, which shows everything before it writes"
  row details "github.com/$repo/blob/$tag/herdr-plugin.toml"
  say ""
  if [ -n "$tty" ]; then
    printf '  Install kebbie %s? [Y/n] ' "$version"
    read -r answer <"$tty" || answer=n
    case "$answer" in ""|y|Y|yes|Yes) ;; *) say ""; say "  Nothing installed."; say ""; exit 0 ;; esac
  fi
fi

move_from_kelpie

if [ -n "$linked" ]; then
  heading "kebbie"
  ok "linked from a local checkout, so the install leaves it as it is"
else
  [ -z "$had_kelpie" ] || heading "Installing kebbie $version"
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
