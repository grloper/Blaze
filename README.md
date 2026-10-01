# Blaze

A Rust workspace containing a lexer/parser, incremental IR queries, and a Cranelift JIT with a live function-slot table.

## Implemented and exercised

The Windows audit ran all 110 existing tests with Rust 1.94.0 and ran the native `living_service` scripted example against its own ephemeral loopback HTTP service. The six scripted cases exercised a body edit, rejection of invalid source, a fuel-exhausted canary, promotion after divergence, ABI relinking, and rollback. This is local test evidence, not a production deployment or a formal proof.

The swap table now uses a fixed boxed allocation of atomics. It stores data pointers; generated calls retain stable slot addresses. This replaces Unix-only mmap/munmap calls that prevented Windows compilation. The minimum Rust version is 1.94, matching the locked Cranelift dependencies.

## Reproduce

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p blaze-jit --example living_service -- --script
cargo run -p blaze-jit --example agent_loop
```

The scripted examples operate on local sample programs. `living_service` binds only to an ephemeral loopback port. No model/API key is required.

## Source map

- `blaze-parse/src/`: lexer, lossless syntax tree and parser.
- `blaze-ir/src/`: IR lowering, diagnostics and incremental queries.
- `blaze-jit/src/live.rs`: function slots, edit classification, canaries and rollback.
- `blaze-jit/examples/living_service/`: native local demonstration and its shared scenario assertions.

## Limits

This audit validates Windows fixture tests and one local demo workload. Linux/macOS were not rerun here. No independent security review, universal correctness theorem, production reliability, zero-downtime guarantee, or hardware-independent latency figure is claimed. Older executable generations are retained; long-running memory growth requires evaluation. The portfolio's JavaScript hot-swap illustration is not this Rust runtime.
