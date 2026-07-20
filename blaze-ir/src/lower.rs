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

use blaze_parse::ast::{BinOp, Block, CallExpr, ElseArm, Expr, Function, SourceFile, Stmt};

use crate::db::{BlazeDatabase, FnKey, SourceProgram};
use crate::ir::{function_id_of, CmpKind, FunctionNode, IrBuilder, Signature, Type};

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
        .map(|f| Signature { params: vec![Type::Int; f.params().len()], ret: Type::Int })
        .unwrap_or_default()
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
        return Arc::new(FunctionNode::empty(function_id_of(name)));
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
        // Parameters occupy the first registers, in declaration order.
        let mut env: HashMap<String, u32> = HashMap::new();
        for param in func.params() {
            let reg = builder.fresh();
            env.insert(param, reg);
        }

        let terminated = match func.body() {
            Some(block) => self.lower_block(&mut builder, &mut env, &block),
            None => false,
        };
        // A function that falls off the end returns 0 (the C-subset has no
        // `void`), keeping every DevIR body properly terminated.
        if !terminated {
            let zero = builder.load_int(0);
            builder.ret(zero);
        }

        builder.finish(function_id_of(name), signature)
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
                    // Each named local owns a dedicated register so later
                    // assignments (possibly across control-flow joins) are
                    // plain `Copy`s into a stable slot.
                    let slot = b.fresh();
                    b.copy(slot, value);
                    env.insert(name, slot);
                }
                false
            }
            Stmt::Assign(assign) => {
                let value = self.lower_opt(b, env, assign.value());
                if let Some(name) = assign.name() {
                    let slot = *env.entry(name).or_insert_with(|| b.fresh());
                    b.copy(slot, value);
                }
                false
            }
            Stmt::Return(ret) => {
                let reg = self.lower_opt(b, env, ret.value());
                b.ret(reg);
                true
            }
            Stmt::If(if_stmt) => {
                let cond = self.lower_opt(b, env, if_stmt.condition());
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
                let cond = self.lower_opt(b, env, while_stmt.condition());
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
            Expr::Literal(lit) => b.load_int(lit.value().unwrap_or(0)),
            Expr::NameRef(name_ref) => {
                let name = name_ref.text().unwrap_or_default();
                match env.get(&name) {
                    Some(&reg) => reg,
                    // An unresolved name lowers to a defined-zero value rather
                    // than aborting; a richer frontend would raise a diagnostic.
                    None => b.load_int(0),
                }
            }
            Expr::Bin(bin) => {
                let (lhs, rhs) = bin.operands();
                let l = self.lower_opt(b, env, lhs);
                let r = self.lower_opt(b, env, rhs);
                match bin.op() {
                    Some(BinOp::Add) | None => b.add(l, r),
                    Some(BinOp::Sub) => b.sub(l, r),
                    Some(BinOp::Mul) => b.mul(l, r),
                    Some(BinOp::Div) => b.div(l, r),
                    Some(BinOp::Lt) => b.cmp(CmpKind::Lt, l, r),
                    Some(BinOp::Gt) => b.cmp(CmpKind::Gt, l, r),
                    Some(BinOp::LtEq) => b.cmp(CmpKind::Le, l, r),
                    Some(BinOp::GtEq) => b.cmp(CmpKind::Ge, l, r),
                    Some(BinOp::EqEq) => b.cmp(CmpKind::Eq, l, r),
                    Some(BinOp::NotEq) => b.cmp(CmpKind::Ne, l, r),
                }
            }
            Expr::Prefix(prefix) => {
                // Unary minus lowers as `0 - operand`.
                let zero = b.load_int(0);
                let operand = self.lower_opt(b, env, prefix.operand());
                b.sub(zero, operand)
            }
            Expr::Call(call) => self.lower_call(b, env, call),
            Expr::Paren(paren) => self.lower_opt(b, env, paren.inner()),
        }
    }

    fn lower_call(&self, b: &mut IrBuilder, env: &mut HashMap<String, u32>, call: &CallExpr) -> u32 {
        let callee_name = call.callee().unwrap_or_default();
        let callee_id = function_id_of(&callee_name);

        // THE FIREWALL EDGE: reading the callee's *signature* (not its body)
        // creates a salsa dependency that backdates whenever the callee's ABI is
        // unchanged, so a caller is insulated from edits to a callee's internals.
        let callee_key = FnKey::new(self.db, callee_name);
        let callee_sig = function_signature(self.db, self.src, callee_key);
        // Reading the callee's signature — never its body — is what records the
        // firewall dependency edge. The arity is informational for now; a
        // mismatch is tolerated because the C-subset has no diagnostics layer.
        let _callee_arity = callee_sig.arity();

        let args: Vec<u32> = call
            .args()
            .iter()
            .map(|arg| self.lower_expr(b, env, arg))
            .collect();

        b.call(callee_id, args)
    }
}
