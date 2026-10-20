#!/bin/bash
# Records the README film (demo) and the feature clips (demo/clips/*.tape), each in a fresh throwaway world: record.sh [demo | clip name]...
set -euo pipefail
cd "$(dirname "$0")/.."
export KDEMO_DIR=${KDEMO_DIR:-$(cd /tmp && pwd -P)/kelpie-demo}
trap 'demo/setup.sh --stop' EXIT
names=("$@")
if [ ${#names[@]} -eq 0 ]; then
  names=(demo)
  for tape in demo/clips/*.tape; do names+=("$(basename "$tape" .tape)"); done
fi
mkdir -p docs/assets/clips
for name in "${names[@]}"; do
  tape=demo/clips/$name.tape
  [ "$name" = demo ] && tape=demo/demo.tape
  [ -f "$tape" ] || { echo "no tape for $name: demo or a name from demo/clips" >&2; exit 1; }
  demo/setup.sh "$name"
  vhs "$tape"
done
ls -lh docs/assets/demo.gif docs/assets/*.png docs/assets/clips/*.gif 2>/dev/null || true
