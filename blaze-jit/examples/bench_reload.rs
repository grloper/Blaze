//! Edit-to-committed-pointer latency for each edit class, against a program of
//! `N_FUNCS` functions — versus the only alternative a restart-based workflow
//! has: recompile everything and lose all state.
//!
//! ```sh
//! cargo run -p blaze-jit --release --example bench_reload
//! ```

use std::time::Instant;

use blaze_jit::{EditClass, LiveRuntime};

const N_FUNCS: usize = 40;
const BODY_EDIT_ROUNDS: usize = 30;

/// A call chain `f0 -> f1 -> ... -> f{n-1}` plus `main -> f0`, so every
/// function except the last has a dependent — the worst case for a reloader
/// that cannot bound blast radius, the common case for one that can.
fn program(n: usize, leaf_constant: i64) -> String {
    let mut src = String::new();
    for i in 0..n {
        if i + 1 < n {
            src.push_str(&format!(
                "int f{i}(int x) {{\n    return f{next}(x) + {i};\n}}\n\n",
                next = i + 1
            ));
        } else {
            src.push_str(&format!(
                "int f{i}(int x) {{\n    return x * 2 + {leaf_constant};\n}}\n\n"
            ));
        }
    }
    src.push_str("int main() {\n    return f0(1);\n}\n");
    src
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    xs[xs.len() / 2]
}

fn main() {
    println!("Blaze reload latency — {N_FUNCS}-function program, call-chain topology\n");

    // Full cold build (what a restart pays in compilation alone, ignoring
    // process startup, linking of the host binary, and the loss of all state).
    let t0 = Instant::now();
    let runtime = LiveRuntime::new(&program(N_FUNCS, 0)).expect("initial compile");
    let cold = t0.elapsed();
    assert!(runtime.call("main", &[]).is_ok());

    // Body-only edits to the chain's *leaf* — the deepest function, which
    // every other function transitively calls. A blast-radius-blind system
    // rebuilds the world; the firewall proves the radius is one function.
    let mut swap_latencies = Vec::new();
    for round in 0..BODY_EDIT_ROUNDS {
        let report = runtime
            .reload(&program(N_FUNCS, 1 + round as i64))
            .expect("reload");
        assert_eq!(report.class, EditClass::SafeSwap);
        assert_eq!(report.changed.len(), 1, "firewall must bound the radius to the leaf");
        swap_latencies.push(report.latency.as_secs_f64() * 1e3);
    }

    // NoEffect edits: a comment toggled inside the leaf.
    let mut noop_latencies = Vec::new();
    for round in 0..10 {
        let marker = format!("int f0(int x) {{\n    // pass {round}\n    return f1(x) + 0;\n}}");
        let src = program(N_FUNCS, BODY_EDIT_ROUNDS as i64).replacen(
            "int f0(int x) {\n    return f1(x) + 0;\n}",
            &marker,
            1,
        );
        let report = runtime.reload(&src).expect("reload");
        assert_eq!(report.class, EditClass::NoEffect);
        noop_latencies.push(report.latency.as_secs_f64() * 1e3);
    }

    // One ABI edit at the leaf: the graph pulls its caller in; radius = 2.
    let mut src = program(N_FUNCS, BODY_EDIT_ROUNDS as i64);
    let last = N_FUNCS - 1;
    let prev = N_FUNCS - 2;
    src = src
        .replace(
            &format!("int f{last}(int x) {{"),
            &format!("int f{last}(int x, int y) {{"),
        )
        .replace(&format!("return f{last}(x) + {prev};"), &format!("return f{last}(x, 7) + {prev};"));
    let report = runtime.reload(&src).expect("reload");
    assert_eq!(report.class, EditClass::Relink);
    let relink_ms = report.latency.as_secs_f64() * 1e3;
    let relink_radius = report.changed.len();

    let cold_ms = cold.as_secs_f64() * 1e3;
    let swap_ms = median(swap_latencies);
    let noop_ms = median(noop_latencies);

    println!("  full cold compile (≈ restart, minus startup & state loss)  {cold_ms:>9.3} ms");
    println!("  body edit  → SafeSwap  (radius 1, median of {BODY_EDIT_ROUNDS})            {swap_ms:>9.3} ms");
    println!("  comment    → NoEffect  (radius 0, median of 10)            {noop_ms:>9.3} ms");
    println!("  ABI edit   → Relink    (radius {relink_radius})                        {relink_ms:>9.3} ms");
    println!();
    println!(
        "  body-edit speedup over restart-compile: {:>5.1}x  (plus: state survives)",
        cold_ms / swap_ms
    );
}
