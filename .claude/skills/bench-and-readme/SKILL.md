---
name: bench-and-readme
description: Run Blaze's benchmark suite (reload latency, call throughput, interpreter comparison, canary overhead) and update every performance number in the README honestly and machine-labeled. Use when the user asks about performance, "how fast is it", re-measuring, updating benchmark or README numbers, preparing launch/marketing/website content, or after any change that could shift the hot path — even if they only ask for "the numbers".
---

# Benchmarks & honest numbers

The README's credibility rests on numbers being real, reproducible, and from one
machine. Mixed-machine figures are how launch pages start lying by accident — this
repo has already had to correct a faster host's numbers once. Ratios and orders of
magnitude are the durable story; absolutes are labeled snapshots.

## The suite (always `--release`)

`cargo run -p blaze-jit --release --example <name>`

| Bench | Measures | Reference (this repo's dev container, 2026-07) |
|---|---|---|
| `bench_reload` | reload-to-committed per edit class, 40-fn chain | SafeSwap ~0.74ms · NoEffect ~0.54ms · Relink ~1.9ms · cold ~8.5ms |
| `bench_calls` | dispatch cost: named vs `FuncHandle`, threaded | handle ~50M/s/thread · ~188M/s on 4 threads |
| `bench_vs_interpreter` | the same branchy rule vs rhai (cross-checked equal first) | ~49M vs ~0.96M calls/s → **~51×** |
| `bench_canary` | `call_canary` overhead idle / 1% / 100%, in ns/call | +~4ns idle and at 1% · +~220ns at 100% |

For "under live load" numbers, quote the release run of
`living_service -- --script`: SafeSwap in ~496µs mid-traffic, p50/p99 scoring
latency, and zero dropped across the six beats.

## Rules

- **One machine, one sweep.** Re-measuring anything re-measures everything; update
  every README number in the same commit and say which machine produced them.
- **Hunt stale figures before finishing.** Check the README's benchmark tables, the
  prose around them, the hero/demo transcript, the Status section, and doc-comments in
  `blaze-jit/src/live.rs` — prose drifts more than tables. A targeted
  `grep -n "calls/s\|ms\|µs\|×" README.md` plus a read-through catches what memory
  misses.
- **No cherry-picking.** One representative run, or the median where the bench prints
  one. If a number got worse, publish the worse number — then investigate why.
- **Ratios headline; absolutes carry their caveat.** "~51× an embedded interpreter"
  travels across machines; "48.84M calls/s" needs its machine label.
- **A new hot-path feature prices itself.** Show its cost ~zero when disabled (the
  metrics-flag pattern: one relaxed load) or price it openly in a table (the canary
  pattern). Nothing is "free" by assertion.
- `rhai` stays bench-only, under `[dev-dependencies]`.

## Recording the hero GIF

Pre-build so the tape captures the run, not the compile:

```sh
cargo build -p blaze-jit --release --example living_service
vhs docs/living_service.tape        # → docs/living_service.gif
```

The tape waits for the final `all six guarantees asserted` line, so its length adapts
to the machine. Then replace the README's placeholder blockquote with
`![living service](docs/living_service.gif)`.
