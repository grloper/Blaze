---
name: verify
description: Run Blaze's full green gate — workspace tests, clippy, and the asserted demo stories — and triage any failure by subsystem. Use before every commit, after any change under blaze-*/src, tests, or examples, whenever the user asks "is it green", "run the tests", "check everything", and at the end of any task that touched Rust code in this repo, even if no one explicitly asked for a test run.
---

# Verify: the Blaze green gate

"Green" in this repo means more than tests passing: the demos are executable proofs of
the product's claims, and clippy warnings are treated as errors. A claim whose test
fails is a lie in the README until fixed — which is why this gate runs before every
commit, not just before releases.

## The gate, in order

1. `cargo test --workspace` — expect **every suite green** (110 tests as of 2026-07;
   the count only grows).
2. `cargo clippy --workspace --all-targets` — expect **zero warnings**. `--all-targets`
   matters: examples and tests are part of the product here.
3. When runtime behavior changed (anything under `blaze-jit/src`), also run the
   asserted stories — they exercise cross-thread interleavings the unit suites don't:
   - `cargo run -p blaze-jit --release --example living_service -- --script`
     → must end `all six guarantees asserted ✓`, with `dropped 0` on every beat.
   - `cargo run -p blaze-jit --example agent_loop`
     → must end `…kept every bad edit out of production ✓`.

## Failure triage

| Failing suite | Where to look |
|---|---|
| `tests/live.rs` | swap/relink/gate/depth/fuel core — `live.rs`, `codegen.rs` |
| `tests/canary.rs` | shadow isolation, lock-free sampler, policy — `Canary`, `call_canary` |
| `tests/floats.rs` | type system, i64↔f64 bit-cast ABI |
| `tests/journal.rs` | journal + rollback re-classification |
| `tests/metrics.rs` | lock-free counters, slot stability across swaps |
| `tests/living_service.rs`, `tests/agent_loop.rs` | an end-to-end *guarantee* regressed — treat as a soundness bug, not demo flake |
| `blaze-ir` suites | firewall memoization (`Arc::ptr_eq` hits) and diagnostics |
| `abi.rs` unit test | `CallState` layout drifted from the `OFF_*` constants — memory-unsafe until reconciled |

## Rules

- Never weaken or delete an assertion to get green. Assertions encode the product's
  claims; if one genuinely must change, the README row it backs changes in the same
  commit, and the user is told.
- Timing assertions in tests are deliberately generous (debug builds under load). If
  one flakes, widen that ceiling rather than adding retries — and keep precise latency
  claims in the release benches only.
- A red story test after a "harmless" change usually means a lock or publish-ordering
  invariant broke. Re-read CLAUDE.md's "Invariants" before patching around symptoms.
