//! `blaze-jit` — the Cranelift code-generation backend and in-memory execution
//! harness (architecture Milestones 2 & 3).
//!
//! It ingests validated [`blaze_ir::FunctionNode`]s and:
//!  * translates DevIR into Cranelift IR ([`codegen`]),
//!  * serializes per-function machine code back into the `salsa` graph
//!    ([`compiled_machine_code`]), so codegen inherits the incremental firewall,
//!  * links whole programs into executable `mmap` pages and runs them
//!    ([`JitEngine`], via Cranelift's `JITModule`).
//!
//! The DevIR-level firewall (proved in `blaze-ir`) carries all the way through:
//! because [`compiled_machine_code`] depends only on `lowered_dev_ir`, editing a
//! callee's body recompiles that callee's code while callers stay memoized.

pub mod codegen;
pub mod engine;
pub mod query;

pub use codegen::{compile_isolated, host_isa};
pub use engine::JitEngine;
pub use query::{compiled_machine_code, jit_program, CompiledFunction};
