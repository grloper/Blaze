//! Machine-checked proof of Blaze's incremental **firewall** invariant.
//!
//! From the architecture spec:
//! > If a function body's internal statement graph is mutated but its ABI
//! > signature remains invariant, the invalidation cascade terminates strictly
//! > at the function boundary. Callers remain O(1) unaffected.
//!
//! These tests drive the `salsa` query graph directly and inspect the execution
//! trace (which query *bodies* actually re-ran) plus `Arc` identity (whether a
//! memoized value was reused) to verify that property — and its converse, that
//! an ABI change *does* cascade to callers.
//!
//! Note on ergonomics: interned [`FnKey`] handles borrow the database, so they
//! cannot be held across a `set_text(&mut db)` edit. The [`ir_of`] helper mints
//! a fresh key by name for each query — interning is idempotent, so the same
//! name always resolves to the same memoized results.

use std::sync::Arc;

use blaze_ir::db::{function_id, BlazeDatabaseImpl, ExecTrace, FnKey};
use blaze_ir::lower::{lowered_dev_ir, program_outline};
use blaze_ir::{FunctionNode, IrOp, SourceProgram};
use salsa::Setter;

/// `add` is a leaf; `main` is its sole caller. Laid out so editing `add`'s body
/// leaves `main`'s own byte span untouched.
const V1: &str = "\
int add(int a, int b) {
    return a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// `add`'s body becomes `a + a + b` — internal logic mutated, ABI invariant.
/// `main`'s text is byte-identical to V1.
const V2_BODY_ONLY: &str = "\
int add(int a, int b) {
    return a + a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// `add` gains a third parameter — an ABI change that must cascade to `main`.
const V3_SIGNATURE: &str = "\
int add(int a, int b, int c) {
    return a + b;
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

/// Lower the function named `name`, returning an owned DevIR node. The interned
/// key lives only for the duration of the call, so the caller stays free to
/// mutate the database afterward.
fn ir_of(db: &BlazeDatabaseImpl, src: SourceProgram, name: &str) -> Arc<FunctionNode> {
    let key = FnKey::new(db, name.to_string());
    lowered_dev_ir(db, src, key)
}

#[test]
fn outline_and_lowering_are_correct() {
    let (db, src) = setup(V1);
    assert_eq!(&*program_outline(&db, src), &["add".to_string(), "main".to_string()]);

    // `int add(int a, int b) { return a + b; }`
    //   a -> r0, b -> r1, then `a + b` -> Add(r2, r0, r1), return r2.
    let add = ir_of(&db, src, "add");
    assert_eq!(add.signature.arity(), 2);
    assert_eq!(add.body, vec![IrOp::Add(2, 0, 1), IrOp::Return(2)]);
    assert!(add.dependencies.is_empty(), "add is a leaf function");

    // `main` calls `add`, so it must record `add` as a dependency — identified
    // by its interned id, which is exactly what `add`'s own node carries.
    let main = ir_of(&db, src, "main");
    assert_eq!(main.dependencies, vec![function_id(&db, "add")]);
    assert_eq!(main.dependencies, vec![add.id], "the dep id must equal the callee's own id");
}

#[test]
fn reevaluation_without_edits_is_a_pure_cache_hit() {
    let (db, src) = setup(V1);
    let trace: ExecTrace = db.enable_tracing();
    let _ = ir_of(&db, src, "add");
    let _ = ir_of(&db, src, "main");
    let _ = trace.take(); // discard the cold-cache execution log

    // Ask again with no intervening edit: nothing should re-execute.
    let _ = ir_of(&db, src, "add");
    let _ = ir_of(&db, src, "main");
    assert!(
        trace.take().is_empty(),
        "a re-query with no edits must be served entirely from memoized values",
    );
}

#[test]
fn body_edit_does_not_invalidate_callers() {
    let (mut db, src) = setup(V1);
    let trace = db.enable_tracing();
    let add_before = ir_of(&db, src, "add");
    let main_before = ir_of(&db, src, "main");
    let _ = trace.take();

    // Mutate `add`'s internal logic; its signature is invariant.
    src.set_text(&mut db).to(V2_BODY_ONLY.to_string());

    let add_after = ir_of(&db, src, "add");
    let main_after = ir_of(&db, src, "main");
    let log = trace.take();

    // The callee re-lowered...
    assert!(
        log.iter().any(|l| l == "lowered_dev_ir(add)"),
        "editing add's body must re-lower add; log = {log:?}",
    );
    assert!(!Arc::ptr_eq(&add_before, &add_after), "add's DevIR must be a fresh value");
    assert_ne!(add_before.body, add_after.body, "add's body must actually change");

    // ...but the caller did NOT. This is the firewall.
    assert!(
        !log.iter().any(|l| l == "lowered_dev_ir(main)"),
        "the caller `main` must hit the memo cache; log = {log:?}",
    );
    assert!(
        !log.iter().any(|l| l == "function_signature(main)"),
        "the caller's signature must not be re-derived; log = {log:?}",
    );
    assert!(
        Arc::ptr_eq(&main_before, &main_after),
        "main's DevIR must be the very same memoized Arc, byte-for-byte reused",
    );
}

#[test]
fn signature_edit_does_cascade_to_callers() {
    let (mut db, src) = setup(V1);
    let trace = db.enable_tracing();
    let _ = ir_of(&db, src, "add");
    let main_before = ir_of(&db, src, "main");
    let _ = trace.take();

    // Change `add`'s ABI: 2 params -> 3.
    src.set_text(&mut db).to(V3_SIGNATURE.to_string());

    let _ = ir_of(&db, src, "add");
    let main_after = ir_of(&db, src, "main");
    let log = trace.take();

    assert!(
        log.iter().any(|l| l == "function_signature(add)"),
        "add's signature must be recomputed; log = {log:?}",
    );
    assert!(
        log.iter().any(|l| l == "lowered_dev_ir(main)"),
        "an ABI change to a callee MUST cascade and re-lower the caller; log = {log:?}",
    );
    assert!(
        !Arc::ptr_eq(&main_before, &main_after),
        "main's DevIR must be recomputed after the callee's ABI changed",
    );
}

#[test]
fn editing_a_caller_leaves_the_callee_cached() {
    // The dual direction: touching `main` must never disturb the leaf `add`.
    let (mut db, src) = setup(V1);
    let trace = db.enable_tracing();
    let add_before = ir_of(&db, src, "add");
    let _ = ir_of(&db, src, "main");
    let _ = trace.take();

    // Rewrite only `main`'s body (its call to `add` is preserved).
    let edited = V1.replace("int x = add(1, 2);\n    return x;", "return add(1, 2);");
    src.set_text(&mut db).to(edited);

    let add_after = ir_of(&db, src, "add");
    let _ = ir_of(&db, src, "main");
    let log = trace.take();

    assert!(
        !log.iter().any(|l| l == "lowered_dev_ir(add)"),
        "editing the caller must not re-lower the callee; log = {log:?}",
    );
    assert!(Arc::ptr_eq(&add_before, &add_after), "add's DevIR must be reused verbatim");
}
