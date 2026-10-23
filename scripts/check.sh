#!/bin/sh
# What CI checks, run locally before a push. Usage: scripts/check.sh
set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"

step() { printf '\n== %s\n' "$*" >&2; }
fail() { printf '\ncheck: FAILED: %s\n' "$*" >&2; exit 1; }

[ -z "$(git status --porcelain)" ] || echo "check: testing uncommitted changes, not HEAD" >&2

step "cargo test"
KELPIE_STATE_DIR=$(mktemp -d) HERDR_BIN_PATH=false GIT_CONFIG_GLOBAL=/dev/null cargo test --locked -q -j 4 || fail "cargo test"
step "cargo clippy"
cargo clippy --locked --all-targets -- -D warnings || fail "cargo clippy"
step "cargo audit and cargo deny"
cargo audit || fail "cargo audit"
cargo deny check bans sources || fail "cargo deny"
oldest=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)
step "cargo +$oldest check (rust-version)"
cargo "+$oldest" check --locked --all-targets || fail "cargo +$oldest check"
printf '\ncheck: all green\n' >&2
