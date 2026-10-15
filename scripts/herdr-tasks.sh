#!/bin/sh
set -u

kelpie="$HERDR_PLUGIN_ROOT/target/release/kelpie"
checkout=$(printf '%s' "${HERDR_PLUGIN_CONTEXT_JSON:-}" | jq -r '.worktree.checkout_path // .workspace_cwd // empty' 2>/dev/null)
cd "${checkout:-$HOME}" 2>/dev/null || cd "$HOME" || exit 1

key() {
  old=$(stty -g)
  stty -icanon -echo
  dd bs=1 count=1 2>/dev/null
  stty "$old"
}

ask() {
  printf '%s' "$1" >&2
  read -r answer
  printf '%s' "$answer"
}

pause() {
  printf '\n\033[2mpress any key\033[0m'
  key >/dev/null
}

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "This Space is not a git checkout."
  pause
  exit 0
fi

repo=$(basename "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")")
task=$(git branch --show-current)
case "$(basename "$PWD")" in
  wt[0-9]*."$repo") ;;
  *) task="" ;;
esac

printf '\033[1;35mkelpie\033[0m  %s' "$repo"
[ -n "$task" ] && printf '  ·  task \033[33m%s\033[0m' "$task"
printf '\n\n'
"$kelpie" list 2>&1 | sed 's/^/  /'
printf '\n  \033[1ms\033[0m start a task'
[ -n "$task" ] && printf '   \033[1ml\033[0m land   \033[1md\033[0m done   \033[1mx\033[0m drop'
printf '   \033[1mq\033[0m close\n\n'

case "$(key)" in
  s)
    name=$(ask "Task name: ")
    [ -n "$name" ] || exit 0
    prompt=$(ask "Prompt for Claude (optional): ")
    if [ -n "$prompt" ]; then "$kelpie" start "$name" "$prompt"; else "$kelpie" start "$name"; fi || pause
    ;;
  l)
    [ -n "$task" ] || exit 0
    if output=$("$kelpie" land 2>&1); then
      printf '%s\n' "$output"
      push=$(printf '%s\n' "$output" | sed -n 's/^Push with: *//p')
      [ "$(ask "
Push now? [y/N] ")" = y ] && sh -c "$push" && "$kelpie" "done"
    else
      printf '%s\n' "$output"
    fi
    pause
    ;;
  d)
    [ -n "$task" ] || exit 0
    "$kelpie" "done"
    pause
    ;;
  x)
    [ -n "$task" ] || exit 0
    if ! "$kelpie" drop; then
      [ "$(ask "
Discard it anyway? [y/N] ")" = y ] && "$kelpie" drop --force
    fi
    pause
    ;;
esac
