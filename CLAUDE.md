# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Blaze is **the live logic runtime**: an embeddable, JIT-native scripting language whose
core feature is *provably sound* hot reload — swap functions in a running process in
microseconds, refuse bad edits before they touch live pointers, canary candidates
against live traffic, roll back in one call. The discipline that holds the repo
together: **every claim is a theorem with a test that attacks it from a second thread.**
If you change what Blaze guarantees, you change a test. If something can't be made
sound, cut it and say so in README's "Honest limits" — never ship it hopeful.

## Commands

```sh
cargo test --workspace                       # full suite (110 tests) — green at every commit
cargo clippy --workspace --all-targets       # zero warnings at every commit (examples + tests count)
cargo test -p blaze-jit --test live          # one suite: live|canary|floats|jit|journal|metrics|living_service|agent_loop
cargo test -p blaze-jit --test live -- body_edit          # single test by name substring
cargo run -p blaze-jit --release --example living_service -- --script   # asserted 6-beat demo over real HTTP
cargo run -p blaze-jit --example agent_loop                             # asserted agent-safety pipeline
cargo run -p blaze-jit --release --example bench_reload     # also: bench_calls, bench_vs_interpreter, bench_canary
vhs docs/living_service.tape                 # record the README GIF (needs charmbracelet/vhs)
```

## Architecture (the big picture)

Three crates, one idea: **reload behavior is derived from an incremental-compilation
proof, never guessed.**

- `blaze-parse` — logos lexer → hand-written recursive-descent parser → lossless rowan
  CST, with typed AST accessors (`ast.rs`). Round-trips source byte-for-byte; parse
  errors become error nodes, never panics.
- `blaze-ir` — lowers CST → DevIR (a tiny register IR) inside a salsa 0.28 query graph
  (`lower.rs`, `db.rs`). **The firewall**: `lowered_dev_ir(f)` depends on f's own text
  and its callees' *signatures* — never their bodies. That asymmetry is what makes an
  edit's blast radius a computable fact rather than a guess. `diag.rs` is the reload
  gate: it re-walks each function with the lowerer's exact statement order and scoping
  and reports every defect the lowerer would otherwise paper over.
- `blaze-jit` — DevIR → Cranelift (`codegen.rs`) and the live runtime (`live.rs`):
  - **SwapTable** — an mmap'd array of atomic code pointers at process-stable
    addresses. Every Blaze→Blaze call compiles to an acquire-load of the callee's slot
    plus an indirect call, so generations never relink and a body swap is one
    release-store.
  - **EditClass** — each reload is classified from the graph: `Rejected` (diagnostics;
    nothing compiled or patched, last-good keeps serving) / `NoEffect` / `SafeSwap`
    (lock-free single-slot store) / `Relink` (a signature moved → the whole proven
    radius recompiles and commits under the dispatch write-lock quiescence barrier).
  - **CallState** (`abi.rs`) — every generated function threads a hidden context
    pointer (depth, fuel, trap) allocated on the caller's stack, so runaway recursion
    and infinite loops abort the *call* with a typed error, never the process.
  - **Canary** — a candidate compiled as a fully isolated `LiveRuntime`,
    shadow-executed on a lock-free-sampled fraction of live calls; the caller always
    receives the live answer. It inherits the primary's fuel/depth limits.
  - **Journal / rollback** — every reload event is recorded with its source snapshot;
    `rollback(gen)` replays a stored source through the same classified reload path.

## Invariants you must not break

These span files; breaking any one silently voids a soundness proof:

1. **Firewall dependency rule** — no query may make a caller's IR or codegen depend on
   a callee's *body*. Callers read signatures only. This is what bounds blast radius.
2. **Arity-before-code publish** — `SwapTable::publish` stores arity, then code (both
   release-ordered); `call_handle` double-checks arity around its code load. Reorder
   either side and mis-arity dispatch UB comes back.
3. **`diag.rs` mirrors `lower.rs`** — every construct the lowerer handles must be
   checked by the gate with identical walking order and scoping, or `Rejected` lies
   and mangled semantics go live.
4. **`CallState` layout is ABI** — the `#[repr(C)]` field offsets are baked into
   generated loads/stores via the `OFF_*` constants; the unit test in `abi.rs` pins
   them. Change both together or neither.
5. **Lock discipline** — the dispatch `RwLock` read is held for a root call's full
   duration (that *is* the Relink quiescence barrier); the `inner` mutex is
   compile-path only and never touched on the call path; the canary lock is taken only
   for the sampled fraction (`canary_rate`/`canary_counter` decide lock-free).
6. **Generations are retired, never freed** — a concurrent caller may still be inside
   old code. The retention is deliberate; don't "fix the leak".

## Discipline (the repo's non-negotiables)

- New guarantee ⇒ a concurrent attacking test plus a row in README's
  "Claims → tests" table — use `/add-guarantee`.
- Language growth walks every station lexer→codegen with the gate updated in the same
  commit as the lowerer — use `/grow-language`.
- Performance numbers are real, machine-labeled, and updated as a complete set — use
  `/bench-and-readme`.
- Before any commit: `/verify`.

## Cross-file gotchas

- `examples/living_service/service.rs` is `#[path]`-included by
  `tests/living_service.rs`, and `examples/agent_loop.rs` by `tests/agent_loop.rs`
  (its `main` is `pub` for that reason). Each build uses a different half of the
  shared module — hence `#[allow(dead_code)]` at the include/module sites.
- `rhai` is a bench-only `[dev-dependencies]` entry for `bench_vs_interpreter`; keep
  it out of `[dependencies]`.
- Latency assertions in story tests use generous ceilings (`COMMITTED_PROMPTLY` =
  100ms) because CI runs debug builds under load; precise latency claims live only in
  the release benchmarks.
- Host functions must not call back into the runtime (documented in
  `register_host_fn`'s safety contract).
