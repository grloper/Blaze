# Blaze 🔥

**The live logic runtime.** Ship a behavior change to a running production
service in microseconds — provably safe, canaried against live traffic, instantly
reversible. For humans, and for AI agents.

> **LaunchDarkly flips booleans. Blaze swaps _functions_** — in ~500 microseconds,
> with a proof, a canary, and an undo button. And it is how you let an AI touch
> production logic without fear.

Blaze is an embeddable, JIT-native scripting language whose headline feature is
*sound* live reload: change a `.blaze` file and the running process updates
instantly, **state preserved** — and the engine *proves* how the update may be
applied before applying it. A body-only edit hot-swaps one atomic pointer,
lock-free, under concurrent execution. An ABI change is detected, its full blast
radius recompiled, and the transition committed atomically so callers and callees
can never be observed with mismatched signatures. A broken save is proven bad and
refused, last-good still serving. A comment edit is proven to be nothing at all,
and costs nothing at all.

No restarts. No guessed diffs. No silent corruption. **Every "instant", "safe",
and "proven" below traces to a named test that attacks the guarantee from a
second thread while it holds.**

> 📽️ **Hero GIF placeholder.** The animated version of the demo below is recorded
> deterministically from a checked-in [`vhs`](https://github.com/charmbracelet/vhs)
> tape — [`docs/living_service.tape`](docs/living_service.tape) — with
> `vhs docs/living_service.tape`. Once produced, drop
> `![living service](docs/living_service.gif)` right here. Until then, the story
> renders just as well as text:

## See it: a living service under fire

[`examples/living_service`](blaze-jit/examples/living_service) is an HTTP
risk-scoring service whose scoring logic is a `.blaze` program, embedded through
`FuncHandle`s and hot-swapped while thousands of requests per second pour through
it. Its `--script` mode tells the whole story — over real HTTP, under live load,
every beat asserted:

```sh
git clone https://github.com/grloper/Blaze && cd Blaze
cargo run -p blaze-jit --release --example living_service            # watch mode: edit rules.blaze, live
cargo run -p blaze-jit --release --example living_service -- --script  # the six-beat story, CI-asserted
```

```text
━━━ Blaze living-service — scripted run under load ━━━
served 51,815 requests across 6 live edits, 0 dropped

  [1] body edit → SafeSwap        radius {velocity_risk}          in 496µs   dropped 0
  [2] broken save → Rejected      1 diagnostic; last-good serves on          dropped 0
  [3] runaway while(1) → canary   trapped FuelExhausted; auto-aborted        dropped 0
  [4] risky rule → canary         saw 309 divergences; promoted as SafeSwap  dropped 0
  [5] ABI change → Relink         radius {velocity_risk, score}   in 1.08ms  dropped 0
  [6] rollback to gen 1 → Relink  radius {..}                     in 1.28ms  dropped 0

steady-state scoring latency: p50 64ns, p99 512ns
all six guarantees asserted ✓
```

Six live edits to a service under fire — a retune, a typo, a runaway, a risky
change, an ABI change, and an undo — and **not one of 51,815 requests was
dropped or wrongly answered.** Each beat is a theorem with a test:
[`tests/living_service.rs`](blaze-jit/tests/living_service.rs) hammers the same
story from four threads.

And [`examples/agent_loop`](blaze-jit/examples/agent_loop.rs) runs that pipeline
for an *autonomous editor* — propose → gate → offline-eval → canary →
promote/abort → read metrics. Its centerpiece: a candidate that passes offline
eval but loops forever on an input the eval set never covered is caught by the
**canary on live traffic, before promotion.** That is the entire argument for
letting an AI touch production logic — the blast radius of a bad idea is a shadow
execution, not an outage.

```sh
cargo run -p blaze-jit --example agent_loop        # the agent reaches its target; every bad edit is stopped
```

There is also a purely visual demo — a terminal particle fountain whose physics
is a hot-swapped `.blaze` file, state surviving every reload:
`cargo run -p blaze-jit --example live` (then edit `blaze-jit/examples/live.blaze`).

## Why this doesn't exist elsewhere

Every hot-reload system today — game-engine script reloaders, `dlopen`
swappers, native patchers — is bolted onto a compiler that *doesn't know what
the edit changed*. They diff files, guess the blast radius, and hope the ABI
didn't move. When the guess is wrong, the process corrupts or crashes.

Blaze inverts the architecture. It is an **incremental compiler first**: source
is decomposed into a fine-grained [`salsa`] query graph in which every function's
lowered IR depends on its own text and on its callees' *signatures* — never
their bodies. That asymmetry (the **firewall**) means the graph computes the
exact invalidation blast radius of any edit as a byproduct of recompiling it:

```text
             raw source (input)
                    │
            function_text(f)            ← per-function firewall node
          ┌─────────┴─────────┐
          ▼                   ▼
  function_signature(f)   lowered_dev_ir(f) ──► compiled_machine_code(f)
          ▲                   │
          └── callee's sig ───┘   ← callers read the SIGNATURE, not the body
```

The live runtime then derives the *swap protocol* from the graph:

| The graph proves…                                | Classification | Commit protocol |
|--------------------------------------------------|----------------|-----------------|
| The program has a syntax error, an undefined callee, a call-site arity mismatch, or an undefined variable | `Rejected` | **nothing is compiled or patched — the previous, known-good generation keeps serving every call untouched.** The first load fails construction outright (there's no "previous" to hold) |
| No function's IR changed                         | `NoEffect`     | nothing compiled, nothing patched |
| Changed functions all kept their signatures      | `SafeSwap`     | one lock-free atomic pointer store — valid under concurrent execution, because the firewall guarantees no caller's code mentions anything about the callee except its slot |
| A signature changed (or a function was removed)  | `Relink`       | the graph *forces* every caller into the radius; the whole set recompiles and commits under a quiescence barrier — mismatched caller/callee ABIs are unobservable |

`StateMigration` is reserved: Blaze functions are pure over their scalar
arguments (`i64`/`f64`) and all persistent state lives in the host — which is
precisely *why* state survives every reload.

`Rejected` is not a fallback bolted on top — it runs from the *same* query
graph as everything else. A dedicated checker (`blaze_ir::diag`) re-walks each
function with the exact statement order and declare-before-use scoping the
lowerer itself uses, and reports every point where the lowerer would otherwise
silently substitute a default (an unresolved name, an unknown callee, a
mismatched argument count). Deleting a function another one still calls is not
a special case: it is simply "now an undefined callee," caught by the same
check, held open by the same protocol.

## How is this safe? Claims → tests

Every "instant" and every "safe" in this README traces to a query-graph
guarantee with a test hammering it from a second thread:

| Claim | Proven by |
|---|---|
| Body edit swaps live under concurrent calls, with zero missed calls and zero torn values | `live.rs::body_edit_swaps_under_concurrent_execution` — a thread calls `main()` in a tight loop through the swap; every result is old-correct or new-correct |
| The blast radius of a body edit is exactly the edited function | same test: `ReloadReport::changed == ["add"]`, and the salsa execution trace shows the caller was a memo hit |
| An ABI edit is classified `Relink` and transitions atomically | `live.rs::signature_edit_relinks_atomically_under_fire` — under the same hammering, observed values are only ever fully-old or fully-new |
| A comment edit is proven `NoEffect`: zero codegen, zero generations | `live.rs::comment_edit_is_proven_no_effect` |
| A syntax error is `Rejected`; last-good keeps answering every concurrent call | `live.rs::syntax_error_holds_last_good_under_concurrent_execution` — hammered the same way as the swap tests; every observed value is old-correct |
| A call to an undefined function never reaches a live process | `live.rs::undefined_callee_is_rejected_not_silently_tolerated` |
| A call-site arity mismatch is `Rejected`, not silently tolerated | `live.rs::arity_mismatch_is_rejected` |
| Deleting a function a caller still uses is `Rejected`, not silently zeroed | `live.rs::removing_a_used_function_is_rejected_and_holds_last_good` — both functions keep working exactly as before |
| A defective *first* program fails construction, not silent misbehavior | `live.rs::initial_load_with_a_defect_fails_construction` |
| Unbounded recursion aborts the *call*, never faults the host | `live.rs::unbounded_recursion_aborts_with_error_not_a_crash` — `spin` recurses forever; every call returns `Err(ResourceExhausted)` and interleaved healthy calls stay exact, under a concurrent bystander |
| The depth guard doesn't false-positive on real recursion | `live.rs::deep_but_bounded_recursion_succeeds` (100-deep succeeds, past-limit aborts) + `mutual_recursion_is_also_bounded` + `depth_limit_is_configurable` |
| An infinite loop aborts with `FuelExhausted`, never hangs | `live.rs::infinite_loop_aborts_with_fuel_exhausted` — `while(1){}` returns a defined error; the runtime is healthy afterward |
| A runaway can't permanently wedge the runtime | `live.rs::relink_commits_after_a_runaway_loop_traps` — a thread spins in a loop holding the dispatch lock; a concurrent `Relink` still commits once fuel runs out (before fuel, it would hang forever) |
| Fuel bounds shallow-but-explosive recursion the depth guard can't | `live.rs::exponential_recursion_is_bounded_by_fuel` — naive `fib(40)` (depth 40, ~3×10⁸ calls) is caught by fuel, not depth |
| Fuel doesn't false-positive real loops | `live.rs::legitimate_loops_run_under_fuel` — a 1000-iteration loop computes correctly under the default budget |
| A live-edited `x / 0` cannot fault the host | `jit.rs::division_is_guarded_and_cannot_fault_the_process` — division is guarded in codegen; `x/0 == 0`, `INT_MIN / -1 == INT_MIN` |
| The fast `FuncHandle` path is torn-free under concurrent hot-swaps | `live.rs::handle_calls_are_correct_under_concurrent_body_swap` — a thread hammers a function through a lock-free handle while it is body-swapped; every value is old- or new-correct |
| A handle survives a body swap yet detects an arity change | `live.rs::handle_survives_body_swap_transparently` + `handle_detects_arity_change` — a stale handle never dispatches a call with a mismatched argument count |
| `float` (f64) runs parse→lower→codegen→execute, exactly, and never faults | `floats.rs::float_arithmetic_executes_end_to_end` + `float_division_never_faults` — `x/0.0` is a defined ±inf/NaN, no guard needed |
| A float round-trips across call/return boundaries (the `i64↔f64` bit-cast ABI) | `floats.rs::float_args_and_returns_round_trip_across_calls` + `raw_and_typed_paths_agree_on_float_bits` |
| A float body edit hot-swaps live under a second thread, torn-free | `floats.rs::float_body_swap_is_sound_under_concurrent_execution` |
| Retyping a parameter `int`↔`float` is a `Relink`, atomic under a second thread | `floats.rs::retyping_a_parameter_relinks_atomically_under_fire` — every observation is fully-old float or fully-new int, decoded by the matching return type |
| A type mismatch is proven and `Rejected`, never a silent bit-reinterpretation | `blaze-ir` `diag::tests::mixed_arithmetic_is_rejected` (+ wrong-typed return, assignment, argument, and bare-float condition) |
| Per-function metrics are exact under concurrent callers, lock-free | `metrics.rs::counts_are_exact_under_concurrent_callers` — four threads hammer one slot; every increment lands |
| Every reload (incl. `Rejected`/`NoEffect`) is journaled with its class, radius, diagnostics, and latency | `journal.rs::every_event_is_journaled_in_order` + `rejected_events_are_journaled_but_not_committed` |
| `rollback(gen)` reverts through the same swap protocol, torn-free under a second thread | `journal.rs::rollback_reverts_a_body_edit` + `rollback_is_sound_under_concurrent_execution` (Relink variant: `rollback_across_an_abi_change_is_a_relink`) |
| A canary's candidate answer never reaches a caller — even under a storm of concurrent mirrored calls | `canary.rs::the_caller_never_sees_the_candidate` + `the_shield_holds_under_concurrent_traffic` |
| A wrong / faulting / too-slow candidate auto-aborts and cannot be promoted | `canary.rs::a_diverging_candidate_auto_aborts` + `a_faulting_candidate_auto_aborts` + `a_slow_candidate_auto_aborts_on_latency` + `an_aborted_candidate_cannot_be_promoted` |
| Promoting a validated candidate is the ordinary classified swap, seamless under load | `canary.rs::a_matching_candidate_promotes_through_the_swap_protocol` + `promote_is_seamless_under_concurrent_traffic` |
| A canary evaluates its candidate under the live program's *own* fuel/depth budget, so a runaway candidate traps promptly on production limits | `canary.rs::a_runaway_candidate_traps_under_the_primary_fuel_budget` |
| The canary sampler is lock-free and exact 1-in-N even under a storm of concurrent callers | `canary.rs::sampling_is_exact_one_in_n_under_concurrent_traffic` |
| **The whole living-service story** — six live edits (body swap, broken save, runaway, risky change, ABI change, rollback) under sustained load, each classified as claimed, every generation computing the exact reference scores, and **zero dropped calls** | `living_service.rs::the_living_service_story_holds_under_load` — four threads hammer `score` through all six beats |
| An autonomous agent's gate + offline-eval + canary pipeline keeps every bad edit out of production (the runaway is caught by the canary, never promoted; the objective is met with the fraud guardrail intact) | `agent_loop.rs::the_agent_pipeline_keeps_every_bad_edit_out_of_production` |
| The firewall itself | `blaze-ir/tests/incremental.rs` — body edits re-lower one function while callers are byte-for-byte memo hits (`Arc::ptr_eq`); ABI edits cascade |

Run everything:

```sh
cargo test --workspace                                       # 110 tests
cargo run -p blaze-jit --release --example living_service -- --script  # the living-service story, asserted
cargo run -p blaze-jit --example agent_loop                   # the AI-agent safety pipeline, asserted
cargo run -p blaze-jit --release --example bench_reload        # reload latency per edit class
cargo run -p blaze-jit --release --example bench_calls         # call throughput (named vs handle)
cargo run -p blaze-jit --release --example bench_vs_interpreter # Blaze vs an embedded interpreter (rhai)
cargo run -p blaze-jit --release --example bench_canary         # canary overhead on the live path
```

## Embedding

```rust
use blaze_jit::{LiveRuntime, ScriptHost};

// One-shot: compile a program and call it.
let rt = LiveRuntime::new("int add(int a, int b) { return a + b; }")?;
assert_eq!(rt.call("add", &[2, 3]), Ok(5));

// Hot path: resolve once, call millions of times, lock-free (~50M calls/s/thread).
// The handle survives body hot-swaps transparently and detects ABI changes.
let mut add = rt.handle("add")?;
assert_eq!(rt.call_handle(&mut add, &[2, 3]), Ok(5));

// Floats are first-class through a typed value API — pass an f64, get one back:
use blaze_jit::Value;
let fx = LiveRuntime::new("float score(float x) { return x * 2.5; }")?;
assert_eq!(fx.call_typed("score", &[Value::Float(2.0)]), Ok(Value::Float(5.0)));

// Native functions, callable from Blaze through the same swap table:
extern "C" fn now_ms() -> i64 { /* ... */ 0 }
unsafe { rt.register_host_fn("now_ms", 0, now_ms as *const u8) };

// Hot-swap an edit; the report tells you what the graph proved:
let report = rt.reload("int add(int a, int b) { return a + a + b; }")?;
assert_eq!(report.class, blaze_jit::EditClass::SafeSwap);
assert_eq!(report.changed, vec!["add".to_string()]);

// Observe (opt-in, lock-free) and revert. Rollback replays a past generation's
// source through the same classified, provably-safe swap protocol.
rt.set_metrics_enabled(true);
rt.call("add", &[2, 3])?;
let m = rt.metrics("add").unwrap();          // calls, total_nanos, faults
let _ = rt.rollback(1)?;                       // back to generation 1, classified
assert_eq!(rt.call("add", &[2, 3]), Ok(5));

// Canary a candidate against live traffic. `call_canary` returns the LIVE
// answer while shadowing the candidate; a wrong/slow one auto-aborts.
use blaze_jit::CanaryPolicy;
rt.canary("int add(int a, int b) { return b + a; }", CanaryPolicy::default())?;
let _ = rt.call_canary("add", &[2, 3])?;       // always the live result
if rt.canary_status().map(|s| s.verdict) == Some(blaze_jit::CanaryVerdict::Running) {
    rt.promote()?;                             // classified swap, journaled
}

// Or bind to a file and poll once per frame:
let mut host = ScriptHost::new("game/logic.blaze")?;
loop {
    if let Some(report) = host.poll()? {
        println!("reloaded: {report:?}");
    }
    host.runtime().call("update", &[/* dt */ 16])?;
    # break;
}
```

`call` is thread-safe and may race freely with `reload` — that interleaving is
exactly what the test suite hammers.

## Mechanics (one paragraph each)

**The swap table.** Every function gets a slot in an `mmap`'d array of atomic
pointers with a process-stable address. Every Blaze→Blaze call compiles to an
acquire-load of the callee's slot plus an indirect call — so functions have *no
relocations against each other*, generations of code never need relinking, and
"swap" is one release-store. Executable pages are managed by Cranelift's JIT
(the `PROT_WRITE` → `PROT_EXEC` transition and i-cache coherence); retired
generations stay mapped for the runtime's life because a concurrent caller may
still be inside them — the cost is old versions of *edited functions only*.

**The compiler.** `blaze-parse` lexes with [`logos`] and parses with a
hand-written recursive-descent parser into a lossless [`rowan`] CST. `blaze-ir`
lowers to DevIR — a tiny register IR with label-structured control flow —
inside the [`salsa`] graph. `blaze-jit` maps DevIR onto Cranelift (registers
become `cranelift-frontend` variables; the SSA builder reconstructs phis) and
emits through one shared pass with pluggable call emission (direct, relocatable,
or table-indirect).

**The language.** A deliberately small C-subset, JIT-compiled to native code:
`int` (i64) and `float` (IEEE-754 f64) functions, parameters, locals,
assignment, `+ - * /`, comparisons, `if / else if / else`, `while`, recursion,
calls, unary minus. The two scalar types are distinct and do **not** implicitly
convert: a mixed-type expression, a wrong-typed return or assignment, or a
bare-`float` condition is a type error the same query graph proves and
`Rejected`s *before* any code goes live — so the machine ABI (which carries
every value as a raw 64-bit word and bit-casts `i64↔f64` only at call/return
boundaries) can never reinterpret one type's bits as the other's at runtime.
Nothing a script can express — not a saved typo, not a divide-by-zero, not
runaway recursion, not an infinite loop — can fault *or hang* the process
embedding it: integer division is guarded (`x/0 == 0`; IEEE float division
simply yields ±inf/NaN, which never traps), every call site is validated against
a known callee with matching arity *and types* before a reload commits (see
`Rejected`), and every function threads a per-call context whose depth counter
aborts runaway recursion and whose fuel budget aborts runaway loops and
explosive recursion — all as typed errors, never a stack overflow or a hang.
Growing the surface (more types, richer state) is roadmap, not architecture: the
reload and safety guarantees don't change as the language grows.

## Benchmarks

All numbers below are single-run, release mode, **measured on the dev container
this repo was built in** — a modest, shared cloud box. They vary run to run; the
ratios and orders of magnitude are the point, and the *structural* claims (blast
radius bounds compilation, the fast path shares nothing) survive any hardware.
Every table has a `cargo run` you can reproduce.

**Reload latency** (`bench_reload` — a 40-function call chain where every
function transitively depends on the edited leaf; median of repeated edits):

| Event | Latency |
|---|---|
| Full cold compile (≈ what a restart pays in compilation alone) | 8.5 ms |
| Body edit → `SafeSwap` (radius 1 of 40) | **0.74 ms** |
| Comment → `NoEffect` (radius 0) | 0.54 ms |
| ABI edit → `Relink` (radius 2) | 1.9 ms |

Sub-millisecond from `reload()` to new native code answering calls — ~11× faster
than recompiling the program, on a 40-function toy — and a restart also forfeits
all state. Compilation cost scales with the **blast radius the graph proves**,
not with program size. (In the living-service demo, a `SafeSwap` under live HTTP
load committed in **496µs**.)

**Call throughput** (`bench_calls` — a trivial leaf, so this is dispatch cost,
not compute):

| Path | Throughput |
|---|---|
| `call(name)` (lock + string lookup per call) | ~19 M calls/s/thread |
| `call_handle` (resolve once, then lock-free) | **~50 M calls/s/thread** |
| `call_handle`, 4 threads | ~188 M calls/s (~94% of linear — nothing shared) |

The fast path is an arity check, one atomic load (double-checked against the
slot's arity so an ABI change is never mis-dispatched), and the indirect call —
~10× the "5 M/s/thread" bar a rules engine needs, and it scales near-linearly
because nothing on it is shared.

**vs. an embedded interpreter** (`bench_vs_interpreter` — the *same* branchy
risk rule, cross-checked to agree on every input, in Blaze's native `FuncHandle`
path vs. [`rhai`]'s compiled-AST `call_fn`):

| Engine | Throughput |
|---|---|
| Blaze `call_handle` (native JIT) | **~49 M calls/s/thread** |
| rhai `call_fn` (AST interpreter) | ~0.96 M calls/s/thread |

**~51× the interpreter** on identical logic — and still hot-swappable, which a
compiled `dylib` is not. That is the "swap functions, don't flip booleans" number.

**Canary overhead** (`bench_canary` — what `call_canary` costs the live path, on
a ~46 ns leaf so the deltas are visible; ns/call is the honest lens):

| Mode | Overhead |
|---|---|
| Idle (no candidate) | +~4 ns/call (one atomic load) |
| Shadowing 1% of calls | +~4 ns/call |
| Shadowing 100% of calls | +~220 ns/call (a full shadow every call) |

An idle canary is one atomic load. An active one adds a **lock-free** counter
increment per call (noise for any real handler) plus a shadow on the sampled
fraction — and because the sampler is lock-free, mirroring never serializes
concurrent traffic. The caller always gets the live answer regardless.

## Honest limits

Blaze is a focused core, not a general language. What it deliberately does *not*
do yet — so you know before you build on it:

- **One file, one flat namespace.** A program is a single `.blaze` source of
  top-level functions. No modules, no imports.
- **Two scalar types, `int` (i64) and `float` (f64).** No strings, arrays,
  structs, or pointers — which is also *why* a script can't corrupt or fault the
  host: there are no memory operations to abuse. Explicit `int`↔`float`
  conversions aren't in the language yet (a mixed-type expression is a proven
  type error, not a silent coercion).
- **State lives in the host, not the script.** Blaze functions are pure over
  their scalar arguments; all persistent state is the embedder's. This is exactly
  what makes state survive every reload — but "migrate a script-owned data
  structure across a schema change" is therefore not something Blaze does.
  `StateMigration` is a reserved `EditClass` for when script-owned state lands;
  today it cannot occur.
- **Retired code is retained, not freed.** A concurrent caller may still be
  inside an old generation mid-swap, so old code pages stay mapped for the
  runtime's life. The cost is bounded by (edit count × edited-function size) —
  negligible for a dev loop or a human/CI-paced rules service, but it does grow
  with the number of edits.
- **An active canary re-executes each sampled call**, so mirror only logic free
  of observable host side effects (the norm for scoring/decision functions), and
  it adds a lock-free counter to every `call_canary` while running — noise for a
  real handler, but real. A canary is an evaluation mode, not steady state.
- **Watch mode is mtime polling.** The file-watching demo re-reads on
  modification-time change (the `ScriptHost` mechanism); it is not an OS file-event
  subscription.

None of these change the reload and safety guarantees — growing the surface is
roadmap, not architecture. Where a milestone couldn't be made sound in time it
was cut and named here, rather than shipped hopeful.

## Status

- ✅ Incremental firewall (proved by tests since the first commit)
- ✅ Cranelift JIT backend, machine code memoized in the query graph
- ✅ Live-swap runtime: `Rejected` / `SafeSwap` / `Relink` / `NoEffect`
  classification, lock-free body swaps, quiesced ABI relinks, host functions,
  file watching
- ✅ **Diagnostics gate**: syntax errors, undefined callees, arity mismatches,
  and undefined variables are proven from the query graph and refused *before*
  touching a live process — a bad save holds the last-good generation open
  rather than hot-swapping mangled semantics in
- ✅ **Depth + fuel guards**: every function threads a per-call context
  (wasmtime's `vmctx` pattern). A call-depth counter aborts runaway recursion
  with `Err(ResourceExhausted)` before it can blow the native stack, and a
  fuel budget — one unit per call and per loop back-edge — aborts runaway
  loops and shallow-but-explosive recursion with `Err(FuelExhausted)`. A
  single bad edit can no longer fault *or wedge* the runtime: a runaway that
  would otherwise hold the dispatch lock forever now ends on its own, so a
  reload always commits
- ✅ **`FuncHandle` fast path**: resolve a function once, then call it
  lock-free at ~50 M calls/s/thread (scaling linearly across threads). Handles
  survive body hot-swaps transparently and detect ABI changes without ever
  dispatching a mismatched call
- ✅ **`float` (f64) + a sound type system**: `int` and `float` are distinct
  machine representations carried through one raw-64-bit ABI, bit-cast only at
  call/return boundaries. A type checker in the same query graph refuses every
  mismatch — mixed arithmetic, a wrong-typed return/assignment/argument, a
  bare-`float` condition — *before* a reload commits, so the coercion path is
  provably never observed at runtime. Retyping a parameter is an ABI change the
  firewall classifies as a `Relink`, atomic even under concurrent calls. Floats
  reach the host through a typed `Value` API (`call_typed` / `call_handle_typed`)
- ✅ Terminal live demo + scripted CI-safe mode + reload & call benchmarks
- ✅ **Per-function metrics**: opt-in, lock-free call/latency/fault counters
  indexed by a function's stable slot (so they survive hot swaps). Off by
  default — the call path pays one relaxed flag load — and exact under
  concurrent callers. Read them with `metrics(name)`
- ✅ **Reload journal + `rollback()`**: every reload event (including
  `Rejected` and `NoEffect`) is recorded with its class, blast radius,
  diagnostics, latency, and the exact source it installed. `rollback(gen)` is
  not a special path — it reinstalls that generation's source through the
  ordinary reload protocol, so a revert is itself classified, committed with
  the synchronization its class proves sound, and journaled as a new event
- ✅ **In-process canary**: `canary(source, policy)` compiles an isolated
  candidate and shadows it against the live program — a sampled fraction of
  calls (routed through `call_canary`) run through both and the results are
  compared, but the caller *always* gets the live answer, so a bad candidate
  can never reach a real request. A wrong, faulting, or too-slow candidate
  auto-aborts per policy and cannot be promoted; `promote()` reinstalls a
  healthy candidate through the ordinary classified swap (and journals it). The
  1-in-N sampler is lock-free, so an active canary never serializes live traffic
- ✅ **The demo — a living service under fire**: `examples/living_service` is an
  HTTP scoring service whose logic is a hot-swapped `.blaze` program, with a live
  TUI (req/s, p50/p99, generation timeline, last reload, canary divergence). Its
  `--script` mode runs the six-beat story — SafeSwap, Rejected, canary-caught
  runaway, canary + promote, Relink, rollback — over real HTTP under load, every
  beat asserted, zero dropped. `examples/agent_loop` runs the same
  gate → canary → promote/abort pipeline for an autonomous editor
- ✅ **Benchmark suite**: reload latency per class, call throughput (named vs
  handle), Blaze vs. an embedded interpreter (`rhai`, ~51×), and canary overhead
  — all reproducible and machine-labeled
- 🔜 Richer types and explicit `int`↔`float` conversions (pure language growth;
  reload semantics unchanged)
- 🔜 `StateMigration`: script-owned persistent state with layout versioning
- 🔜 Windowed demo (the terminal demos are engine-complete; a `winit`/`macroquad`
  frontend is presentation) and an editor status-line plugin. A `vhs` tape
  (`docs/living_service.tape`) records the README GIF deterministically

## Design notes

- **Modern salsa.** Blaze targets salsa 0.28 (`#[salsa::tracked]` functions
  over a `#[salsa::db]` trait) — the maintained API that rust-analyzer uses,
  not the long-removed `query_group` macros.
- **Function identity is interned, not hashed.** A function's id is salsa's
  interned id for its name — an injective map — so two distinct names can never
  collide onto one id and route a call to the wrong function. Proven over 5000
  names in `blaze-ir` (`function_ids_are_idempotent_and_collision_free`). Ids
  are consistent for the life of one runtime, the only scope they're compared
  in.
- **Generations are retired, not freed.** Deliberate: soundness under
  concurrent callers first; the leak is bounded by edit count × edited-function
  size, negligible for a dev loop.
- **Host-facing arity is part of your API.** `reload` keeps host `call`s safe
  (`ArityMismatch` errors rather than UB) if you change an entrypoint's
  signature while the host still passes the old argument count.

Apache-2.0.

[`salsa`]: https://github.com/salsa-rs/salsa
[`logos`]: https://github.com/maciejhirsz/logos
[`rowan`]: https://github.com/rust-analyzer/rowan
