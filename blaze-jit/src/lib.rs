//! `blaze-jit` — the Cranelift code-generation backend and hot-swap execution
//! harness (architecture Milestones 2 & 3).
//!
//! This crate ingests validated [`blaze_ir::FunctionNode`]s and translates the
//! DevIR into Cranelift IR, JIT-compiles them into executable memory, and
//! (Milestone 3) hot-swaps the resulting code pages under a running program.
//!
//! Milestone 1 (the lexer, parser, DevIR graph, and incremental firewall) is
//! implemented and verified in `blaze-parse` and `blaze-ir`. The backend is
//! introduced next; this module is intentionally left as the seam it plugs into.

#![doc(html_no_source)]

/// Placeholder marker so the crate participates in the workspace build while the
/// Cranelift backend (Milestone 2) is brought online in the following commits.
pub const MILESTONE: &str = "1-complete; jit backend pending";
