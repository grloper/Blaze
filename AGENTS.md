# AGENTS.md

Instructions for AI coding agents and contributors.

## Verification gate (required before claiming anything works)

Run exactly this and read the output (bash, Rust 1.94.0 (rustup) with clippy + rustfmt):

```
scripts/verify.sh
```

Exit code 0 = pass; anything else = not done. CI (`.github/workflows/gate.yml`) runs the same script (job `verify`) plus `scripts/check-test-integrity.sh` (job `test-integrity`). What it runs: cargo fmt --check (non-blocking), cargo clippy --workspace --all-targets --locked -D warnings, cargo test --workspace --locked, actionlint

**Not checked by the gate:** release builds on other platforms, benchmarks, anything outside the workspace tests.

Rules: never delete tests, add skip/ignore/xfail markers, remove assertions or loosen expected values to get green. `test-integrity` also fails if the number of test cases drops below `scripts/test-baseline.txt`; only the owner may approve a test change (PR label `test-change-approved`; refresh the baseline with `scripts/check-test-integrity.sh --update-baseline`). Do not edit `.github/`, `scripts/verify.sh` or `scripts/check-test-integrity.sh` to make a failing check pass; they are owned by @grloper (CODEOWNERS).
