//! The derived queries that turn source text into DevIR, and the lowering pass
//! behind them.
//!
//! The query graph is shaped so that the incremental *firewall* falls out of
//! `salsa`'s early-cutoff (backdating) automatically:
//!
//! ```text
//!            raw_source (SourceProgram input)
//!                    │
//!         ┌──────────┴───────────┐
//!         ▼                      ▼
//!   program_outline        function_text(f)          ← per-function firewall node
//!                                 │
//!                    ┌────────────┴───────────┐
//!                    ▼                         ▼
//!            function_signature(f)      lowered_dev_ir(f)
//!                    ▲                         │
//!                    └───────── (callee sig) ──┘   ← a caller reads only the
//!                                                     callee's SIGNATURE
//! ```
//!
//! Editing a function body changes `function_text(f)` and re-runs
//! `lowered_dev_ir(f)`, but `function_signature(f)` re-runs to an *equal* value
//! and is backdated. Because a caller depends on the callee's signature — never
//! its body — the caller's `lowered_dev_ir` is not invalidated.

use std::collections::HashMap;
use std::sync::Arc;

use blaze_parse::ast::{BinExpr, BinOp, Block, CallExpr, ElseArm, Expr, Function, Lit, SourceFile, Stmt};

use crate::db::{function_id, BlazeDatabase, FnKey, SourceProgram};
use crate::ir::{CmpKind, FunctionNode, IrBuilder, IrOp, Signature, Type};

/// The lossless green tree of the whole file — parsed **once per revision**.
///
/// Everything downstream slices this tree instead of re-parsing, so the
/// per-edit analysis cost is one parse plus cheap per-function extraction,
/// independent of how many functions consume it. (`GreenNode` is an immutable,
/// `Send + Sync`, reference-counted tree; cloning is pointer-cheap.)
#[salsa::tracked(returns(clone))]
pub fn parse_program(db: &dyn BlazeDatabase, src: SourceProgram) -> rowan::GreenNode {
    db.record_exec("parse_program".into());
    blaze_parse::parse(src.text(db)).green()
}

/// The ordered list of top-level function names in the program.
///
/// Re-runs when the tree changes, but backdates whenever the *set and order*
/// of function names is unchanged, insulating consumers that only care about
/// program structure from body edits.
#[salsa::tracked(returns(clone))]
pub fn program_outline(db: &dyn BlazeDatabase, src: SourceProgram) -> Arc<Vec<String>> {
    db.record_exec("program_outline".into());
    let file = SourceFile::from_green(parse_program(db, src));
    let names = file.functions().filter_map(|f| f.name()).collect::<Vec<_>>();
    Arc::new(names)
}

/// The exact source text of function `key`, or empty if it is not defined.
///
/// This is the per-function firewall node: it re-executes on any edit (the
/// shared tree changed) but backdates unless *this* function's own bytes
/// changed — terminating the cascade for every untouched function.
#[salsa::tracked(returns(deref))]
pub fn function_text<'db>(
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
    key: FnKey<'db>,
) -> Arc<str> {
    let name = key.name(db);
    db.record_exec(format!("function_text({name})"));
    let file = SourceFile::from_green(parse_program(db, src));
    let text = file
        .functions()
        .find(|f| f.name().as_deref() == Some(name.as_str()))
        .map(|f| f.source_text())
        .unwrap_or_default();
    Arc::from(text.as_str())
}

/// The ABI signature of function `key`.
///
/// Depends only on [`function_text`] of the same function, so it is unaffected
/// by edits to any other function, and backdates when the signature is
/// unchanged — the property callers rely on.
#[salsa::tracked(returns(clone))]
pub fn function_signature<'db>(
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
    key: FnKey<'db>,
) -> Signature {
    let name = key.name(db);
    db.record_exec(format!("function_signature({name})"));
    let text = function_text(db, src, key);
    parse_single(text)
        .map(|f| Signature {
            params: f.param_decls().iter().map(|(_, ty)| lower_ty(*ty)).collect(),
            ret: lower_ty(f.return_type()),
        })
        .unwrap_or_default()
}

/// Map a surface [`blaze_parse::Ty`] to a DevIR [`Type`].
fn lower_ty(ty: blaze_parse::Ty) -> Type {
    match ty {
        blaze_parse::Ty::Int => Type::Int,
        blaze_parse::Ty::Float => Type::Float,
    }
}

/// The lowered DevIR node for function `key`.
///
/// Depends on this function's own [`function_text`] (its body) and, for each
/// callee, that callee's [`function_signature`] (its ABI) — deliberately *not*
/// the callee's body. That asymmetry is the incremental firewall.
#[salsa::tracked(returns(clone))]
pub fn lowered_dev_ir<'db>(
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
    key: FnKey<'db>,
) -> Arc<FunctionNode> {
    let name = key.name(db);
    db.record_exec(format!("lowered_dev_ir({name})"));

    let text = function_text(db, src, key);
    let Some(func) = parse_single(text) else {
        return Arc::new(FunctionNode::empty(function_id(db, name)));
    };
    let signature = function_signature(db, src, key);
    let node = Lowerer { db, src }.lower_function(&func, name, signature);
    Arc::new(node)
}

/// Parse a single-function fragment (the output of [`function_text`]) and return
/// its typed `Function`, if present.
fn parse_single(text: &str) -> Option<Function> {
    SourceFile::parse(text).functions().next()
}

/// Carries the database context needed to resolve cross-function ABI reads while
/// lowering one function's body.
struct Lowerer<'db> {
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
}

impl<'db> Lowerer<'db> {
    fn lower_function(&self, func: &Function, name: &str, signature: Signature) -> FunctionNode {
        let mut builder = IrBuilder::default();
        // Parameters occupy the first registers, in declaration order, each
        // allocated with its declared type.
        let mut env: HashMap<String, u32> = HashMap::new();
        for (pname, pty) in func.param_decls() {
            let reg = builder.alloc(lower_ty(pty));
            env.insert(pname, reg);
        }
        let ret_ty = signature.ret;

        let terminated = match func.body() {
            Some(block) => self.lower_block(&mut builder, &mut env, &block),
            None => false,
        };
        // A function that falls off the end returns a zero of its declared
        // return type (the C-subset has no `void`), keeping every DevIR body
        // properly terminated with an ABI-correct value.
        if !terminated {
            let zero = self.zero_of(&mut builder, ret_ty);
            builder.ret(zero);
        }

        builder.finish(function_id(self.db, name), signature)
    }

    /// A zero constant of the given type.
    fn zero_of(&self, b: &mut IrBuilder, ty: Type) -> u32 {
        match ty {
            Type::Int => b.load_int(0),
            Type::Float => b.load_float(0.0),
        }
    }

    /// Return `reg` if it already has type `ty`; otherwise materialize a
    /// register of type `ty` holding `reg`'s value. Only well-typed programs
    /// avoid coercion entirely; a mismatch here is a diagnostic-rejected program
    /// (the coercion just keeps lowering total and the backend panic-free — the
    /// bit reinterpretation the backend performs is never observed live).
    fn coerce(&self, b: &mut IrBuilder, reg: u32, ty: Type) -> u32 {
        if b.reg_type(reg) == ty {
            reg
        } else {
            let dst = b.alloc(ty);
            b.copy(dst, reg);
            dst
        }
    }

    /// Lower every statement of `block`. Returns `true` if the block
    /// *terminated* — every control path through it ended in `return` — in
    /// which case any trailing statements are dead and deliberately not
    /// emitted, so DevIR never contains unreachable blocks or unbound labels.
    fn lower_block(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, block: &Block) -> bool {
        for stmt in block.statements() {
            if self.lower_stmt(b, env, &stmt) {
                return true;
            }
        }
        false
    }

    /// Lower one statement; returns `true` if it terminates control flow.
    fn lower_stmt(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Let(let_stmt) => {
                let value = self.lower_opt(b, env, let_stmt.value());
                if let Some(name) = let_stmt.name() {
                    // Each named local owns a dedicated register (so later
                    // assignments across control-flow joins are plain `Copy`s)
                    // typed by its initializer.
                    let ty = b.reg_type(value);
                    let slot = b.alloc(ty);
                    b.copy(slot, value);
                    env.insert(name, slot);
                }
                false
            }
            Stmt::Assign(assign) => {
                let value = self.lower_opt(b, env, assign.value());
                if let Some(name) = assign.name() {
                    match env.get(&name) {
                        // Assign into the variable's existing, fixed-type slot.
                        Some(&slot) => {
                            let coerced = self.coerce(b, value, b.reg_type(slot));
                            b.copy(slot, coerced);
                        }
                        // Assignment to a never-declared name: allocate a slot of
                        // the value's type. (This is a diagnostic-rejected case.)
                        None => {
                            let ty = b.reg_type(value);
                            let slot = b.alloc(ty);
                            b.copy(slot, value);
                            env.insert(name, slot);
                        }
                    }
                }
                false
            }
            Stmt::Return(ret) => {
                let reg = self.lower_opt(b, env, ret.value());
                b.ret(reg);
                true
            }
            Stmt::If(if_stmt) => {
                let cond = self.lower_cond(b, env, if_stmt.condition());
                let l_then = b.fresh_label();
                let l_end = b.fresh_label();

                match if_stmt.else_arm() {
                    None => {
                        // No else: the false edge falls straight through to
                        // `l_end`, which is therefore always reachable.
                        b.branch(cond, l_then, l_end);
                        b.bind_label(l_then);
                        let t = match if_stmt.then_block() {
                            Some(then) => self.lower_block(b, env, &then),
                            None => false,
                        };
                        if !t {
                            b.jump(l_end);
                        }
                        b.bind_label(l_end);
                        false
                    }
                    Some(else_arm) => {
                        let l_else = b.fresh_label();
                        b.branch(cond, l_then, l_else);

                        b.bind_label(l_then);
                        let then_terminated = match if_stmt.then_block() {
                            Some(then) => self.lower_block(b, env, &then),
                            None => false,
                        };
                        if !then_terminated {
                            b.jump(l_end);
                        }

                        b.bind_label(l_else);
                        let else_terminated = match else_arm {
                            ElseArm::Block(block) => self.lower_block(b, env, &block),
                            ElseArm::If(nested) => {
                                self.lower_stmt(b, env, &Stmt::If(nested))
                            }
                        };
                        if !else_terminated {
                            b.jump(l_end);
                        }

                        // Bind the join point only if some path reaches it;
                        // otherwise the whole `if` terminates and the caller
                        // stops emitting dead code after it.
                        let terminated = then_terminated && else_terminated;
                        if !terminated {
                            b.bind_label(l_end);
                        }
                        terminated
                    }
                }
            }
            Stmt::While(while_stmt) => {
                let l_head = b.fresh_label();
                let l_body = b.fresh_label();
                let l_end = b.fresh_label();

                b.jump(l_head);
                b.bind_label(l_head);
                let cond = self.lower_cond(b, env, while_stmt.condition());
                b.branch(cond, l_body, l_end);

                b.bind_label(l_body);
                let body_terminated = match while_stmt.body() {
                    Some(body) => self.lower_block(b, env, &body),
                    None => false,
                };
                if !body_terminated {
                    b.jump(l_head);
                }

                // `l_end` is always reachable via the condition's false edge.
                b.bind_label(l_end);
                false
            }
            Stmt::Expr(expr_stmt) => {
                if let Some(expr) = expr_stmt.expr() {
                    // Evaluated for effects (calls); the result register is unused.
                    self.lower_expr(b, env, &expr);
                }
                false
            }
        }
    }

    /// Lower a condition expression, coercing it to `int` (the branch tests
    /// `cond != 0`). Well-typed conditions are already `int` comparisons.
    fn lower_cond(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, cond: Option<Expr>) -> u32 {
        let c = self.lower_opt(b, env, cond);
        self.coerce(b, c, Type::Int)
    }

    /// Lower an optional expression, materializing a `0` when absent so the IR
    /// stays total on malformed input.
    fn lower_opt(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, expr: Option<Expr>) -> u32 {
        match expr {
            Some(e) => self.lower_expr(b, env, &e),
            None => b.load_int(0),
        }
    }

    fn lower_expr(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, expr: &Expr) -> u32 {
        match expr {
            Expr::Literal(lit) => match lit.literal() {
                Some(Lit::Int(v)) => b.load_int(v),
                Some(Lit::Float(v)) => b.load_float(v),
                None => b.load_int(0),
            },
            Expr::NameRef(name_ref) => {
                let name = name_ref.text().unwrap_or_default();
                match env.get(&name) {
                    Some(&reg) => reg,
                    // An unresolved name lowers to a defined-zero value rather
                    // than aborting, keeping this query total over arbitrary
                    // input. `crate::diag::function_diagnostics` re-walks this
                    // same traversal and reports every case that would land
                    // here — that gate, not this fallback, is what a live
                    // reload actually relies on to reject bad input.
                    None => b.load_int(0),
                }
            }
            Expr::Bin(bin) => self.lower_bin(b, env, bin),
            Expr::Prefix(prefix) => {
                // Unary minus lowers as `0 - operand`, in the operand's type.
                let operand = self.lower_opt(b, env, prefix.operand());
                match b.reg_type(operand) {
                    Type::Float => {
                        let zero = b.load_float(0.0);
                        b.float_arith(IrOp::FSub, zero, operand)
                    }
                    Type::Int => {
                        let zero = b.load_int(0);
                        b.int_arith(IrOp::Sub, zero, operand)
                    }
                }
            }
            Expr::Call(call) => self.lower_call(b, env, call),
            Expr::Paren(paren) => self.lower_opt(b, env, paren.inner()),
        }
    }

    fn lower_bin(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, bin: &BinExpr) -> u32 {
        let (lhs, rhs) = bin.operands();
        let l = self.lower_opt(b, env, lhs);
        let r = self.lower_opt(b, env, rhs);
        // The operation is performed in `float` iff either operand is a float;
        // both operands are coerced to that common type. Well-typed programs
        // never actually coerce (their operands already match).
        let common = if b.reg_type(l) == Type::Float || b.reg_type(r) == Type::Float {
            Type::Float
        } else {
            Type::Int
        };
        let l = self.coerce(b, l, common);
        let r = self.coerce(b, r, common);

        let op = bin.op().unwrap_or(BinOp::Add);
        match (op, common) {
            (BinOp::Add, Type::Int) => b.int_arith(IrOp::Add, l, r),
            (BinOp::Sub, Type::Int) => b.int_arith(IrOp::Sub, l, r),
            (BinOp::Mul, Type::Int) => b.int_arith(IrOp::Mul, l, r),
            (BinOp::Div, Type::Int) => b.int_arith(IrOp::Div, l, r),
            (BinOp::Add, Type::Float) => b.float_arith(IrOp::FAdd, l, r),
            (BinOp::Sub, Type::Float) => b.float_arith(IrOp::FSub, l, r),
            (BinOp::Mul, Type::Float) => b.float_arith(IrOp::FMul, l, r),
            (BinOp::Div, Type::Float) => b.float_arith(IrOp::FDiv, l, r),
            (cmp, Type::Int) => b.cmp(cmp_kind(cmp), l, r),
            (cmp, Type::Float) => b.fcmp(cmp_kind(cmp), l, r),
        }
    }

    fn lower_call(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, call: &CallExpr) -> u32 {
        let callee_name = call.callee().unwrap_or_default();
        let callee_id = function_id(self.db, &callee_name);

        // THE FIREWALL EDGE: reading the callee's *signature* (not its body)
        // creates a salsa dependency that backdates whenever the callee's ABI is
        // unchanged, so a caller is insulated from edits to a callee's internals.
        // The signature also gives the argument and result *types* used below;
        // an arity/type mismatch is tolerated here (the lowerer stays total) and
        // reported by `crate::diag::function_diagnostics`, which the live reload
        // gates on.
        let callee_key = FnKey::new(self.db, callee_name);
        let callee_sig = function_signature(self.db, self.src, callee_key);

        let args: Vec<u32> = call
            .args()
            .iter()
            .enumerate()
            .map(|(i, arg)| {
                let reg = self.lower_expr(b, env, arg);
                // Coerce each argument to the callee's declared parameter type.
                match callee_sig.params.get(i) {
                    Some(&pty) => self.coerce(b, reg, pty),
                    None => reg,
                }
            })
            .collect();

        b.call(callee_id, args, callee_sig.ret)
    }
}

/// Map a surface comparison operator to a DevIR [`CmpKind`].
fn cmp_kind(op: BinOp) -> CmpKind {
    match op {
        BinOp::Lt => CmpKind::Lt,
        BinOp::Gt => CmpKind::Gt,
        BinOp::LtEq => CmpKind::Le,
        BinOp::GtEq => CmpKind::Ge,
        BinOp::EqEq => CmpKind::Eq,
        BinOp::NotEq => CmpKind::Ne,
        // Arithmetic ops are dispatched before this is reached.
        _ => CmpKind::Eq,
    }
}
