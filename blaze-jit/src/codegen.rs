//! Translation of DevIR into Cranelift IR, plus host-ISA construction and
//! isolated (module-free) machine-code emission.
//!
//! DevIR registers are *mutable virtual registers* (see `blaze_ir::IrOp`), so
//! they map 1:1 onto `cranelift-frontend` [`Variable`]s and the frontend's SSA
//! builder reconstructs phis across control-flow joins mechanically. Labels map
//! 1:1 onto Cranelift blocks.
//!
//! The single [`build_body`] pass is shared by every backend — the direct
//! in-process JIT, the isolated byte compiler, and the live hot-swap runtime —
//! via the [`CallEmitter`] seam, which abstracts the one thing that differs:
//! how a call to another Blaze function is emitted.
//!
//! **No emitted instruction can fault the host process.** Division is guarded
//! (`x / 0 == 0`, `INT_MIN / -1 == INT_MIN`) instead of trapping, because a
//! live-edited script must never be able to take down the process embedding it.

use std::collections::HashMap;

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    types, AbiParam, ExtFuncData, ExternalName, InstBuilder, Signature as ClifSig,
    UserExternalName, UserFuncName, Value,
};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

use blaze_ir::{CmpKind, FunctionId, FunctionNode, IrOp};

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

/// Emits a call to another Blaze function. The three backends differ only here:
///  * direct-module: `call` against a `FuncId` declared in a live `JITModule`;
///  * isolated: `call` against an imported user external (a relocation);
///  * live table: `call_indirect` through the hot-swap slot table.
pub trait CallEmitter {
    fn emit_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee: FunctionId,
        args: &[Value],
    ) -> Value;
}

/// Emit the body of `node` into `builder`. The builder must be freshly created
/// for `node`'s function; this seals all blocks but the caller finalizes.
pub fn build_body(builder: &mut FunctionBuilder, node: &FunctionNode, emitter: &mut dyn CallEmitter) {
    let arity = node.signature.arity();
    let nregs = node.register_count().max(arity);

    // One frontend Variable per DevIR register; the SSA builder handles phis.
    let vars: Vec<_> = (0..nregs).map(|_| builder.declare_var(types::I64)).collect();

    // One Cranelift block per bound DevIR label (lowering guarantees every
    // referenced label is bound exactly once).
    let mut blocks: HashMap<u32, cranelift_codegen::ir::Block> = HashMap::new();
    for op in &node.body {
        if let IrOp::Label(l) = op {
            blocks.insert(*l, builder.create_block());
        }
    }

    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);

    // Seed parameters, then zero-initialize every other register so a `use_var`
    // is total on all paths even for variables first assigned inside a branch
    // (the C-subset has flat function scope).
    let params = builder.block_params(entry).to_vec();
    for (reg, value) in params.into_iter().enumerate() {
        builder.def_var(vars[reg], value);
    }
    if nregs > arity {
        let zero = builder.ins().iconst(types::I64, 0);
        for var in &vars[arity..] {
            builder.def_var(*var, zero);
        }
    }

    let mut terminated = false;
    for op in &node.body {
        // Skip straight-line ops that lowering placed nowhere reachable; only a
        // Label opens a new block. (Lowering never actually emits dead code —
        // this is defensive.)
        if terminated && !matches!(op, IrOp::Label(_)) {
            continue;
        }
        match op {
            IrOp::LoadInt(dst, imm) => {
                let v = builder.ins().iconst(types::I64, *imm);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Add(dst, lhs, rhs) => {
                let (a, b) = (builder.use_var(vars[*lhs as usize]), builder.use_var(vars[*rhs as usize]));
                let v = builder.ins().iadd(a, b);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Sub(dst, lhs, rhs) => {
                let (a, b) = (builder.use_var(vars[*lhs as usize]), builder.use_var(vars[*rhs as usize]));
                let v = builder.ins().isub(a, b);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Mul(dst, lhs, rhs) => {
                let (a, b) = (builder.use_var(vars[*lhs as usize]), builder.use_var(vars[*rhs as usize]));
                let v = builder.ins().imul(a, b);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Div(dst, lhs, rhs) => {
                let (a, b) = (builder.use_var(vars[*lhs as usize]), builder.use_var(vars[*rhs as usize]));
                let v = emit_guarded_sdiv(builder, a, b);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Cmp(kind, dst, lhs, rhs) => {
                let (a, b) = (builder.use_var(vars[*lhs as usize]), builder.use_var(vars[*rhs as usize]));
                let cc = match kind {
                    CmpKind::Lt => IntCC::SignedLessThan,
                    CmpKind::Le => IntCC::SignedLessThanOrEqual,
                    CmpKind::Gt => IntCC::SignedGreaterThan,
                    CmpKind::Ge => IntCC::SignedGreaterThanOrEqual,
                    CmpKind::Eq => IntCC::Equal,
                    CmpKind::Ne => IntCC::NotEqual,
                };
                let flag = builder.ins().icmp(cc, a, b);
                let v = builder.ins().uextend(types::I64, flag);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Copy(dst, src) => {
                let v = builder.use_var(vars[*src as usize]);
                builder.def_var(vars[*dst as usize], v);
            }
            IrOp::Call(dst, callee, args) => {
                let arg_values: Vec<Value> =
                    args.iter().map(|r| builder.use_var(vars[*r as usize])).collect();
                let result = emitter.emit_call(builder, *callee, &arg_values);
                builder.def_var(vars[*dst as usize], result);
            }
            IrOp::Return(value) => {
                let v = builder.use_var(vars[*value as usize]);
                builder.ins().return_(&[v]);
                terminated = true;
            }
            IrOp::Jump(label) => {
                builder.ins().jump(blocks[label], &[]);
                terminated = true;
            }
            IrOp::Branch(cond, then_label, else_label) => {
                let c = builder.use_var(vars[*cond as usize]);
                // Normalize to a flag so `brif` sees a canonical boolean.
                let flag = builder.ins().icmp_imm_s(IntCC::NotEqual, c, 0);
                builder
                    .ins()
                    .brif(flag, blocks[then_label], &[], blocks[else_label], &[]);
                terminated = true;
            }
            IrOp::Label(label) => {
                let block = blocks[label];
                if !terminated {
                    // Defensive fall-through edge; lowering always terminates
                    // blocks explicitly, so this is normally unreachable.
                    builder.ins().jump(block, &[]);
                }
                builder.switch_to_block(block);
                terminated = false;
            }
        }
    }

    // Cranelift requires every block to end in a terminator. A body that falls
    // off the end returns 0 (defensive: lowering already guarantees a Return).
    if !terminated {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[zero]);
    }

    builder.seal_all_blocks();
}

/// Signed division that cannot trap: `x / 0 == 0`, `INT_MIN / -1 == INT_MIN`.
///
/// Both x86 and Cranelift's `sdiv` fault on these two inputs; a live-edited
/// script must never be able to fault the embedding process, so the guards are
/// part of the language semantics, not an optimization choice.
fn emit_guarded_sdiv(builder: &mut FunctionBuilder, a: Value, b: Value) -> Value {
    let zero = builder.ins().iconst(types::I64, 0);
    let one = builder.ins().iconst(types::I64, 1);
    let min = builder.ins().iconst(types::I64, i64::MIN);
    let neg_one = builder.ins().iconst(types::I64, -1);

    let b_is_zero = builder.ins().icmp_imm_s(IntCC::Equal, b, 0);
    let a_is_min = builder.ins().icmp(IntCC::Equal, a, min);
    let b_is_neg_one = builder.ins().icmp(IntCC::Equal, b, neg_one);
    let overflows = builder.ins().band(a_is_min, b_is_neg_one);

    // Force the divisor to 1 in both faulting cases, then patch the result.
    let safe_b = builder.ins().select(b_is_zero, one, b);
    let safe_b = builder.ins().select(overflows, one, safe_b);
    let quotient = builder.ins().sdiv(a, safe_b);
    let quotient = builder.ins().select(overflows, a, quotient);
    builder.ins().select(b_is_zero, zero, quotient)
}

/// Emits callees as imported *user externals*, producing relocatable machine
/// code with no live module. Used by the `salsa` byte-compiler.
struct IsolatedEmitter {
    call_conv: CallConv,
    next_index: u32,
}

impl CallEmitter for IsolatedEmitter {
    fn emit_call(
        &mut self,
        builder: &mut FunctionBuilder,
        _callee: FunctionId,
        args: &[Value],
    ) -> Value {
        let sig = clif_signature(args.len(), self.call_conv);
        let sig_ref = builder.func.import_signature(sig);
        let index = self.next_index;
        self.next_index += 1;
        let name_ref = builder
            .func
            .declare_imported_user_function(UserExternalName::new(0, index));
        let callee_ref = builder.func.import_function(ExtFuncData {
            name: ExternalName::user(name_ref),
            signature: sig_ref,
            colocated: false,
            patchable: false,
        });
        let call = builder.ins().call(callee_ref, args);
        builder.inst_results(call)[0]
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
        let mut emitter = IsolatedEmitter { call_conv, next_index: 0 };
        build_body(&mut builder, node, &mut emitter);
        builder.finalize(isa.frontend_config());
    }

    let mut ctrl = ControlPlane::default();
    let code = ctx
        .compile(isa, &mut ctrl)
        .map_err(|e| format!("cranelift codegen failed: {:?}", e.inner))?;
    Ok(code.code_buffer().to_vec())
}
