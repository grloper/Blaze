//! Static diagnostics: defects the query graph can prove *before* a reload is
//! ever allowed to touch a live process.
//!
//! Without this module, `blaze-ir`'s lowering pass is deliberately permissive —
//! an unresolved name lowers to a defined `0` instead of aborting, so a single
//! `salsa` query graph stays total over arbitrary (including malformed) input.
//! That permissiveness is the right property for a *query*, and the wrong one
//! for a *reload*: silently swapping in code that reads garbage zeros where the
//! author meant a variable is exactly the "safe but wrong" failure mode a live
//! system must reject, not tolerate.
//!
//! [`function_diagnostics`] closes that gap by re-walking each function's AST
//! with the *same traversal shape* `lower::Lowerer` uses — same statement
//! order, same flat, declare-before-use scoping — but instead of emitting IR it
//! reports every point where the lowerer would have silently substituted a
//! default: an unresolved variable read, an assignment to a name never
//! declared, a call to a function nobody defines, or a call whose argument
//! count doesn't match the callee's declared arity. Syntax errors from the
//! function's own parse are folded in too. [`program_diagnostics`] aggregates
//! this across the whole program; `blaze-jit`'s live runtime treats *any*
//! non-empty result as a hard gate — the edit is rejected and the previous,
//! known-good generation keeps serving traffic untouched.

use std::collections::HashMap;
use std::sync::Arc;

use blaze_parse::ast::{
    AstNode, BinExpr, BinOp, Block, CallExpr, ElseArm, Expr, Function, IfStmt, Stmt,
};
use blaze_parse::SyntaxNode;

use crate::db::{BlazeDatabase, FnKey, HostFunctions, SourceProgram};
use crate::ir::{Signature, Type};
use crate::lower::{function_signature, function_text, program_outline};

/// A single statically-detectable defect in one function's source.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub message: String,
    /// Byte offset of the defect within the *function's own* source text (as
    /// returned by [`crate::lower::function_text`]), not the whole file.
    pub offset: usize,
}

/// Every diagnostic in function `key`, syntax and semantic alike.
///
/// Depends on this function's own text (syntax errors), the whole program's
/// outline and every function's signature (to resolve callees), and the
/// host's registered functions — so it re-runs whenever any of those change,
/// but that cost is cheap name/arity bookkeeping, never full recompilation.
#[salsa::tracked(returns(clone))]
pub fn function_diagnostics<'db>(
    db: &'db dyn BlazeDatabase,
    src: SourceProgram,
    key: FnKey<'db>,
    host: HostFunctions,
) -> Arc<Vec<Diagnostic>> {
    let name = key.name(db);
    db.record_exec(format!("function_diagnostics({name})"));

    let text = function_text(db, src, key);
    let parse = blaze_parse::parse(text);
    let mut diags: Vec<Diagnostic> = parse
        .errors()
        .iter()
        .map(|e| Diagnostic { message: e.message.clone(), offset: e.offset })
        .collect();

    // Re-root the same parse (no second parse) as the typed AST.
    let Some(file) = blaze_parse::SourceFile::cast(parse.syntax()) else {
        return Arc::new(diags);
    };
    let Some(func) = file.functions().next() else {
        return Arc::new(diags);
    };

    let sigs = signatures(db, src, host);
    let mut checker = Checker {
        sigs: &sigs,
        vars: HashMap::new(),
        ret: Type::Int,
        diags: Vec::new(),
    };
    checker.check_function(&func);
    diags.extend(checker.diags);

    Arc::new(diags)
}

/// Every diagnostic across the whole program, attributed by function name.
///
/// An empty result is the precondition `blaze-jit`'s live runtime requires
/// before it will commit any generation to a running process.
#[salsa::tracked(returns(clone))]
pub fn program_diagnostics(
    db: &dyn BlazeDatabase,
    src: SourceProgram,
    host: HostFunctions,
) -> Arc<Vec<(String, Diagnostic)>> {
    db.record_exec("program_diagnostics".into());
    let outline = program_outline(db, src);
    let mut all = Vec::new();
    for name in outline.iter() {
        let key = FnKey::new(db, name.clone());
        for d in function_diagnostics(db, src, key, host).iter() {
            all.push((name.clone(), d.clone()));
        }
    }
    Arc::new(all)
}

/// Format a diagnostic list for a human (used when construction itself must
/// fail — there is no "hold last-good" to fall back to on the first load).
pub fn format_diagnostics(diags: &[(String, Diagnostic)]) -> String {
    let mut out = format!("rejected: {} diagnostic(s)", diags.len());
    for (func, d) in diags {
        out.push_str(&format!("\n  {func} (byte {}): {}", d.offset, d.message));
    }
    out
}

/// name → full ABI signature, over the union of Blaze-defined functions and
/// host-registered native functions — the one namespace call sites resolve
/// against. Host functions are `(int × arity) -> int` (the C ABI the embedder
/// registers them with).
fn signatures(
    db: &dyn BlazeDatabase,
    src: SourceProgram,
    host: HostFunctions,
) -> HashMap<String, Signature> {
    let mut sigs = HashMap::new();
    for name in program_outline(db, src).iter() {
        let key = FnKey::new(db, name.clone());
        sigs.insert(name.clone(), function_signature(db, src, key));
    }
    for (name, arity) in host.arities(db) {
        sigs.entry(name.clone())
            .or_insert_with(|| Signature { params: vec![Type::Int; *arity], ret: Type::Int });
    }
    sigs
}

/// Map a surface [`blaze_parse::Ty`] to a DevIR [`Type`] (kept local so this
/// gate does not couple to `lower`'s internals).
fn ty_of(ty: blaze_parse::Ty) -> Type {
    match ty {
        blaze_parse::Ty::Int => Type::Int,
        blaze_parse::Ty::Float => Type::Float,
    }
}

/// The surface spelling of a type, for diagnostics.
fn type_name(t: Type) -> &'static str {
    match t {
        Type::Int => "int",
        Type::Float => "float",
    }
}

/// The surface spelling of a binary operator, for diagnostics.
fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::LtEq => "<=",
        BinOp::GtEq => ">=",
        BinOp::EqEq => "==",
        BinOp::NotEq => "!=",
    }
}

/// Walks a function's AST with exactly the statement/expression order
/// `lower::Lowerer` uses, tracking each name's *type* (not merely whether it is
/// declared). Where the lowerer would fall back to a default — or silently
/// bit-reinterpret one type as another via [`crate::lower`]'s `coerce` — this
/// reports a diagnostic instead.
///
/// This is what makes the language's implicit-coercion path unreachable in a
/// live program: `int` and `float` are distinct machine representations, so a
/// mismatch at an assignment, return, argument, condition, or arithmetic
/// boundary would compile to a bit reinterpretation that reads garbage. The
/// live runtime gates on this being empty, so such a program is rejected before
/// any generation is committed — never observed at runtime.
struct Checker<'a> {
    sigs: &'a HashMap<String, Signature>,
    /// name → type of every variable declared so far, in the same flat,
    /// order-of-traversal scope the lowerer uses.
    vars: HashMap<String, Type>,
    /// The declared return type of the function currently being checked.
    ret: Type,
    diags: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn check_function(&mut self, func: &Function) {
        for (name, ty) in func.param_decls() {
            self.vars.insert(name, ty_of(ty));
        }
        self.ret = ty_of(func.return_type());
        if let Some(block) = func.body() {
            self.check_block(&block);
        }
    }

    fn check_block(&mut self, block: &Block) {
        for stmt in block.statements() {
            self.check_stmt(&stmt);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(s) => {
                let value_ty = s.value().and_then(|v| self.type_of(&v));
                let declared = s.ty().map(ty_of);
                if let Some(name) = s.name() {
                    if let (Some(d), Some(v)) = (declared, value_ty) {
                        if d != v {
                            self.report(
                                format!(
                                    "type mismatch: `{name}` is declared {} but its initializer is {}",
                                    type_name(d),
                                    type_name(v)
                                ),
                                s.syntax(),
                            );
                        }
                    }
                    // The declared type is the variable's contract for the rest
                    // of the function; fall back to the initializer's type only
                    // when no type was written (malformed input).
                    self.vars.insert(name, declared.or(value_ty).unwrap_or(Type::Int));
                }
            }
            Stmt::Assign(s) => {
                let value_ty = s.value().and_then(|v| self.type_of(&v));
                if let Some(name) = s.name() {
                    match self.vars.get(&name).copied() {
                        Some(var_ty) => {
                            if let Some(v) = value_ty {
                                if v != var_ty {
                                    self.report(
                                        format!(
                                            "type mismatch: cannot assign {} to `{name}` of type {}",
                                            type_name(v),
                                            type_name(var_ty)
                                        ),
                                        s.syntax(),
                                    );
                                }
                            }
                        }
                        None => {
                            self.report(
                                format!("assignment to undeclared variable `{name}` (use `int {name} = ...;` to declare it)"),
                                s.syntax(),
                            );
                            // The lowerer auto-declares on first assignment with
                            // the value's type; mirror that so a single typo
                            // reports once, not on every later use.
                            self.vars.insert(name, value_ty.unwrap_or(Type::Int));
                        }
                    }
                }
            }
            Stmt::Return(s) => {
                if let Some(v) = s.value() {
                    if let Some(vt) = self.type_of(&v) {
                        if vt != self.ret {
                            self.report(
                                format!(
                                    "type mismatch: returns {} but the function's return type is {}",
                                    type_name(vt),
                                    type_name(self.ret)
                                ),
                                s.syntax(),
                            );
                        }
                    }
                }
            }
            Stmt::If(s) => self.check_if(s),
            Stmt::While(s) => {
                self.check_condition(s.condition());
                if let Some(b) = s.body() {
                    self.check_block(&b);
                }
            }
            Stmt::Expr(s) => {
                if let Some(e) = s.expr() {
                    self.type_of(&e);
                }
            }
        }
    }

    fn check_if(&mut self, s: &IfStmt) {
        self.check_condition(s.condition());
        if let Some(then) = s.then_block() {
            self.check_block(&then);
        }
        match s.else_arm() {
            Some(ElseArm::Block(b)) => self.check_block(&b),
            Some(ElseArm::If(nested)) => self.check_if(&nested),
            None => {}
        }
    }

    /// A condition is tested `!= 0`, so it must be an `int` (comparisons already
    /// yield `int`). A bare `float` condition would compile to a bit-reinterpret
    /// of its pattern — rejected here.
    fn check_condition(&mut self, cond: Option<Expr>) {
        if let Some(c) = cond {
            if let Some(t) = self.type_of(&c) {
                if t != Type::Int {
                    self.report(
                        format!("type mismatch: condition must be int, found {}", type_name(t)),
                        c.syntax(),
                    );
                }
            }
        }
    }

    /// Compute an expression's type, reporting any defect it contains
    /// (undefined name/callee, arity mismatch, or a type mismatch at an
    /// operator or call boundary). Returns `None` when a subexpression is
    /// unresolvable, which suppresses cascading type errors above it.
    fn type_of(&mut self, expr: &Expr) -> Option<Type> {
        match expr {
            Expr::Literal(lit) => match lit.literal() {
                Some(blaze_parse::ast::Lit::Int(_)) => Some(Type::Int),
                Some(blaze_parse::ast::Lit::Float(_)) => Some(Type::Float),
                None => None,
            },
            Expr::NameRef(n) => {
                let name = n.text()?;
                match self.vars.get(&name) {
                    Some(&t) => Some(t),
                    None => {
                        self.report(format!("use of undefined variable `{name}`"), n.syntax());
                        None
                    }
                }
            }
            Expr::Bin(b) => self.type_of_bin(b),
            Expr::Prefix(p) => p.operand().and_then(|o| self.type_of(&o)),
            Expr::Paren(p) => p.inner().and_then(|i| self.type_of(&i)),
            Expr::Call(c) => self.type_of_call(c),
        }
    }

    fn type_of_bin(&mut self, b: &BinExpr) -> Option<Type> {
        let (l, r) = b.operands();
        let lt = l.as_ref().and_then(|e| self.type_of(e));
        let rt = r.as_ref().and_then(|e| self.type_of(e));
        let op = b.op().unwrap_or(BinOp::Add);
        let is_cmp = matches!(
            op,
            BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq | BinOp::EqEq | BinOp::NotEq
        );
        // Both operands must share a type: the lowerer would otherwise coerce
        // (bit-reinterpret) one side. Only report when both types are known, so
        // an already-reported undefined operand doesn't cascade.
        if let (Some(lt), Some(rt)) = (lt, rt) {
            if lt != rt {
                self.report(
                    format!(
                        "type mismatch: `{}` cannot combine {} and {}",
                        op_symbol(op),
                        type_name(lt),
                        type_name(rt)
                    ),
                    b.syntax(),
                );
            }
        }
        // A comparison always yields `int`; arithmetic yields the operand type.
        if is_cmp {
            Some(Type::Int)
        } else {
            lt.or(rt)
        }
    }

    fn type_of_call(&mut self, c: &CallExpr) -> Option<Type> {
        // Resolve argument types first (reporting any nested defects).
        let arg_exprs = c.args();
        let arg_types: Vec<Option<Type>> =
            arg_exprs.iter().map(|a| self.type_of(a)).collect();
        let callee = c.callee().unwrap_or_default();
        match self.sigs.get(callee.as_str()) {
            None => {
                self.report(format!("call to undefined function `{callee}`"), c.syntax());
                None
            }
            Some(sig) => {
                let expected = sig.params.len();
                let got = arg_exprs.len();
                if expected != got {
                    self.report(
                        format!("`{callee}` expects {expected} argument(s), found {got}"),
                        c.syntax(),
                    );
                } else {
                    for (i, (at, pty)) in arg_types.iter().zip(sig.params.iter()).enumerate() {
                        if let Some(at) = at {
                            if at != pty {
                                self.report(
                                    format!(
                                        "type mismatch: argument {} to `{callee}` is {} but expects {}",
                                        i + 1,
                                        type_name(*at),
                                        type_name(*pty)
                                    ),
                                    c.syntax(),
                                );
                            }
                        }
                    }
                }
                Some(sig.ret)
            }
        }
    }

    fn report(&mut self, message: String, node: &SyntaxNode) {
        let offset: u32 = node.text_range().start().into();
        self.diags.push(Diagnostic { message, offset: offset as usize });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BlazeDatabaseImpl;

    fn messages(src: &str) -> Vec<String> {
        let db = BlazeDatabaseImpl::default();
        let source = SourceProgram::new(&db, src.to_string());
        let host = HostFunctions::new(&db, Default::default());
        program_diagnostics(&db, source, host)
            .iter()
            .map(|(_, d)| d.message.clone())
            .collect()
    }

    #[test]
    fn clean_program_has_no_diagnostics() {
        assert!(messages("int add(int a, int b) { return a + b; }\nint main() { return add(1, 2); }").is_empty());
    }

    #[test]
    fn catches_syntax_errors() {
        let msgs = messages("int main() { return 1 }\n");
        assert!(!msgs.is_empty());
    }

    #[test]
    fn catches_undefined_variable() {
        let msgs = messages("int main() { return x; }\n");
        assert!(msgs.iter().any(|m| m.contains("undefined variable `x`")), "{msgs:?}");
    }

    #[test]
    fn catches_undefined_callee() {
        let msgs = messages("int main() { return ghost(1); }\n");
        assert!(msgs.iter().any(|m| m.contains("undefined function `ghost`")), "{msgs:?}");
    }

    #[test]
    fn catches_arity_mismatch_between_functions() {
        let msgs = messages("int add(int a, int b) { return a + b; }\nint main() { return add(1); }");
        assert!(msgs.iter().any(|m| m.contains("expects 2 argument")), "{msgs:?}");
    }

    #[test]
    fn catches_assignment_to_undeclared_variable() {
        let msgs = messages("int main() { y = 5; return y; }\n");
        assert!(msgs.iter().any(|m| m.contains("undeclared variable `y`")), "{msgs:?}");
    }

    #[test]
    fn variables_bound_in_one_if_branch_are_visible_after_matching_the_lowerer() {
        // This mirrors `lower::Lowerer`'s flat, order-of-traversal scoping
        // exactly (see module docs): both branches are walked in sequence, so
        // a name declared in either becomes visible to code that follows.
        let msgs = messages("int main() { if (1) { int y = 1; } return y; }\n");
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn forward_reference_to_a_later_let_is_rejected() {
        let msgs = messages("int main() { int x = y; int y = 1; return x; }\n");
        assert!(msgs.iter().any(|m| m.contains("undefined variable `y`")), "{msgs:?}");
    }

    #[test]
    fn host_functions_are_known_callables() {
        let db = BlazeDatabaseImpl::default();
        let source = SourceProgram::new(&db, "int main() { return triple(2); }".to_string());
        let mut arities = std::collections::BTreeMap::new();
        arities.insert("triple".to_string(), 1);
        let host = HostFunctions::new(&db, arities);
        assert!(program_diagnostics(&db, source, host).is_empty());
    }

    #[test]
    fn well_typed_float_program_has_no_diagnostics() {
        let msgs = messages(
            "float scale(float x) { float y = x * 2.5; return y; }\nfloat main() { return scale(1.5); }",
        );
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn mixed_arithmetic_is_rejected() {
        // `1 + 2.5` would bit-reinterpret the integer `1` as an f64 — garbage.
        let msgs = messages("float main() { return 1 + 2.5; }\n");
        assert!(
            msgs.iter().any(|m| m.contains("cannot combine int and float")),
            "{msgs:?}"
        );
    }

    #[test]
    fn returning_the_wrong_type_is_rejected() {
        let msgs = messages("int main() { return 1.5; }\n");
        assert!(
            msgs.iter().any(|m| m.contains("returns float")
                && m.contains("return type is int")),
            "{msgs:?}"
        );
    }

    #[test]
    fn assigning_the_wrong_type_is_rejected() {
        let msgs = messages("int main() { int x = 0; x = 1.5; return x; }\n");
        assert!(
            msgs.iter().any(|m| m.contains("cannot assign float to `x` of type int")),
            "{msgs:?}"
        );
    }

    #[test]
    fn let_initializer_type_must_match_declaration() {
        let msgs = messages("int main() { float x = 3; return 0; }\n");
        assert!(
            msgs.iter().any(|m| m.contains("`x` is declared float")
                && m.contains("initializer is int")),
            "{msgs:?}"
        );
    }

    #[test]
    fn float_condition_is_rejected() {
        // A bare float condition would compile to a bit-reinterpret + `!= 0`.
        let msgs = messages("int main() { float x = 1.5; if (x) { return 1; } return 0; }\n");
        assert!(
            msgs.iter().any(|m| m.contains("condition must be int, found float")),
            "{msgs:?}"
        );
    }

    #[test]
    fn float_comparison_is_a_valid_condition() {
        // A comparison yields `int`, so `x > 0.0` is a well-typed condition.
        let msgs =
            messages("int main() { float x = 1.5; if (x > 0.0) { return 1; } return 0; }\n");
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn passing_the_wrong_argument_type_is_rejected() {
        let msgs = messages(
            "int twice(int n) { return n + n; }\nfloat main() { return twice(1.5); }",
        );
        assert!(
            msgs.iter().any(|m| m.contains("argument 1 to `twice` is float but expects int")),
            "{msgs:?}"
        );
    }
}
