//! Honest throughput comparison: the same risk rule, served by Blaze's native
//! `FuncHandle` fast path versus an embedded AST interpreter ([`rhai`]).
//!
//! ```sh
//! cargo run -p blaze-jit --release --example bench_vs_interpreter
//! ```
//!
//! This is the apples-to-apples number behind "swap functions, don't flip
//! booleans": a rules/scoring service that embeds a scripting language pays the
//! interpreter's per-call cost on every request. Blaze compiles the same rule to
//! native code and dispatches it through one atomic load and an indirect call —
//! while keeping the *hot-swappable* property the interpreter has and a compiled
//! `dylib` does not. Same logic, same inputs, both warmed; we publish the
//! multiple we actually measure on this machine.

use std::time::Instant;

use blaze_jit::LiveRuntime;

/// The rule, in Blaze. A handful of branches and some arithmetic — representative
/// of a real per-request scoring/decision function.
const BLAZE_RULE: &str = "\
int score(int amount, int velocity, int age) {
    int r = 0;
    if (amount > 100000) {
        r = r + 45;
    } else if (amount > 10000) {
        r = r + 20;
    }
    r = r + velocity * 5;
    if (age < 30) {
        r = r + 25;
    }
    return r;
}
";

/// The *same* rule, in rhai. Identical control flow and arithmetic, so the only
/// thing being measured is dispatch + evaluation strategy.
const RHAI_RULE: &str = "\
fn score(amount, velocity, age) {
    let r = 0;
    if amount > 100000 {
        r += 45;
    } else if amount > 10000 {
        r += 20;
    }
    r += velocity * 5;
    if age < 30 {
        r += 25;
    }
    r
}
";

/// Inputs cycle through this table so both engines take every branch, and
/// neither can hoist a constant result out of the loop.
const INPUTS: [(i64, i64, i64); 4] =
    [(5_000, 3, 40), (50_000, 4, 20), (150_000, 8, 10), (250_000, 11, 60)];

fn main() {
    // Cross-check: the two implementations must agree on every input before we
    // dare compare their speed. A benchmark of two things computing different
    // answers is worthless.
    let blaze = LiveRuntime::new(BLAZE_RULE).expect("blaze rule compiles");
    let engine = rhai::Engine::new();
    let ast = engine.compile(RHAI_RULE).expect("rhai rule compiles");
    for &(a, v, g) in &INPUTS {
        let b = blaze.call("score", &[a, v, g]).expect("blaze call");
        let r: i64 = engine
            .call_fn(&mut rhai::Scope::new(), &ast, "score", (a, v, g))
            .expect("rhai call");
        assert_eq!(b, r, "the two rules must agree on score({a}, {v}, {g})");
    }

    let blaze_per_s = bench_blaze(&blaze);
    let rhai_per_s = bench_rhai(&engine, &ast);

    let m = 1_000_000.0;
    println!("Blaze native FuncHandle vs. an embedded interpreter — same risk rule\n");
    println!("  Blaze  call_handle (native JIT)   {:>10.2} M calls/s/thread", blaze_per_s / m);
    println!("  rhai   call_fn     (AST interp)   {:>10.3} M calls/s/thread", rhai_per_s / m);
    println!();
    println!(
        "  Blaze is \x1b[1m{:.0}×\x1b[0m the throughput of the interpreter — same rule, same inputs,",
        blaze_per_s / rhai_per_s
    );
    println!("  and still hot-swappable: an edit re-lands as native code in microseconds.");
}

/// Warm iterations before timing, so both engines are past any first-call cost.
const WARMUP: u64 = 100_000;
/// Timed iterations for Blaze (cheap per call, so a big count for a stable rate).
const BLAZE_ITERS: u64 = 20_000_000;
/// Timed iterations for rhai (far dearer per call; fewer keep the run short).
const RHAI_ITERS: u64 = 1_000_000;

fn bench_blaze(rt: &LiveRuntime) -> f64 {
    // The hot path a real deployment uses: resolve once, then call lock-free.
    let mut handle = rt.handle("score").expect("resolve score");
    for i in 0..WARMUP {
        let (a, v, g) = INPUTS[(i % 4) as usize];
        std::hint::black_box(rt.call_handle(&mut handle, &[a, v, g]).unwrap());
    }
    let t = Instant::now();
    let mut acc = 0i64;
    for i in 0..BLAZE_ITERS {
        let (a, v, g) = INPUTS[(i % 4) as usize];
        acc = acc.wrapping_add(rt.call_handle(&mut handle, &[a, v, g]).unwrap());
    }
    std::hint::black_box(acc);
    BLAZE_ITERS as f64 / t.elapsed().as_secs_f64()
}

fn bench_rhai(engine: &rhai::Engine, ast: &rhai::AST) -> f64 {
    let mut scope = rhai::Scope::new();
    for i in 0..WARMUP {
        let (a, v, g) = INPUTS[(i % 4) as usize];
        let r: i64 = engine.call_fn(&mut scope, ast, "score", (a, v, g)).unwrap();
        std::hint::black_box(r);
    }
    let t = Instant::now();
    let mut acc = 0i64;
    for i in 0..RHAI_ITERS {
        let (a, v, g) = INPUTS[(i % 4) as usize];
        let r: i64 = engine.call_fn(&mut scope, ast, "score", (a, v, g)).unwrap();
        acc = acc.wrapping_add(r);
    }
    std::hint::black_box(acc);
    RHAI_ITERS as f64 / t.elapsed().as_secs_f64()
}
