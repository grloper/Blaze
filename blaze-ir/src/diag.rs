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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use blaze_parse::ast::{AstNode, Block, ElseArm, Expr, Function, IfStmt, Stmt};
use blaze_parse::SyntaxNode;

use crate::db::{BlazeDatabase, FnKey, HostFunctions, SourceProgram};
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

    let known = known_callables(db, src, host);
    let mut checker = Checker { known: &known, diags: Vec::new() };
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

/// name → declared arity, over the union of Blaze-defined functions and
/// host-registered native functions — the one namespace call sites resolve
/// against.
fn known_callables(
    db: &dyn BlazeDatabase,
    src: SourceProgram,
    host: HostFunctions,
) -> HashMap<String, usize> {
    let mut known = HashMap::new();
    for name in program_outline(db, src).iter() {
        let key = FnKey::new(db, name.clone());
        known.insert(name.clone(), function_signature(db, src, key).arity());
    }
    for (name, arity) in host.arities(db) {
        known.entry(name.clone()).or_insert(*arity);
    }
    known
}

/// Walks a function's AST with exactly the statement/expression order
/// `lower::Lowerer` uses, tracking which names are declared-so-far instead of
/// building IR. Where the lowerer would fall back to a default, this reports a
/// diagnostic instead.
struct Checker<'a> {
    known: &'a HashMap<String, usize>,
    diags: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn check_function(&mut self, func: &Function) {
        let mut declared: HashSet<String> = func.params().into_iter().collect();
        if let Some(block) = func.body() {
            self.check_block(&block, &mut declared);
        }
    }

    fn check_block(&mut self, block: &Block, declared: &mut HashSet<String>) {
        for stmt in block.statements() {
            self.check_stmt(&stmt, declared);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt, declared: &mut HashSet<String>) {
        match stmt {
            Stmt::Let(s) => {
                if let Some(v) = s.value() {
                    self.check_expr(&v, declared);
                }
                if let Some(n) = s.name() {
                    declared.insert(n);
                }
            }
            Stmt::Assign(s) => {
                if let Some(v) = s.value() {
                    self.check_expr(&v, declared);
                }
                if let Some(n) = s.name() {
                    if !declared.contains(&n) {
                        self.report(
                            format!("assignment to undeclared variable `{n}` (use `int {n} = ...;` to declare it)"),
                            s.syntax(),
                        );
                    }
                    // The lowerer auto-declares on first assignment (matches
                    // its `env.entry(..).or_insert_with(..)`); mirror that so a
                    // single typo reports once, not on every later use.
                    declared.insert(n);
                }
            }
            Stmt::Return(s) => {
                if let Some(v) = s.value() {
                    self.check_expr(&v, declared);
                }
            }
            Stmt::If(s) => self.check_if(s, declared),
            Stmt::While(s) => {
                if let Some(c) = s.condition() {
                    self.check_expr(&c, declared);
                }
                if let Some(b) = s.body() {
                    self.check_block(&b, declared);
                }
            }
            Stmt::Expr(s) => {
                if let Some(e) = s.expr() {
                    self.check_expr(&e, declared);
                }
            }
        }
    }

    fn check_if(&mut self, s: &IfStmt, declared: &mut HashSet<String>) {
        if let Some(c) = s.condition() {
            self.check_expr(&c, declared);
        }
        if let Some(then) = s.then_block() {
            self.check_block(&then, declared);
        }
        match s.else_arm() {
            Some(ElseArm::Block(b)) => self.check_block(&b, declared),
            Some(ElseArm::If(nested)) => self.check_if(&nested, declared),
            None => {}
        }
    }

    fn check_expr(&mut self, expr: &Expr, declared: &HashSet<String>) {
        match expr {
            Expr::Literal(_) => {}
            Expr::NameRef(n) => {
                if let Some(name) = n.text() {
                    if !declared.contains(&name) {
                        self.report(format!("use of undefined variable `{name}`"), n.syntax());
                    }
                }
            }
            Expr::Bin(b) => {
                let (l, r) = b.operands();
                if let Some(l) = l {
                    self.check_expr(&l, declared);
                }
                if let Some(r) = r {
                    self.check_expr(&r, declared);
                }
            }
            Expr::Prefix(p) => {
                if let Some(o) = p.operand() {
                    self.check_expr(&o, declared);
                }
            }
            Expr::Paren(p) => {
                if let Some(i) = p.inner() {
                    self.check_expr(&i, declared);
                }
            }
            Expr::Call(c) => {
                for a in c.args() {
                    self.check_expr(&a, declared);
                }
                let callee = c.callee().unwrap_or_default();
                match self.known.get(callee.as_str()) {
                    None => self.report(
                        format!("call to undefined function `{callee}`"),
                        c.syntax(),
                    ),
                    Some(&expected) => {
                        let got = c.args().len();
                        if expected != got {
                            self.report(
                                format!("`{callee}` expects {expected} argument(s), found {got}"),
                                c.syntax(),
                            );
                        }
                    }
                }
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
}
