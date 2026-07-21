---
name: grow-language
description: Add a language feature to Blaze — a token, operator, statement, control-flow form, or type — through the full seven-station pipeline with the diagnostics gate kept in lockstep with the lowerer. Use whenever the user asks for new syntax or semantics ("add strings", "support for-loops", "a modulo operator", "explicit int↔float casts", "arrays"), or for any change touching blaze-parse or blaze-ir lowering, even when phrased as a small tweak.
---

# Grow the language (all stations, gate in lockstep)

The reload guarantees survive language growth only if every station learns the feature
together. The most dangerous shortcut available here: teaching `lower.rs` a construct
without teaching `diag.rs` the matching checks. The gate then *approves programs it
doesn't understand*, and `Rejected` silently stops being a guarantee. Lowerer and gate
move in the same commit, always.

## The stations, in order

1. **Lexer** — `blaze-parse/src/lexer.rs` (logos). Watch two-char operators winning
   over their one-char prefixes; extend the losslessness test.
2. **Parser** — `blaze-parse/src/parser.rs` (recursive descent → rowan CST). The CST
   must stay lossless — extend `round_trips_source_text`. Malformed input becomes
   error nodes, never a panic: the parser survives anything; rejection is the gate's
   job, not the parser's.
3. **AST** — `blaze-parse/src/ast.rs`: typed accessors over the new nodes.
4. **Lowering** — `blaze-ir/src/lower.rs` → DevIR. Write down the evaluation-order and
   scoping decisions you make here; station 5 must replicate them exactly.
5. **The gate** — `blaze-ir/src/diag.rs`. Re-walk the new construct with the *same*
   statement order and declare-before-use scoping as the lowerer, and emit a
   diagnostic at every point where lowering would otherwise substitute a default. If
   the feature is typed, extend the type rules — mixed types are an error; Blaze has
   no implicit conversions, which is what makes the bit-cast ABI sound.
6. **Codegen** — `blaze-jit/src/codegen.rs` (DevIR → Cranelift). Safety hooks to wire:
   - every new **loop back-edge** spends fuel (follow the `while` lowering);
   - every new **call form** passes the entry depth guard and dispatches through the
     slot table (`CallEmitter`), never a direct relocation;
   - every op that can **trap natively** gets guarded like division (`x/0 == 0`,
     `INT_MIN / -1 == INT_MIN`) — a script must never be able to fault the process.
7. **Tests at every station**, plus two runtime ones: a live-edit test proving the
   feature hot-swaps correctly under a second thread, and a diagnostics test proving
   the gate rejects its misuse.

## Two questions that decide reload classification

- **Does it change what a `Signature` is** (parameter count, types, return type)?
  Signature equality drives Relink-vs-SafeSwap, so follow the floats precedent
  (`retyping_a_parameter_relinks_atomically_under_fire`): test the transition under
  concurrent fire, and expect the firewall to pull callers into the radius.
- **Does it add a value representation?** Every value crosses the ABI as one raw
  64-bit word, bit-cast only at call/return boundaries (`Value::to_abi/from_abi`) —
  and the type checker must make cross-type reinterpretation *provably unreachable*
  before codegen may rely on that. A new representation also touches
  `clif_signature`, `Value`, `call_typed`/`call_handle_typed`, and the host
  trampolines. `MAX_ARITY` (8) bounds the dispatch table in `abi.rs::invoke`; grow it
  there if the feature changes arity ranges.

## Firewall check before you ship

Run the `blaze-ir` incremental tests and confirm the asymmetry held: a body-only use
of the new feature re-lowers exactly one function (callers stay `Arc::ptr_eq` memo
hits), while a signature-visible use cascades to callers. If a body edit starts
invalidating callers, body information leaked into a signature-facing query — that is
a firewall breach; fix it before anything else, because every blast-radius proof
downstream depends on it.
