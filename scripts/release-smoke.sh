#!/bin/sh
# Before a release, run real agents through kebbie: scripts/release-smoke.sh [sandbox repo], ~/projects/kebbie-sandbox by default.
set -u

usage() {
  cat <<'EOF'
usage: scripts/release-smoke.sh [sandbox repo]

Starts one real Claude task, and one real Codex task when kebbie's Codex plugin
is installed and a profile runs Codex, in a sandbox repo (default
~/projects/kebbie-sandbox). Each adds a line to SMOKE.md and commits it. The
script waits for each to report ready, restarts it and waits for it to resume,
then drops both tasks. A task that waits on you, as an agent asking whether to
trust the repo's folder, fails at once with what it waits for. It never pushes
and touches no other repo.

The repo must be a git checkout with nothing uncommitted.

  SMOKE_TIMEOUT  seconds to wait for each report (default 900)
  SMOKE_POLL     seconds between looks at the board (default 10)
EOF
}

case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  -*) usage >&2; exit 2 ;;
esac
[ $# -le 1 ] || { usage >&2; exit 2; }

repo=${1:-$HOME/projects/kebbie-sandbox}
timeout=${SMOKE_TIMEOUT:-900}
poll=${SMOKE_POLL:-10}
kebbie=${KEBBIE:-kebbie}
herdr=${HERDR_BIN_PATH:-herdr}

refuse() { echo "release-smoke: $*" >&2; exit 2; }

command -v "$kebbie" >/dev/null 2>&1 || refuse "kebbie is not on PATH"
command -v jq >/dev/null 2>&1 || refuse "jq is not on PATH"
[ -d "$repo" ] || refuse "$repo is not there; make a sandbox repo or name one"
top=$(git -C "$repo" rev-parse --show-toplevel 2>/dev/null) || refuse "$repo is not a git repo"
repo=$(CDPATH='' cd -- "$repo" && pwd -P)
[ "$(CDPATH='' cd -- "$top" && pwd -P)" = "$repo" ] || refuse "$repo is inside $top, not a repo of its own"
[ -z "$(git -C "$repo" status --porcelain)" ] || refuse "$repo has uncommitted changes; commit or stash them first"

stamp=$(date +%Y%m%d%H%M%S)
place=$(basename "$repo")
started=""
results=""
failed=0

record() {
  results="$results$1	$2	$3
"
  [ "$2" = PASS ] || failed=1
}

cleanup() {
  left=0
  for task in $started; do
    "$kebbie" -C "$repo" drop "$task" --force >/dev/null 2>&1 || { left=1; echo "release-smoke: could not drop $task; kebbie -C $repo drop $task --force" >&2; }
  done
  started=""
  return "$left"
}

trap cleanup EXIT
trap 'cleanup; echo "release-smoke: stopped" >&2; exit 130' INT TERM HUP

item() {
  "$kebbie" status --json 2>/dev/null | jq -c --arg name "$1" --arg place "$place" '[.items[] | select(.name == $name and .place == $place)][0] // empty'
}

# 0 on the ready report, 2 with what it waits for in $waits once it sits under NEEDS YOU, 1 at the timeout.
wait_ready() {
  task=$1 word=$2 deadline=$(($(date +%s) + timeout))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    shown=$(item "$task")
    if [ -n "$shown" ] && [ "$(printf '%s' "$shown" | jq -r '.report.state // empty')" = ready ] && printf '%s' "$shown" | jq -r '.report.summary // empty' | grep -qi "$word"; then
      return 0
    fi
    if [ -n "$shown" ] && [ "$(printf '%s' "$shown" | jq -r '.group // empty')" = "NEEDS YOU" ]; then
      waits=$(printf '%s' "$shown" | jq -r '.next_for_you // .summary // "nothing it names"')
      return 2
    fi
    sleep "$poll"
  done
  return 1
}

# 0 once the task runs no turn and its rules check neither runs nor waits on a fix, 1 at the timeout: a Space closed mid-turn takes the turn's last answer with it.
wait_idle() {
  task=$1 deadline=$(($(date +%s) + timeout))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    shown=$(item "$task")
    if [ -n "$shown" ] && printf '%s' "$shown" | jq -e '(.agent.status // "") != "working" and ((.git.rules // "") | IN("checking", "breaks") | not)' >/dev/null; then
      return 0
    fi
    sleep "$poll"
  done
  return 1
}

said_in_full() {
  printf 'release-smoke: %s %s said:\n%s\n\n' "$label" "$1" "$said" >&2
}

waited() {
  case $1 in
    2) record "$label" FAIL "waits on you: $waits" ;;
    *) record "$label" FAIL "$2" ;;
  esac
}

prompt="This is kebbie's release smoke test. Append the line 'kebbie release smoke $stamp' to SMOKE.md, commit it with a subject and a body that says why, then report ready with the summary: smoke done. Do nothing else."
again="kebbie restarted you for its release smoke test. Report ready again with the summary: resumed. Do nothing else."

smoke() {
  label=$1 task=$2
  shift 2
  if ! said=$("$kebbie" -C "$repo" start "$task" "$prompt" --no-focus --no-note --recipe "small-fix: kebbie's release smoke test" "$@" 2>&1); then
    said_in_full start
    record "$label" FAIL "start: $(printf '%s' "$said" | head -n 1)"
    return
  fi
  started="$started $task"
  record "$label" PASS "started $task"
  wait_ready "$task" "smoke"
  got=$?
  if [ "$got" -ne 0 ]; then
    waited "$got" "no ready report within ${timeout}s"
    return
  fi
  if ! wait_idle "$task"; then
    record "$label" FAIL "still working or in its rules check after ${timeout}s"
    return
  fi
  ahead=$(item "$task" | jq -r '.git.ahead // 0')
  if [ "$ahead" -ge 1 ]; then record "$label" PASS "reported ready with $ahead commit"; else record "$label" FAIL "reported ready without a commit"; fi
  workspace=$(item "$task" | jq -r '.workspace // empty')
  if [ -z "$workspace" ] || ! "$herdr" workspace close "$workspace" >/dev/null 2>&1; then
    record "$label" FAIL "could not close its Space to restart it"
    return
  fi
  if ! said=$("$kebbie" -C "$repo" start "$task" "$again" --no-focus --no-note "$@" 2>&1) || ! printf '%s' "$said" | grep -q "It resumes its last conversation"; then
    said_in_full restart
    record "$label" FAIL "restart did not resume: $(printf '%s' "$said" | tr '\n' ' ' | cut -c1-120)"
    return
  fi
  wait_ready "$task" "resumed"
  got=$?
  if [ "$got" -eq 0 ]; then record "$label" PASS "restarted and resumed its conversation"; else waited "$got" "no report after the restart within ${timeout}s"; fi
}

smoke claude "smoke-claude-$stamp"

if "$kebbie" doctor 2>/dev/null | grep -Eq '^ok +codex plugin'; then
  coder=$("$kebbie" profiles --json 2>/dev/null | jq -r '[.[] | select(.agent == "codex" and (.permission_mode // "") != "plan" and (.kind // "task") == "task") | .name][0] // empty')
  if [ -n "$coder" ]; then
    smoke codex "smoke-codex-$stamp" --profile "$coder"
  else
    record codex PASS "skipped: no profile runs Codex"
  fi
else
  record codex PASS "skipped: kebbie's Codex plugin is not installed"
fi

if cleanup; then record cleanup PASS "dropped the smoke tasks"; else record cleanup FAIL "a smoke task is left; drop it by hand"; fi

printf '\n%-8s %-5s %s\n' agent result what
printf '%s' "$results" | while IFS='	' read -r agent result what; do
  printf '%-8s %-5s %s\n' "$agent" "$result" "$what"
done
if [ "$failed" -ne 0 ]; then
  printf '\nrelease-smoke: FAILED\n' >&2
  exit 1
fi
printf '\nrelease-smoke: all green\n' >&2
