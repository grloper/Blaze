//! `blaze-ir` — lowering the Blaze CST into an SSA-style **DevIR** wired into a
//! `salsa` demand-driven query database.
//!
//! This crate is the incremental heart of Blaze. It turns the whole-file source
//! ([`SourceProgram`]) into per-function [`FunctionNode`]s through a graph of
//! `#[salsa::tracked]` queries ([`function_signature`], [`lowered_dev_ir`], …).
//! The graph is shaped so `salsa`'s early-cutoff gives the invariant the
//! architecture demands: *editing a function body invalidates that function's
//! DevIR but not its callers'*, because callers depend only on callee
//! signatures. See [`lower`] for the graph diagram and the `tests/` directory
//! for the machine-checked proof of that property.

pub mod db;
pub mod ir;
pub mod lower;

pub use db::{BlazeDatabase, BlazeDatabaseImpl, ExecTrace, FnKey, HasExecTrace, SourceProgram};
pub use ir::{function_id_of, CmpKind, FunctionId, FunctionNode, IrBuilder, IrOp, Signature, Type};
pub use lower::{function_signature, function_text, lowered_dev_ir, program_outline};
