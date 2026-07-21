---
name: add-guarantee
description: Add a new runtime guarantee, safety property, or feature to Blaze the theorem-with-test way — attacking concurrent test first, then implementation, then the README claims table. Use whenever the user asks to add, harden, or make safe any runtime behavior (limits, isolation, EditClass semantics, metrics, canary policy, rollback), or uses words like "guarantee", "prove", "make sure X can never happen" — even if they don't mention tests.
---

# Add a guarantee (claims are theorems)

Blaze's entire pitch is that its safety words are backed by named tests a second thread
tries to falsify. So a guarantee isn't done when the code works — it's done when
(1) a test attacks it under concurrency and would fail on the pre-change code,
(2) the implementation makes that test pass, and (3) README's "Claims → tests" table
names the pair.

## Workflow

1. **State the claim as one falsifiable sentence**, in the register the README table
   uses ("A wrong candidate auto-aborts and cannot be promoted"). If it can't be
   phrased that way, it isn't a guarantee yet — it's a feature idea; sharpen it first.
2. **Write the attacking test first**, in the matching suite (see the triage map in
   the `verify` skill). Name it sentence-style, as the claim itself:
   `a_runaway_candidate_traps_under_the_primary_fuel_budget`. Confirm it fails (or
   reason precisely why it would) without the change.
3. **Implement minimally**, respecting CLAUDE.md's invariants — publish ordering, lock
   discipline, the firewall rule. If the implementation wants to bend one of those,
   stop and reconsider the design instead.
4. **Add the concurrency dimension** if the claim has an "…even under concurrent
   callers" reading — most do. A single-threaded pass is not proof here.
5. **Add the README row** in the same commit: `| <claim> | <file>::<test_name> |`.
6. **If it can't be made sound in time, cut it** and record it in README's
   "Honest limits". That is a respected outcome in this repo; shipping hopeful is not.

## The hammer pattern (house style)

Modeled on `the_shield_holds_under_concurrent_traffic` and
`body_edit_swaps_under_concurrent_execution`:

```rust
let rt = Arc::new(LiveRuntime::new(SRC).expect("compile"));
let start = Arc::new(AtomicBool::new(false));            // gate: release all threads at once
let workers: Vec<_> = (0..4).map(|_| {
    let (rt, start) = (rt.clone(), start.clone());
    thread::spawn(move || {
        while !start.load(Ordering::Relaxed) { std::hint::spin_loop(); }
        for _ in 0..N {
            // Assert on EVERY observation: old-correct or new-correct, never torn,
            // never an unexpected Err. A worker panic IS the failed guarantee.
        }
    })
}).collect();
start.store(true, Ordering::Relaxed);
// …perturb from the main thread MID-hammer: reload / canary / rollback…
for w in workers { w.join().expect("no worker may panic"); }
```

Habits that make these tests strong:

- Assert every observation, not a sample — "old xor new value" beats "didn't crash".
- Perturb mid-flight from the main thread; never pre-arrange a quiet moment.
- For whole-run invariants, count with lock-free atomics during the run (the
  living-service story pins "zero dropped" via `Stats.errors` this way).
- Latency ceilings stay generous (`COMMITTED_PROMPTLY`-style, 100ms) because CI is a
  debug build under load; exact numbers belong to the release benches.
- Runaway-code tests set a tight fuel budget first (`set_fuel_budget(200_000)`) so the
  trap fires in milliseconds and the test can never hang.

## Where things live

- Core swap/limits: `blaze-jit/tests/live.rs` · canary: `tests/canary.rs` ·
  journal/rollback: `tests/journal.rs` · metrics: `tests/metrics.rs` ·
  types/floats: `tests/floats.rs` · end-to-end stories: `tests/living_service.rs`,
  `tests/agent_loop.rs` · firewall + diagnostics: `blaze-ir/tests/incremental.rs` and
  `blaze-ir/src/diag.rs` unit tests.
- If the guarantee is visible in the product story, add a beat or assertion to
  `examples/living_service/service.rs::assert_story` too — the demo is itself a test.
