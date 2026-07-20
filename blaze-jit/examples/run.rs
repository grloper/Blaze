//! End-to-end demo: source text → salsa graph → DevIR → Cranelift → execution,
//! then an incremental edit that hot-recompiles and changes the result.
//!
//! Run with: `cargo run -p blaze-jit --example run`

use blaze_ir::db::{BlazeDatabaseImpl, SourceProgram};
use blaze_jit::jit_program;
use salsa::Setter;

const PROGRAM: &str = "\
int square_sum(int a, int b) {
    int s = a + b;
    return s + s;
}

int main() {
    return square_sum(3, 4);
}
";

fn main() {
    let mut db = BlazeDatabaseImpl::default();
    let src = SourceProgram::new(&db, PROGRAM.to_string());

    let engine = jit_program(&db, src).expect("compile");
    let before = engine.call("main", &[]).expect("main is arity 0");
    println!("main() = {before}   // square_sum(3, 4) = (3+4) + (3+4) = 14");
    assert_eq!(before, 14);

    // Incrementally edit only `square_sum`'s body. `main` is re-lowered from the
    // memo cache; only the changed function passes through codegen again.
    let edited = PROGRAM.replace("return s + s;", "return s + s + a;");
    src.set_text(&mut db).to(edited);

    let engine = jit_program(&db, src).expect("recompile");
    let after = engine.call("main", &[]).expect("main is arity 0");
    println!("main() = {after}   // after edit: 14 + a(=3) = 17");
    assert_eq!(after, 17);

    println!("hot recompile changed behavior without touching main's source.");
}
