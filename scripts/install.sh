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

installed() { herdr plugin list --json 2>/dev/null | jq -r --arg id "${1:-kebbie}" '.result.plugins[]? | select(.plugin_id == $id) | "\(.source.kind) \(.plugin_root)"' || true; }

state_home=${XDG_STATE_HOME:-$HOME/.local/state}
config_home=${XDG_CONFIG_HOME:-$HOME/.config}
herdr_config=${HERDR_CONFIG_PATH:-$config_home/herdr/config.toml}

move_dir() {
  if [ ! -e "$1" ]; then return 0; fi
  if [ -e "$2" ]; then say "  both $1 and $2 exist; kebbie uses $2, so merge or remove $1 yourself"; return 0; fi
  mv "$1" "$2" && say "  moved $1 to $2"
}

move_from_kelpie() {
  old_plugin=$(installed kelpie)
  [ -n "$old_plugin" ] || [ -e "$config_home/kelpie" ] || [ -e "$state_home/kelpie" ] || return 0
  say "Found kelpie, kebbie's earlier name. Moving it over to kebbie:"
  case "$old_plugin" in
    local\ *) say "  kelpie is linked from ${old_plugin#local }; unlink it with herdr plugin unlink kelpie once you have checked out kebbie there" ;;
    ?*) herdr plugin uninstall kelpie >/dev/null && say "  removed the kelpie plugin from herdr" ;;
  esac
  move_dir "$config_home/kelpie" "$config_home/kebbie"
  move_dir "$state_home/kelpie" "$state_home/kebbie"
  if [ -L "$HOME/.local/bin/kelpie" ]; then rm "$HOME/.local/bin/kelpie" && say "  removed the ~/.local/bin/kelpie link"; fi
  old_data="${XDG_DATA_HOME:-$HOME/.local/share}/kelpie"
  if [ -d "$old_data" ]; then rm -r "$old_data" && say "  removed $old_data"; fi
  if [ -f "$herdr_config" ] && grep -q 'command = "kelpie\.' "$herdr_config"; then
    cp "$herdr_config" "$herdr_config.kelpie-backup"
    sed 's/command = "kelpie\./command = "kebbie./' "$herdr_config.kelpie-backup" > "$herdr_config"
    say "  pointed herdr's kelpie keys at kebbie (the old file is $herdr_config.kelpie-backup)"
  fi
  for id in $(claude plugin list --json 2>/dev/null | jq -r '.[]?.id // empty' | grep '^kelpie@' || true); do
    claude plugin uninstall "$id" >/dev/null 2>&1 && say "  removed the $id plugin from Claude Code"
  done
  if claude plugin marketplace remove kelpie >/dev/null 2>&1; then say "  removed the kelpie marketplace from Claude Code"; fi
}

move_from_kelpie

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
