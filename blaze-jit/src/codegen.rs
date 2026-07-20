//! Translation of DevIR into Cranelift IR, plus host-ISA construction and
//! isolated (module-free) machine-code emission.
//!
//! Because DevIR is already in SSA form with dense `u32` value numbers and no
//! control flow (the C-subset is single-block), lowering is a direct map from
//! `u32` register → Cranelift `Value`; no `Variable`s, `phi`s, or block routing
//! are required. The single [`build_body`] pass is shared by both backends —
//! the in-process JIT (calls resolved through a live [`JITModule`]) and the
//! isolated byte compiler (calls resolved as imported user externals) — via the
//! [`CalleeResolver`] seam.

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::{
    types, AbiParam, ExtFuncData, ExternalName, FuncRef, Function, InstBuilder, Signature as ClifSig,
    UserExternalName, UserFuncName, Value,
};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

use blaze_ir::{FunctionId, FunctionNode, IrOp};

/// Build a Cranelift target ISA for the host machine.
///
/// PIC and colocated libcalls are disabled: the JIT places code at absolute
/// addresses in the current process, which is exactly the model Cranelift's own
/// JIT example uses.
pub fn host_isa() -> Result<OwnedTargetIsa, String> {
    let mut flags = settings::builder();
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| e.to_string())?;
    flags.set("is_pic", "false").map_err(|e| e.to_string())?;
    let isa_builder = cranelift_native::builder().map_err(|m| m.to_string())?;
    isa_builder
        .finish(settings::Flags::new(flags))
        .map_err(|e| e.to_string())
}

/// The Cranelift signature for a Blaze function of the given arity: every
/// parameter and the single result are machine `i64`.
pub fn clif_signature(arity: usize, call_conv: CallConv) -> ClifSig {
    let mut sig = ClifSig::new(call_conv);
    for _ in 0..arity {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

/// Resolves a DevIR [`FunctionId`] callee into a Cranelift [`FuncRef`] usable
/// inside the function currently being built. The two backends differ only in
/// how a call target is named, which this abstracts.
pub trait CalleeResolver {
    fn resolve(&mut self, func: &mut Function, callee: FunctionId, arity: usize) -> FuncRef;
}

/// Emit the body of `node` into `builder`. The builder must be freshly created
/// for `node`'s function; the caller finalizes it afterward.
pub fn build_body(builder: &mut FunctionBuilder, node: &FunctionNode, resolver: &mut dyn CalleeResolver) {
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    // Map DevIR registers to Cranelift values. Parameters occupy registers
    // `0..arity` and are seeded from the entry block's parameters.
    let slots = node.register_count().max(node.signature.arity());
    let mut values: Vec<Option<Value>> = vec![None; slots];
    for (reg, value) in builder.block_params(entry).to_vec().into_iter().enumerate() {
        values[reg] = Some(value);
    }

    let get = |values: &[Option<Value>], reg: u32| -> Value {
        values[reg as usize].expect("DevIR is SSA: every register is defined before use")
    };

    let mut terminated = false;
    for op in &node.body {
        match op {
            IrOp::LoadInt(dst, imm) => {
                let v = builder.ins().iconst(types::I64, *imm);
                values[*dst as usize] = Some(v);
            }
            IrOp::Add(dst, lhs, rhs) => {
                let a = get(&values, *lhs);
                let b = get(&values, *rhs);
                let v = builder.ins().iadd(a, b);
                values[*dst as usize] = Some(v);
            }
            IrOp::Call(dst, callee, args) => {
                let arg_values: Vec<Value> = args.iter().map(|r| get(&values, *r)).collect();
                let callee_ref = resolver.resolve(builder.func, *callee, args.len());
                let call = builder.ins().call(callee_ref, &arg_values);
                let result = builder.inst_results(call)[0];
                values[*dst as usize] = Some(result);
            }
            IrOp::Return(value) => {
                let v = get(&values, *value);
                builder.ins().return_(&[v]);
                terminated = true;
                break; // nothing after a return is reachable
            }
        }
    }

    // Cranelift requires every block to end in a terminator. A function that
    // falls off the end returns 0 (the C-subset has no `void`).
    if !terminated {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[zero]);
    }
}

/// Resolves callees as imported *user externals*, producing relocatable machine
/// code with no live module. Used by the `salsa` byte-compiler.
struct IsolatedResolver {
    call_conv: CallConv,
    next_index: u32,
}

impl CalleeResolver for IsolatedResolver {
    fn resolve(&mut self, func: &mut Function, _callee: FunctionId, arity: usize) -> FuncRef {
        let sig = clif_signature(arity, self.call_conv);
        let sig_ref = func.import_signature(sig);
        let index = self.next_index;
        self.next_index += 1;
        let name_ref = func.declare_imported_user_function(UserExternalName::new(0, index));
        func.import_function(ExtFuncData {
            name: ExternalName::user(name_ref),
            signature: sig_ref,
            colocated: false,
            patchable: false,
        })
    }
}

/// Compile a single [`FunctionNode`] to a raw machine-code buffer, independent
/// of any module. Calls become relocations against imported externals; the
/// bytes are the artifact the incremental graph memoizes.
pub fn compile_isolated(isa: &dyn TargetIsa, node: &FunctionNode) -> Result<Vec<u8>, String> {
    let call_conv = isa.default_call_conv();
    let mut ctx = Context::new();
    ctx.func.signature = clif_signature(node.signature.arity(), call_conv);
    ctx.func.name = UserFuncName::user(0, node.id.0);

    let mut fb_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let mut resolver = IsolatedResolver { call_conv, next_index: 0 };
        build_body(&mut builder, node, &mut resolver);
        builder.finalize(isa.frontend_config());
    }

    let mut ctrl = ControlPlane::default();
    let code = ctx
        .compile(isa, &mut ctrl)
        .map_err(|e| format!("cranelift codegen failed: {:?}", e.inner))?;
    Ok(code.code_buffer().to_vec())
}
