//! End-to-end tests for the Cranelift backend: real JIT execution, the codegen
//! firewall, and incremental recompilation ("hot-swap") after a source edit.

use blaze_ir::db::{BlazeDatabaseImpl, FnKey, SourceProgram};
use blaze_jit::{compiled_machine_code, jit_program};
use salsa::Setter;

const V1: &str = "\
int add(int a, int b) {
    return a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// `add`'s body becomes `a + a + b` (ABI invariant). `add(1, 2)` now yields 4,
/// so `main()` returns 4 instead of 3 once recompiled.
const V2_BODY_ONLY: &str = "\
int add(int a, int b) {
    return a + a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

fn setup(source: &str) -> (BlazeDatabaseImpl, SourceProgram) {
    let db = BlazeDatabaseImpl::default();
    let src = SourceProgram::new(&db, source.to_string());
    (db, src)
}

#[test]
fn compiles_and_executes_a_program() {
    let (db, src) = setup(V1);
    let engine = jit_program(&db, src).expect("JIT compilation should succeed");

    // `main` computes `add(1, 2)` = 3.
    assert_eq!(engine.call("main", &[]), Some(3));
    // `add` is directly invocable with the C ABI.
    assert_eq!(engine.call("add", &[10, 20]), Some(30));
    assert_eq!(engine.call("add", &[-5, 5]), Some(0));
}

#[test]
fn machine_code_is_emitted_and_memoized() {
    let (db, src) = setup(V1);
    let add = FnKey::new(&db, "add".to_string());
    let compiled = compiled_machine_code(&db, src, add);
    assert!(compiled.code_len() > 0, "add must produce real machine code");
    assert_eq!(compiled.signature.arity(), 2);
}

#[test]
fn codegen_inherits_the_incremental_firewall() {
    let (mut db, src) = setup(V1);
    let trace = db.enable_tracing();

    // Warm the codegen cache for both functions.
    let _ = compiled_machine_code(&db, src, FnKey::new(&db, "add".to_string()));
    let _ = compiled_machine_code(&db, src, FnKey::new(&db, "main".to_string()));
    let _ = trace.take();

    // Edit only `add`'s body.
    src.set_text(&mut db).to(V2_BODY_ONLY.to_string());

    let _ = compiled_machine_code(&db, src, FnKey::new(&db, "add".to_string()));
    let _ = compiled_machine_code(&db, src, FnKey::new(&db, "main".to_string()));
    let log = trace.take();

    assert!(
        log.iter().any(|l| l == "compiled_machine_code(add)"),
        "add's machine code must be recompiled; log = {log:?}",
    );
    assert!(
        !log.iter().any(|l| l == "compiled_machine_code(main)"),
        "the caller's machine code must be served from cache; log = {log:?}",
    );
}

#[test]
fn hot_recompile_reflects_source_edits_in_execution() {
    let (mut db, src) = setup(V1);

    let before = jit_program(&db, src).unwrap();
    assert_eq!(before.call("main", &[]), Some(3), "add(1, 2) == 3 before the edit");

    // Mutate `add`'s internals; only `add` is re-lowered (main is a memo hit),
    // then the program is re-linked and the updated logic executes.
    src.set_text(&mut db).to(V2_BODY_ONLY.to_string());

    let after = jit_program(&db, src).unwrap();
    assert_eq!(after.call("main", &[]), Some(4), "add(1, 2) == 1 + 1 + 2 == 4 after the edit");
}
