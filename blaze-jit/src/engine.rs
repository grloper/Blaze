//! The in-process JIT: links a set of DevIR functions into executable memory
//! via [`JITModule`] (which manages the `mmap`'d `PROT_EXEC` pages) and hands
//! back callable pointers.
//!
//! This is the executable counterpart to the isolated byte-compiler in
//! [`crate::codegen`]: here calls are resolved against real, colocated
//! definitions so the finalized code is directly invocable.

use std::collections::HashMap;

use cranelift_codegen::ir::{AbiParam, types, Function, UserFuncName};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, FuncId, Linkage, Module};

use blaze_ir::{FunctionId, FunctionNode};

use crate::codegen::{build_body, host_isa, CalleeResolver};

/// Resolves DevIR callees against functions already declared in the module.
struct JitResolver<'a> {
    module: &'a mut JITModule,
    ids: &'a HashMap<FunctionId, FuncId>,
}

impl CalleeResolver for JitResolver<'_> {
    fn resolve(&mut self, func: &mut Function, callee: FunctionId, _arity: usize) -> cranelift_codegen::ir::FuncRef {
        let func_id = *self
            .ids
            .get(&callee)
            .expect("every called function must be declared before definition");
        self.module.declare_func_in_func(func_id, func)
    }
}

/// A JIT compilation session. Owns the executable pages of everything it has
/// compiled; dropping it frees them.
pub struct JitEngine {
    module: JITModule,
    /// DevIR id → Cranelift module id (for call resolution).
    ids: HashMap<FunctionId, FuncId>,
    /// Source name → Cranelift module id (for invocation).
    by_name: HashMap<String, FuncId>,
}

impl JitEngine {
    /// Create an engine targeting the host machine.
    pub fn new() -> Result<Self, String> {
        let isa = host_isa()?;
        let module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
        Ok(JitEngine { module, ids: HashMap::new(), by_name: HashMap::new() })
    }

    /// Compile and link a whole program: `funcs` pairs each function's source
    /// name with its lowered DevIR node. After this returns, [`Self::call`] can
    /// invoke any of them.
    pub fn compile(&mut self, funcs: &[(String, FunctionNode)]) -> Result<(), String> {
        // Pass 1 — declare every function so intra-program calls can be linked.
        for (name, node) in funcs {
            let mut sig = self.module.make_signature();
            for _ in 0..node.signature.arity() {
                sig.params.push(AbiParam::new(types::I64));
            }
            sig.returns.push(AbiParam::new(types::I64));
            let func_id = self
                .module
                .declare_function(name, Linkage::Export, &sig)
                .map_err(|e| format!("declare `{name}` failed: {e}"))?;
            self.ids.insert(node.id, func_id);
            self.by_name.insert(name.clone(), func_id);
        }

        // Pass 2 — define each body and translate it to Cranelift IR.
        for (name, node) in funcs {
            let func_id = self.ids[&node.id];
            let frontend_cfg = self.module.target_config();

            let mut ctx = self.module.make_context();
            ctx.func.signature = {
                let mut sig = self.module.make_signature();
                for _ in 0..node.signature.arity() {
                    sig.params.push(AbiParam::new(types::I64));
                }
                sig.returns.push(AbiParam::new(types::I64));
                sig
            };
            ctx.func.name = UserFuncName::user(0, func_id.as_u32());

            let mut fb_ctx = FunctionBuilderContext::new();
            {
                let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
                let mut resolver = JitResolver { module: &mut self.module, ids: &self.ids };
                build_body(&mut builder, node, &mut resolver);
                builder.finalize(frontend_cfg);
            }

            self.module
                .define_function(func_id, &mut ctx)
                .map_err(|e| format!("define `{name}` failed: {e}"))?;
            self.module.clear_context(&mut ctx);
        }

        // Resolve relocations and flip pages to `PROT_EXEC`.
        self.module
            .finalize_definitions()
            .map_err(|e| format!("finalization failed: {e}"))?;
        Ok(())
    }

    /// Invoke a compiled function by name. Supports arities 0..=4 (sufficient
    /// for the C-subset); returns `None` for unknown names or higher arities.
    ///
    /// # Safety
    ///
    /// Internally transmutes the finalized code pointer to a C ABI function.
    /// This is sound because every Blaze function is compiled with the
    /// `(i64, …) -> i64` signature this dispatches on.
    pub fn call(&self, name: &str, args: &[i64]) -> Option<i64> {
        let func_id = *self.by_name.get(name)?;
        let code = self.module.get_finalized_function(func_id);
        // SAFETY: `code` points at finalized, executable code generated with the
        // matching `(i64 * n) -> i64` signature; arity is checked here.
        unsafe {
            let result = match args {
                [] => std::mem::transmute::<*const u8, extern "C" fn() -> i64>(code)(),
                [a] => std::mem::transmute::<*const u8, extern "C" fn(i64) -> i64>(code)(*a),
                [a, b] => {
                    std::mem::transmute::<*const u8, extern "C" fn(i64, i64) -> i64>(code)(*a, *b)
                }
                [a, b, c] => std::mem::transmute::<*const u8, extern "C" fn(i64, i64, i64) -> i64>(
                    code,
                )(*a, *b, *c),
                [a, b, c, d] => std::mem::transmute::<
                    *const u8,
                    extern "C" fn(i64, i64, i64, i64) -> i64,
                >(code)(*a, *b, *c, *d),
                _ => return None,
            };
            Some(result)
        }
    }
}
