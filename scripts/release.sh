#!/bin/sh
# Releases kelpie: scripts/release.sh <X.Y.Z> "<what it brings>" moves CHANGES.md's Unreleased entries under X.Y.Z, bumps the manifests, tags, pushes and waits until the binaries download.
set -eu

version=${1:-}
summary=${2:-}
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "usage: scripts/release.sh <X.Y.Z> \"<what it brings>\"" >&2; exit 2 ;;
esac
[ -n "$summary" ] || { echo "say what the release brings: scripts/release.sh $version \"<what it brings>\"" >&2; exit 2; }
tag="v$version"
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

fail() { echo "release: $*" >&2; exit 1; }

[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || fail "check out main first"
[ -z "$(git status --porcelain)" ] || fail "commit or stash your changes first"
git fetch --quiet --tags origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || fail "main is not origin/main; pull or push first"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "$tag exists already"

head=$(git rev-parse HEAD)
ci=$(gh run list --commit "$head" --workflow ci --json status,conclusion -q '.[0] | "\(.status) \(.conclusion)"' 2>/dev/null || true)
[ "$ci" = "completed success" ] || fail "CI on $(git rev-parse --short HEAD) is '${ci:-not found}'; release a commit whose CI is green"

sh scripts/release-notes.sh --move "$version" || fail "write what $version brings in CHANGES.md first"
perl -pi -e 's/^version = "[^"]*"/version = "'"$version"'"/ if $. <= 10; close ARGV if eof' Cargo.toml herdr-plugin.toml
perl -pi -e 's/"version": "[^"]*"/"version": "'"$version"'"/' .claude-plugin/plugin.json
for manifest in Cargo.toml herdr-plugin.toml; do grep -q "^version = \"$version\"" "$manifest" || fail "$manifest still names another version"; done
grep -q "\"version\": \"$version\"" .claude-plugin/plugin.json || fail ".claude-plugin/plugin.json still names another version"
cargo check --quiet
git add CHANGES.md Cargo.toml Cargo.lock herdr-plugin.toml .claude-plugin/plugin.json
git commit --quiet -m "kelpie $version: $summary"
git tag -a "$tag" -m "kelpie $version"
git push --atomic --quiet origin main "$tag"
echo "release: pushed $tag; waiting for the release workflow"

run=""
for _ in $(seq 1 30); do
  run=$(gh run list --workflow release --branch "$tag" --json databaseId -q '.[0].databaseId' 2>/dev/null || true)
  [ -n "$run" ] && break
  sleep 5
done
[ -n "$run" ] || fail "the release workflow did not start for $tag"
gh run watch "$run" --exit-status >/dev/null || fail "the release workflow failed: $(gh run view "$run" --json url -q .url)"

slug=$(git remote get-url origin | sed -E 's#^(git@github.com:|https://github.com/)##; s#\.git$##')
for target in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu; do
  url="https://github.com/$slug/releases/download/$tag/kelpie-$version-$target.tar.gz"
  code=$(curl -sL -r 0-0 -o /dev/null -w '%{http_code}' "$url?check=$$" || true)
  case "$code" in 2*) ;; *) fail "kelpie-$version-$target.tar.gz answers $code" ;; esac
done
echo "release: kelpie $version is published: https://github.com/$slug/releases/tag/$tag"
