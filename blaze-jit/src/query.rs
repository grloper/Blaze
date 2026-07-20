//! The `salsa` codegen query and the database-driven JIT driver.
//!
//! [`compiled_machine_code`] serializes Cranelift's output back into the
//! incremental graph: it depends only on [`lowered_dev_ir`], so the DevIR-level
//! firewall extends transparently to machine code — editing a callee's body
//! recompiles that callee's bytes while callers stay memoized.

use std::sync::{Arc, OnceLock};

use cranelift_codegen::isa::OwnedTargetIsa;

use blaze_ir::db::{BlazeDatabase, FnKey, SourceProgram};
use blaze_ir::lower::{lowered_dev_ir, program_outline};
use blaze_ir::{FunctionId, Signature};

use crate::codegen::{compile_isolated, host_isa};
use crate::engine::JitEngine;

/// The host ISA is expensive to construct and never changes within a process,
/// so it is built once and shared across every codegen query.
static HOST_ISA: OnceLock<OwnedTargetIsa> = OnceLock::new();

fn cached_isa() -> &'static OwnedTargetIsa {
    HOST_ISA.get_or_init(|| host_isa().expect("host machine must be a supported Cranelift target"))
}

/// A function's compiled artifact: its ABI plus the raw machine-code buffer
/// Cranelift emitted. `'static` and content-hashable, so `salsa` can memoize it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompiledFunction {
    pub id: FunctionId,
    pub signature: Signature,
    /// Relocatable machine code for this function alone.
    pub code: Arc<[u8]>,
}

impl CompiledFunction {
    /// Length in bytes of the emitted machine code.
    pub fn code_len(&self) -> usize {
        self.code.len()
    }
}

/// Compile function `key` to machine code, memoized in the query graph.
#[salsa::tracked(returns(clone))]
pub fn compiled_machine_code<'db>(
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
    key: FnKey<'db>,
) -> Arc<CompiledFunction> {
    let node = lowered_dev_ir(db, src, key);
    db.record_exec(format!("compiled_machine_code({})", key.name(db)));

    let code = match compile_isolated(cached_isa().as_ref(), &node) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("blaze-jit: failed to compile `{}`: {err}", key.name(db));
            Vec::new()
        }
    };

    Arc::new(CompiledFunction {
        id: node.id,
        signature: node.signature.clone(),
        code: Arc::from(code),
    })
}

/// JIT-compile and link an entire program straight from the database, returning
/// a ready-to-invoke [`JitEngine`]. Pulls each function's DevIR through the
/// incremental graph, so unchanged functions are lowered from cache.
pub fn jit_program(db: &dyn BlazeDatabase, src: SourceProgram) -> Result<JitEngine, String> {
    let names = program_outline(db, src);
    let mut funcs = Vec::with_capacity(names.len());
    for name in names.iter() {
        let key = FnKey::new(db, name.clone());
        let node = lowered_dev_ir(db, src, key);
        funcs.push((name.clone(), (*node).clone()));
    }

    let mut engine = JitEngine::new()?;
    engine.compile(&funcs)?;
    Ok(engine)
}
