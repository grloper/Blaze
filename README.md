# Blaze 🔥

**The first hot-reload engine that is correct by construction.**

Blaze is an embeddable, JIT-native scripting language whose headline feature is
*sound* live reload: save a `.blaze` file and the running process updates
instantly, **state preserved** — and the engine *proves* how the update may be
applied before applying it. A body-only edit hot-swaps one atomic pointer,
lock-free, under concurrent execution. An ABI change is detected, its full
blast radius recompiled, and the transition committed atomically so callers and
callees can never be observed with mismatched signatures. A comment edit is
proven to be nothing at all, and costs nothing at all.

No restarts. No guessed diffs. No silent corruption. **Reload is a theorem of
the invalidation graph, not a trick.**

```sh
git clone https://github.com/grloper/Blaze && cd Blaze
cargo run -p blaze-jit --example live        # then edit blaze-jit/examples/live.blaze and save
```

```text
  Blaze live fountain — tick 214    (edit the .blaze file; Ctrl-C quits)
  [gen 3] Relink    radius {gravity, step_vy} in 6.9ms — state preserved
+------------------------------------------------------------------------+
|                       *    *   *                                       |
|                  *   *  * *  *    *                                    |
|               *    *   * ** * *  *    *                                |
|                 *    * * ** * * *   *                                  |
|                    *  * ***  * *  *                                    |
|                        * * * *                                         |
+------------------------------------------------------------------------+
```

Every particle's position survives the reload. Only the physics changed.

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

`StateMigration` is reserved: Blaze functions are pure over `i64`s and all
persistent state lives in the host — which is precisely *why* state survives
every reload.

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
| The firewall itself | `blaze-ir/tests/incremental.rs` — body edits re-lower one function while callers are byte-for-byte memo hits (`Arc::ptr_eq`); ABI edits cascade |

Run everything:

```sh
cargo test --workspace                                  # 57 tests
cargo run -p blaze-jit --example live -- --script        # scripted demo of all 3 apply-classes
cargo run -p blaze-jit --release --example bench_reload  # reload latency per edit class
cargo run -p blaze-jit --release --example bench_calls   # call throughput (named vs handle)
```

## Embedding

```rust
use blaze_jit::{LiveRuntime, ScriptHost};

// One-shot: compile a program and call it.
let rt = LiveRuntime::new("int add(int a, int b) { return a + b; }")?;
assert_eq!(rt.call("add", &[2, 3]), Ok(5));

// Hot path: resolve once, call millions of times, lock-free (~95M calls/s/thread).
// The handle survives body hot-swaps transparently and detects ABI changes.
let mut add = rt.handle("add")?;
assert_eq!(rt.call_handle(&mut add, &[2, 3]), Ok(5));

// Native functions, callable from Blaze through the same swap table:
extern "C" fn now_ms() -> i64 { /* ... */ 0 }
unsafe { rt.register_host_fn("now_ms", 0, now_ms as *const u8) };

// Hot-swap an edit; the report tells you what the graph proved:
let report = rt.reload("int add(int a, int b) { return a + a + b; }")?;
assert_eq!(report.class, blaze_jit::EditClass::SafeSwap);
assert_eq!(report.changed, vec!["add".to_string()]);

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
`int` (i64) functions, parameters, locals, assignment, `+ - * /`, comparisons,
`if / else if / else`, `while`, recursion, calls, unary minus. Nothing a script
can express — not a saved typo, not a divide-by-zero, not runaway recursion, not
an infinite loop — can fault *or hang* the process embedding it: division is
guarded (`x/0 == 0`), every call site is validated against a known callee with
matching arity before a reload commits (see `Rejected`), and every function
threads a per-call context whose depth counter aborts runaway recursion and
whose fuel budget aborts runaway loops and explosive recursion — all as typed
errors, never a stack overflow or a hang. Growing the surface (floats, more
types, richer state) is roadmap, not architecture: the reload and safety
guarantees don't change as the
language grows.

## Benchmarks

`cargo run -p blaze-jit --release --example bench_reload` — a 40-function
call-chain program (every function transitively depends on the edited leaf),
median latencies, one warm process, measured on the dev container this repo
was built in:

| Event | Latency |
|---|---|
| Full cold compile (≈ what a restart pays in compilation alone) | 3.88 ms |
| Body edit → `SafeSwap` (radius 1 of 40) | **0.50 ms** |
| Comment → `NoEffect` (radius 0) | 0.40 ms |
| ABI edit → `Relink` (radius 2) | 0.65 ms |

Sub-millisecond from `reload()` to new native code answering calls — 7.8×
faster than recompiling the program, on a 40-function toy. The structural
point survives any hardware and grows with program size: the file is parsed
once per edit and *compilation* cost scales with the **blast radius the graph
proves**, not with how big the program is — and a restart also forfeits all
state.

`cargo run -p blaze-jit --release --example bench_calls` — call throughput for a
trivial leaf function (dispatch cost, not compute):

| Path | Throughput |
|---|---|
| `call(name)` (lock + string lookup per call) | ~28 M calls/s/thread |
| `call_handle` (resolve once, then lock-free) | **~95 M calls/s/thread** |
| `call_handle`, 4 threads | ~400 M calls/s (linear — no shared lock) |

The fast path is an arity check, one atomic load (double-checked against the
slot's arity so an ABI change is never mis-dispatched), and the indirect call —
~19× the "5 M/s/thread" bar a rules engine needs, and it scales linearly
because nothing on it is shared.

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
  lock-free at ~95 M calls/s/thread (scaling linearly across threads). Handles
  survive body hot-swaps transparently and detect ABI changes without ever
  dispatching a mismatched call
- ✅ Terminal live demo + scripted CI-safe mode + reload & call benchmarks
- 🔜 In-process canary: mirror live traffic through a candidate generation
  before promoting it, with an auto-abort policy
- 🔜 Generation journal + `rollback()`: every reload's source, class, and
  blast radius persisted; reverting is just another classified, provably-safe
  swap
- 🔜 `f64` and richer types (pure language growth; reload semantics unchanged)
- 🔜 `StateMigration`: script-owned persistent state with layout versioning
- 🔜 Windowed demo (the terminal demo is engine-complete; a `winit`/`macroquad`
  frontend is presentation), editor status-line plugin, GIF for this README

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
