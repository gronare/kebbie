#!/bin/sh
# Usage: review.sh split [base [path ...]] | history | hook

set -eu

script="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
herdr="${HERDR_BIN_PATH:-herdr}"
state="${KEBBIE_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/kebbie}"

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
  if [ "$1" = "$(worktree_base 2>/dev/null)" ]; then echo "since $(default_branch)"; else echo "against $1"; fi
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

review_dir() {
  echo "$state/reviews/$(echo "$1" | tr ':/' '__')"
}

kebbie_bin() {
  if [ -n "${KEBBIE_BIN:-}" ]; then
    echo "$KEBBIE_BIN"
  elif [ -x "$(dirname "$script")/../target/release/kebbie" ]; then
    echo "$(dirname "$script")/../target/release/kebbie"
  else
    echo kebbie
  fi
}

# What the hook's baseline needs: every change since the base, untracked files too, and each file's contents.
changed_files() {
  git diff --name-only "$1"
  git ls-files --others --exclude-standard
}

fingerprint() {
  if [ -e "$1" ]; then git hash-object -- "$1"; else echo deleted; fi
}

open_split() {
  root=$(git rev-parse --show-toplevel)
  pane=$("$herdr" pane split --pane "$HERDR_PANE_ID" --direction right --cwd "$root" "--${3:-focus}" | jq -r '.result.pane.pane_id')
  "$herdr" pane run "$pane" "$2; exit" >/dev/null
  echo "$pane"
}

review_open() {
  [ -f "$1/live" ] && kill -0 "$(cat "$1/live")" 2>/dev/null
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
  pane=$(open_split review "'$(kebbie_bin)' review --view '$1'" "$2")
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
    changed_files "$head" | while read -r path; do
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
  *) echo "usage: review.sh split [base [path ...]] | review.sh history" >&2; exit 2 ;;
esac
