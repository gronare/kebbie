#!/bin/sh
# Run by a herdr tab_bar_right command entry every few seconds; prints nothing.

herdr="${HERDR_BIN_PATH:-$(command -v herdr)}"
[ -n "$herdr" ] || exit 0
state="${XDG_STATE_HOME:-$HOME/.local/state}/herdr-sync"
mkdir -p "$state"

panes=$("$herdr" pane list 2>/dev/null) || exit 0
tabs=$("$herdr" tab list 2>/dev/null) || exit 0

programs=$(echo "$panes" | jq -r '.result.panes[] | select(.agent != "claude") | .pane_id' |
  while read -r pane; do
    "$herdr" pane process-info --pane "$pane" 2>/dev/null | jq -r --arg pane "$pane" '.result.process_info as $i
      | $i.foreground_processes[0] as $f
      | select($f != null)
      | if $f.pid == $i.shell_pid then [$f.name, "idle"]
        elif ($f.argv | join(" ") | test("rails (c|console)")) then ["console", "busy"]
        else [$f.name, "busy"] end
      | [$pane, .[0], .[1]] | @tsv'
  done | jq -Rn '[inputs | split("\t") | { (.[0]): { name: .[1], idle: (.[2] == "idle") } }] | add // {}')

# The board's w key runs worktree.dev_server in a tab labelled server.
jq -rn --argjson panes "$panes" --argjson tabs "$tabs" --argjson programs "$programs" '
  [$tabs.result.tabs[] | select(.label == "server") | .tab_id] as $server
  | $panes.result.panes | group_by(.workspace_id)[]
  | [.[0].workspace_id, any(.[]; .tab_id as $tab | any($server[]; . == $tab) and ($programs[.pane_id] | . != null and (.idle | not)))] | @tsv' |
while IFS="$(printf '\t')" read -r ws server; do
  set -- --source sync --ttl-ms 15000
  if [ "$server" = true ]; then set -- "$@" --token server=server; else set -- "$@" --clear-token server; fi
  "$herdr" workspace report-metadata "$ws" "$@" >/dev/null 2>&1
done

# --agent claude keeps the task title from outliving Claude in that pane.
echo "$panes" | jq -r '.result.panes[] | select(.agent == "claude") | [.pane_id, .terminal_title_stripped, (.agent_session.value // "-")] | @tsv' |
while IFS="$(printf '\t')" read -r pane title session; do
  set -- --source sync --agent claude
  [ -n "$title" ] && set -- "$@" --display-agent "$title"
  transcript=""
  for file in "$HOME"/.claude/projects/*/"$session".jsonl; do
    [ -e "$file" ] && transcript=$file && break
  done
  if [ -n "$transcript" ]; then
    # Context in use: fresh input plus cache reads and writes of the last reply.
    usage=$(tail -n 300 "$transcript" | jq -r 'select(.type == "assistant" and .message.usage) | .message as $m
      | [($m.model | sub("^claude-"; "") | sub("-(?<a>[0-9]+)-(?<b>[0-9]+)$"; " \(.a).\(.b)")),
         ((($m.usage.input_tokens + $m.usage.cache_read_input_tokens + $m.usage.cache_creation_input_tokens) / 1000 | floor | tostring) + "k")]
      | @tsv' 2>/dev/null | tail -n 1)
    model=$(echo "$usage" | cut -f1)
    ctx=$(echo "$usage" | cut -f2)
    [ -n "$model" ] && set -- "$@" --token "model=$model" --token "ctx=$ctx"
  fi
  "$herdr" pane report-metadata "$pane" "$@" >/dev/null 2>&1
done

# Leaves labels you set alone: empty, numeric or the last name set here counts as unnamed; "-" clears.
auto_rename() {
  file="$state/$(echo "$1" | tr ':' '_')"
  case "$2" in
    - | *[!0-9]*) [ "$2" = - ] || [ "$2" = "$(cat "$file" 2>/dev/null)" ] || return 0 ;;
  esac
  [ "$2" = "$3" ] && return 0
  if [ "$3" = - ]; then
    "$herdr" "$4" rename "$1" --clear >/dev/null 2>&1 && rm -f "$file"
  else
    "$herdr" "$4" rename "$1" "$3" >/dev/null 2>&1 && echo "$3" > "$file"
  fi
}

# Pane frames carry no labels; this clears the ones earlier versions set, and leaves yours alone.
jq -rn --argjson panes "$panes" '
  $panes.result.panes[] | select(.label != null) | [.pane_id, .label, "-"] | @tsv' |
while IFS="$(printf '\t')" read -r pane label name; do
  auto_rename "$pane" "$label" "$name" pane
done

jq -rn --argjson panes "$panes" --argjson tabs "$tabs" --argjson programs "$programs" '
  [$tabs.result.tabs[] as $tab
   | [$panes.result.panes[] | select(.tab_id == $tab.tab_id)] | sort_by(.focused | not) as $p
   | [$p[] | $programs[.pane_id] | select(. != null and (.idle | not)) | .name] as $running
   | {
       tab: $tab.tab_id, ws: $tab.workspace_id, label: $tab.label,
       words: ([$p[] | select(.agent == "claude") | .terminal_title_stripped][0] | if . then ascii_downcase | split(" ") | map(select(. != "")) else null end),
       name: ($running[0] // ($tab.number | tostring))
     }]
  | group_by(.ws)[]
  | . as $space
  | .[]
  | (.words[0]?) as $first
  | (if .words == null then .name
     elif .words == [] then "✻ claude"
     elif ([$space[] | select(.words[0]? == $first)] | length) > 1 then "✻ " + (.words[0:2] | join(" "))
     else "✻ " + .words[0] end) as $name
  | [.tab, .label, (if ($name | length) > 28 then ($name | .[0:27]) + "…" else $name end)] | @tsv' |
while IFS="$(printf '\t')" read -r tab label name; do
  auto_rename "$tab" "$label" "$name" tab
done

live=$(jq -rn --argjson panes "$panes" --argjson tabs "$tabs" '$panes.result.panes[].pane_id, $tabs.result.tabs[].tab_id' | tr ':' '_')
for file in "$state"/*; do
  [ -e "$file" ] || continue
  echo "$live" | grep -qx "$(basename "$file")" || rm -f "$file"
done
