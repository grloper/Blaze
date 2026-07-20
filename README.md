# Blaze

An **incremental computation-graph middleware** for compilers. Blaze sits
between a language frontend (AST diffs) and a lightweight JIT backend
(Cranelift), abstracting a codebase into a fine-grained, demand-driven query
graph so that a one-line edit recompiles one function — not the translation
unit.

The core invariant, from the architecture spec:

> If a function body's internal statement graph is mutated but its ABI
> signature remains invariant, the invalidation cascade terminates strictly at
> the function boundary. Callers remain O(1) unaffected.

Blaze implements this as a **firewall** in the query graph and proves it with a
machine-checked test suite.

## Workspace

| Crate         | Responsibility |
|---------------|----------------|
| `blaze-parse` | `logos` lexer → hand-written recursive-descent parser → lossless `rowan` CST → typed AST, for a minimalist `int`-typed C-subset (functions, locals, `+`, calls, `return`). |
| `blaze-ir`    | Lowers the CST into an SSA-style **DevIR** and wires it into a `salsa` demand-driven query database (raw source → per-function text → signature → lowered DevIR). |
| `blaze-jit`   | Translates DevIR into Cranelift IR, serializes per-function machine code back into the query graph, and links whole programs into executable `mmap` pages via Cranelift's `JITModule`. |

## The firewall

The query graph is shaped so `salsa`'s early-cutoff (backdating) yields the
invariant automatically:

```
             raw source (SourceProgram input)
                        │
          ┌─────────────┴──────────────┐
          ▼                            ▼
    program_outline            function_text(f)      ← per-function firewall node
                                       │
                        ┌──────────────┴──────────────┐
                        ▼                             ▼
                function_signature(f)          lowered_dev_ir(f)
                        ▲                             │
                        └──────── callee signature ──┘   ← a caller reads only
                                                            the callee's SIGNATURE
```

A caller's `lowered_dev_ir` depends on each callee's **signature**, never its
body. So editing a body re-lowers that function while `function_signature`
backdates to an equal value — and the caller is a byte-for-byte memo hit.
Because `compiled_machine_code` depends only on `lowered_dev_ir`, the firewall
extends transparently to codegen.

`blaze-ir/tests/incremental.rs` verifies this by inspecting which query bodies
re-executed and whether memoized `Arc`s were reused:

- `body_edit_does_not_invalidate_callers` — editing a callee body re-lowers the
  callee but the caller is reused verbatim.
- `signature_edit_does_cascade_to_callers` — an ABI change *does* propagate.
- `editing_a_caller_leaves_the_callee_cached` — the dual direction.

`blaze-jit/tests/jit.rs` shows the same firewall at the machine-code level, plus
real JIT execution and hot recompilation.

## Build & run

```sh
cargo test --workspace          # 19 tests: lexer, parser, AST, firewall, JIT
cargo run -p blaze-jit --example run
```

## Design note: modern `salsa`

The original blueprint sketched the database with `#[salsa::query_group]`. That
macro belonged to `salsa` ≤ 0.17 and was removed in the 2022 rewrite; the
current release (0.28) models a query group as `#[salsa::tracked]` free
functions over a `#[salsa::db]` trait. Blaze targets the maintained API — the
same one rust-analyzer uses — so the code is idiomatic and forward-compatible.
The blueprint's *intent* (an `input` for raw source and derived
`function_signature` / `lowered_dev_ir` / `compiled_machine_code` queries) is
implemented faithfully; only the surface macro syntax differs.

## Status

- **Milestone 1 — Lexer, parser, DevIR graph, incremental firewall:** complete
  and verified.
- **Milestone 2 — Cranelift JIT backend:** complete — DevIR → Cranelift IR,
  machine code memoized in `salsa`, whole-program linking and execution.
- **Milestone 3 — Hot-swap execution harness:** the recompile-and-re-execute
  loop is demonstrated (`blaze-jit/examples/run.rs`, `hot_recompile_*` test)
  atop `JITModule`'s executable-page management; in-place single-page patching
  under a live process and OS-level file-watch driving are the remaining depth.

Licensed under Apache-2.0.
