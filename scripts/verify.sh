#!/usr/bin/env bash
# Verification gate: Blaze (Rust workspace: blaze-ir, blaze-jit, blaze-parse): rustfmt (non-blocking), clippy -D warnings, cargo test
#
# NOT CHECKED (do not claim these from a green run): release builds on other platforms, benchmarks, anything outside the workspace tests.
#
# Usage: scripts/verify.sh   (exit 0 = pass). Non-blocking steps print "WARN" and never fail the run.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }
need() { command -v "$1" >/dev/null 2>&1 || { echo "verify: required tool '$1' not found" >&2; exit 2; }; }
# warn_step "name" cmd...: run a check that fails today for pre-existing reasons; report but do not block.
warn_step() { local n="$1"; shift; step "$n (non-blocking)"; if ! "$@"; then echo "WARN: '$n' failed (pre-existing, non-blocking)"; fi; }
need git; need bash

step "shell syntax"
for f in scripts/*.sh; do bash -n "$f"; done

step "workflow lint (actionlint)"
if command -v actionlint >/dev/null 2>&1; then actionlint
elif command -v pipx >/dev/null 2>&1; then pipx run --spec actionlint-py==1.7.12.25 actionlint
else echo "verify: actionlint (or pipx) required" >&2; exit 2; fi

need cargo
step "rustfmt"
warn_step "rustfmt" cargo fmt --all -- --check
step "clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests"
cargo test --workspace --locked

printf '\nPASS: clippy + cargo test + actionlint + shell syntax.\n'
printf 'NOT CHECKED: release builds on other platforms, benchmarks, anything outside the workspace tests.\n'
