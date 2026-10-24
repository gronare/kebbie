#!/bin/sh
# Usage: review.sh split [base [path ...]] | history | hook
# Short flags: BSD awk/wc/cut on macOS lack the GNU long forms.

set -eu

script="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
herdr="${HERDR_BIN_PATH:-herdr}"
tab="$(printf '\t')"
state="${KEBBIE_STATE_DIR:-${KELPIE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/kebbie}}"

# origin/HEAD is unset in some older clones.
default_branch() {
  git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null && return
  for candidate in origin/main origin/master; do
    git show-ref --verify --quiet "refs/remotes/$candidate" && echo "$candidate" && return
  done
  return 1
}

in_linked_worktree() {
  [ "$(git rev-parse --path-format=absolute --git-dir)" != "$(git rev-parse --path-format=absolute --git-common-dir)" ]
}

# A worktree's base is its fork point, so its commits stay in the review as the default branch moves on.
worktree_base() {
  in_linked_worktree && branch=$(default_branch) && git merge-base HEAD "$branch"
}

# A review's base "fork" is resolved on every use: a sha frozen at opening would count the default branch's commits as the task's after a rebase.
resolve_base() {
  if [ "$1" = fork ]; then worktree_base 2>/dev/null || echo HEAD; else echo "$1"; fi
}

describe_base() {
  if [ "$1" = "$(worktree_base 2>/dev/null)" ]; then
    echo "$(git branch --show-current) since $(default_branch) ($(git rev-parse --short "$1"))"
  else
    echo "against $1"
  fi
}

default_base() {
  if worktree_base >/dev/null 2>&1; then
    echo fork
  elif [ -n "$(git status --porcelain)" ]; then
    echo HEAD
  else
    git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null || echo HEAD
  fi
}

label_of() {
  if [ "$(cat "$1/base")" = fork ]; then describe_base "$(resolve_base fork)"; else cat "$1/label"; fi
}

review_dir() {
  echo "$state/reviews/$(echo "$1" | tr ':/' '__')"
}

changed_files() {
  git diff --numstat "$(resolve_base "$1")" | awk -F '\t' '{ printf "%s\t+%s -%s\n", $3, $1, $2 }'
  git ls-files --others --exclude-standard | while read -r path; do
    printf '%s\t+%s new\n' "$path" "$(wc -l < "$path" | tr -d ' ')"
  done
}

status_of() {
  if git ls-files --error-unmatch -- "$2" >/dev/null 2>&1 || git cat-file -e "$1:$2" 2>/dev/null; then
    git diff --name-status "$1" -- "$2" | cut -c 1
  else
    echo A
  fi
}

unified_diff() {
  base=$(resolve_base "$1")
  case "$(status_of "$base" "$2")" in
    A) git diff --color="$3" --no-index /dev/null "$2" || true ;;
    *) git diff --color="$3" "$base" -- "$2" ;;
  esac
}

# Read marks are keyed on contents: committing keeps a file read, any edit brings it back.
read_marks() {
  mkdir -p "$state"
  echo "$state/$(git rev-parse --show-toplevel | tr '/' '_')"
}

fingerprint() {
  if [ -e "$1" ]; then git hash-object -- "$1"; else echo deleted; fi
}

is_read() {
  grep -qxF "$1$tab$(fingerprint "$1")" "$(read_marks)" 2>/dev/null
}

unmark_read() {
  marks=$(read_marks)
  [ -f "$marks" ] || return 0
  awk -F '\t' -v path="$1" '$1 != path' "$marks" > "$marks.tmp" && mv "$marks.tmp" "$marks"
}

mark_read() {
  unmark_read "$1"
  printf '%s\t%s\n' "$1" "$(fingerprint "$1")" >> "$(read_marks)"
}

toggle_read() {
  if is_read "$1"; then unmark_read "$1"; else mark_read "$1"; fi
}

categorized() {
  changes=$(changed_files "$(cat "$1/base")")
  if [ -f "$1/scope" ]; then
    sed 's|^\./||' "$1/scope" | awk '!seen[$0]++' | while read -r path; do
      echo "$changes" | awk -F '\t' -v path="$path" '$1 == path { print "scope\t" $0 }'
    done
    echo "$changes" | awk -F '\t' 'NR == FNR { sub(/^\.\//, ""); scoped[$0]; next } !($1 in scoped) { print "other\t" $0 }' "$1/scope" -
  elif [ -f "$1/baseline" ]; then
    new_work=""
    while IFS="$tab" read -r path stats; do
      [ -n "$path" ] || continue
      if grep -qxF "$path$tab$(fingerprint "$path")" "$1/baseline"; then
        printf 'other\t%s\t%s\n' "$path" "$stats"
      else
        new_work="$new_work$path
"
      fi
    done <<EOF
$changes
EOF
    [ -n "$new_work" ] || return 0
    echo "$new_work" | while read -r path; do [ -e "$path" ] && printf '%s\0' "$path"; done | xargs -0 ls -td -- 2>/dev/null |
      while read -r path; do echo "$changes" | awk -F '\t' -v path="$path" '$1 == path { print "scope\t" $0 }'; done
    echo "$new_work" | while read -r path; do
      [ -n "$path" ] && [ ! -e "$path" ] && echo "$changes" | awk -F '\t' -v path="$path" '$1 == path { print "scope\t" $0 }'
    done
    return 0
  else
    echo "$changes" | sed "s/^/scope$tab/"
  fi
  return 0
}

row() {
  added=${2%% *} deleted=${2#* }
  if [ "$deleted" = new ]; then deleted=""; note=" new"; else note=""; fi
  printf '%s\t%s\033[32m%6s\033[0m%s \033[31m%5s\033[0m%s  %s\033[90m%s%s\033[0m\n' \
    "$1" "$4" "$added" "$4" "$deleted" "$4" "$1" "$note" "$3"
}

rows() {
  dim="$(printf '\033[2m')"
  in_scope=0 others=0 read_count=0 main="" rest="" read=""
  while IFS="$tab" read -r kind path stats; do
    [ -n "$path" ] || continue
    if [ "$kind" = other ]; then
      others=$((others + 1))
      [ -e "$1/view/other" ] || continue
    else
      in_scope=$((in_scope + 1))
    fi
    if is_read "$path"; then
      read_count=$((read_count + 1))
      read="$read$(row "$path" "$stats" "  ✓ read" "$dim")
"
    elif [ "$kind" = other ]; then
      rest="$rest$(row "$path" "$stats" "  other" "")
"
    else
      main="$main$(row "$path" "$stats" "" "")
"
    fi
  done <<EOF
$(categorized "$1")
EOF
  count() { printf '\033[33m%s\033[0m %s' "$1" "$2"; }
  status="$(printf '\033[35m%s\033[0m' "$(label_of "$1")")"
  if [ -f "$1/scope" ] || [ -f "$1/baseline" ]; then
    status="$status  ·  $(count "$in_scope" "to review")"
    if [ "$others" -gt 0 ]; then
      if [ -e "$1/view/other" ]; then status="$status  ·  $(count "$others" "other shown")"; else status="$status  ·  $(count "$others" "other hidden")"; fi
    fi
  else
    status="$status  ·  $(count "$in_scope" changed)"
  fi
  if [ "$read_count" -gt 0 ]; then
    if [ -e "$1/view/read" ]; then status="$status  ·  $(count "$read_count" "read shown")"; else status="$status  ·  $(count "$read_count" "read hidden")"; fi
  fi
  printf '\t%s  ·  \033[1m?\033[0m help\n' "$status"
  printf '%s' "$main"
  printf '%s' "$rest"
  [ -e "$1/view/read" ] && printf '%s' "$read"
  return 0
}

help() {
  heading() { printf '\n \033[1;35m%s\033[0m\n' "$1"; }
  key() { printf '   \033[1m%-12s\033[0m%s\n' "$1" "$2"; }
  heading "File list"
  key "type" "filter; !word hides matches (!spec/ is prefilled)"
  key "enter" "open the file's diff"
  key "ctrl-r" "mark the file read, or unread"
  key "ctrl-h" "show or hide read files"
  key "ctrl-o" "show or hide changes outside the review"
  key "ctrl-c" "close the split (or close the pane from herdr)"
  heading "Diff"
  key "c" "comment on the line, or a visual selection"
  key "o" "open the file over the diff, at that line"
  key "-" "open the file's folder over the diff"
  key "u" "reload the diff when the file changed"
  key "w" "toggle line wrap"
  key "r" "mark read and go back to the list"
  key "q  esc" "back to the list"
  heading "Opened files (the float)"
  key "leader m" "comment on the line, or a visual selection (leader is space in many configs)"
  key "ctrl-q" "back to the diff"
  printf '   %s\n' "Everything else is your usual nvim. Saved edits are" "reported to Claude when you go back to the diff."
  heading "Good to know"
  printf '   %s\n' "Comments are typed into Claude's input box; nothing is" "sent until you press Enter there." "" "The list follows changes by itself; a read file comes back" "when its contents change." "" "Move the cursor to get the diff preview back."
}

open() {
  base=$(cat "$1/base")
  unified_diff "$base" "$2" never |
    REVIEW_PATH="$2" REVIEW_BASE="$base" REVIEW_AGENT_PANE="$(cat "$1/agent")" REVIEW_SCRIPT="$script" \
    nvim -S "$(dirname "$script")/review.lua" - || true
}

signature() {
  base=$(resolve_base "$(cat "$1/base")")
  {
    echo "$base"
    cat "$1/scope" 2>/dev/null || true
    git diff "$base" 2>/dev/null
    git ls-files --others --exclude-standard | while read -r path; do echo "$path $(git hash-object -- "$path")"; done
  } | cksum
}

reload_list() {
  [ -S "$1/fzf.sock" ] || return 0
  curl -s --max-time 2 --unix-socket "$1/fzf.sock" -X POST http://localhost -d "reload-sync(sh '$script' rows '$1')" >/dev/null || true
}

watch() {
  last=$(signature "$1")
  while kill -0 "$2" 2>/dev/null; do
    sleep 2
    now=$(signature "$1")
    if [ "$now" != "$last" ]; then
      reload_list "$1"
      last=$now
    fi
  done
}

list() {
  d=$1
  mkdir -p "$d/view"
  rm -f "$d/fzf.sock"
  sh "$script" watch "$d" "$$" &
  watcher=$!
  trap 'kill "$watcher" 2>/dev/null; rm -rf "$d/view" "$d/fzf.sock"' EXIT
  reload="reload(sh '$script' rows '$d')"
  flip() { echo "execute-silent(if [ -e '$d/view/$1' ]; then rm '$d/view/$1'; else touch '$d/view/$1'; fi)+$reload"; }
  sh "$script" rows "$d" | fzf \
    --listen "$d/fzf.sock" \
    --ansi --header-lines 1 \
    --delimiter '\t' --with-nth 2 --id-nth 1 --track \
    --query '!spec/ ' \
    --layout reverse --info inline-right --prompt '  ' --pointer '▌' \
    --color 'prompt:magenta,pointer:magenta,hl:yellow,hl+:yellow,info:bright-black' \
    --preview "sh '$script' preview '$d' {1}" \
    --preview-window 'right,60%,<140(down,70%)' \
    --bind "enter:execute(sh '$script' open '$d' {1})+$reload" \
    --bind "ctrl-r:execute-silent(sh '$script' toggle-read {1})+$reload" \
    --bind "ctrl-h:$(flip read)" \
    --bind "ctrl-o:$(flip other)" \
    --bind "?:preview(sh '$script' help)" \
    --bind 'esc:ignore,ctrl-q:ignore,ctrl-g:ignore' || true
}

open_split() {
  root=$(git rev-parse --show-toplevel)
  pane=$("$herdr" pane split --pane "$HERDR_PANE_ID" --direction right --cwd "$root" "--${3:-focus}" | jq -r '.result.pane.pane_id')
  "$herdr" pane run "$pane" "$2; exit" >/dev/null
  echo "$pane"
}

review_open() {
  [ -f "$1/pane" ] && [ -S "$1/fzf.sock" ] && "$herdr" pane get "$(cat "$1/pane")" >/dev/null 2>&1
}

# herdr never reuses pane IDs, so a review whose Claude pane is gone can go.
prune_reviews() {
  for dir in "$state"/reviews/*; do
    [ -f "$dir/agent" ] || continue
    "$herdr" pane get "$(cat "$dir/agent")" >/dev/null 2>&1 || rm -rf "$dir"
  done
}

open_review() {
  prune_reviews
  pane=$(open_split review "sh '$script' list '$1'" "$2")
  echo "$pane" > "$1/pane"
  echo "$pane"
}

split() {
  d=$(review_dir "$HERDR_PANE_ID")
  base="${1:-$(default_base)}"
  [ $# -gt 0 ] && shift
  if review_open "$d"; then
    rm -f "$d/scope" "$d/baseline"
  else
    rm -rf "$d"
    mkdir -p "$d"
    echo "$HERDR_PANE_ID" > "$d/agent"
  fi
  echo "$base" > "$d/base"
  describe_base "$(resolve_base "$base")" > "$d/label"
  [ $# -gt 0 ] && printf '%s\n' "$@" > "$d/scope"
  if review_open "$d"; then
    reload_list "$d"
    cat "$d/pane"
  else
    open_review "$d" focus
  fi
}

# Only a session in a linked worktree (a task) opens a review on its own; one closed from herdr stays closed for that session.
hook() {
  [ "${CLAUDE_PLUGIN_OPTION_AUTO_OPEN:-false}" = true ] || return 0
  [ "${HERDR_ENV:-}" = 1 ] && [ -n "${HERDR_PANE_ID:-}" ] || return 0
  input=$(cat)
  cwd=$(echo "$input" | jq -r '.cwd // empty')
  [ -n "$cwd" ] && (cd "$cwd" 2>/dev/null && in_linked_worktree 2>/dev/null) || return 0
  session=$(echo "$input" | jq -r '.session_id // empty')
  d=$(review_dir "$HERDR_PANE_ID")
  [ -n "$session" ] && [ "$(cat "$d/session" 2>/dev/null)" = "$session" ] && return 0
  if review_open "$d"; then
    echo "$session" > "$d/session"
    return 0
  fi
  file=$(echo "$input" | jq -r '.tool_input.file_path // .tool_input.notebook_path // empty')
  dir=$(dirname "${file:-.}")
  [ -d "$dir" ] || dir=$(echo "$input" | jq -r '.cwd // "."')
  top=$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null) || return 0
  cd "$top" || return 0
  head=$(git rev-parse --verify HEAD 2>/dev/null) || return 0
  rm -rf "$d"
  mkdir -p "$d"
  echo "$session" > "$d/session"
  echo "$HERDR_PANE_ID" > "$d/agent"
  if worktree_base >/dev/null 2>&1; then
    echo fork > "$d/base"
  else
    echo "$head" > "$d/base"
    echo "since the work started ($(git rev-parse --short HEAD))" > "$d/label"
    changed_files "$head" | while IFS="$tab" read -r path _; do
      printf '%s\t%s\n' "$path" "$(fingerprint "$path")"
    done > "$d/baseline"
  fi
  open_review "$d" no-focus >/dev/null
}

command="${1:-}"
[ $# -gt 0 ] && shift
case "$command" in
  split) split "$@" ;;
  history) open_split history "tig" ;;
  hook) hook >/dev/null 2>&1 || true ;;
  list) list "$@" ;;
  rows) rows "$@" ;;
  watch) watch "$@" ;;
  preview) unified_diff "$(cat "$1/base")" "$2" always ;;
  diff) unified_diff "$1" "$2" never ;;
  open) open "$@" ;;
  mark-read) mark_read "$1" ;;
  toggle-read) toggle_read "$1" ;;
  help) help ;;
  *) echo "usage: review.sh split [base [path ...]] | review.sh history" >&2; exit 2 ;;
esac
