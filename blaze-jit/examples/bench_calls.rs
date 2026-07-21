//! Call-throughput benchmark: the `FuncHandle` fast path vs. the string-keyed
//! `call(name, ..)` path, single-threaded and multi-threaded.
//!
//! ```sh
//! cargo run -p blaze-jit --release --example bench_calls
//! ```
//!
//! The point of `FuncHandle` (H5): a rules/scoring service invokes the same few
//! functions millions of times per second. `call(name, ..)` pays a `RwLock`
//! read plus a string hash + map lookup every time; `call_handle` resolves once
//! and then costs an arity check, one atomic load (double-checked), and the
//! indirect call — no lock, no lookup.

use std::sync::Arc;
use std::thread;
use std::time::Instant;

use blaze_jit::LiveRuntime;

// A trivial leaf function: `score(a, b) = a * 3 + b`. Leaf, so it pays no
// fuel/among-call overhead — this measures dispatch cost, not compute.
const SRC: &str = "\
int score(int a, int b) {
    return a * 3 + b;
}
";

const WARMUP: u64 = 200_000;
const ITERS: u64 = 20_000_000;

fn bench_named(runtime: &LiveRuntime) -> f64 {
    // Warm up.
    for i in 0..WARMUP {
        std::hint::black_box(runtime.call("score", &[i as i64, 1]).unwrap());
    }
    let t = Instant::now();
    let mut acc = 0i64;
    for i in 0..ITERS {
        acc = acc.wrapping_add(runtime.call("score", &[i as i64, 1]).unwrap());
    }
    std::hint::black_box(acc);
    ITERS as f64 / t.elapsed().as_secs_f64()
}

fn bench_handle(runtime: &LiveRuntime) -> f64 {
    let mut h = runtime.handle("score").unwrap();
    for i in 0..WARMUP {
        std::hint::black_box(runtime.call_handle(&mut h, &[i as i64, 1]).unwrap());
    }
    let t = Instant::now();
    let mut acc = 0i64;
    for i in 0..ITERS {
        acc = acc.wrapping_add(runtime.call_handle(&mut h, &[i as i64, 1]).unwrap());
    }
    std::hint::black_box(acc);
    ITERS as f64 / t.elapsed().as_secs_f64()
}

fn bench_handle_threaded(runtime: &Arc<LiveRuntime>, threads: usize) -> f64 {
    let t = Instant::now();
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let runtime = runtime.clone();
            thread::spawn(move || {
                // Each thread resolves its own handle once, then hammers.
                let mut h = runtime.handle("score").unwrap();
                let mut acc = 0i64;
                for i in 0..ITERS {
                    acc = acc.wrapping_add(runtime.call_handle(&mut h, &[i as i64, 1]).unwrap());
                }
                std::hint::black_box(acc);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    (ITERS * threads as u64) as f64 / t.elapsed().as_secs_f64()
}

fn main() {
    let runtime = Arc::new(LiveRuntime::new(SRC).expect("compile"));
    // Fuel would otherwise cost a per-call budget read; leave it on (default) —
    // this is the honest hot-path number a real deployment sees.

    let named = bench_named(&runtime);
    let handle = bench_handle(&runtime);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let threaded = bench_handle_threaded(&runtime, threads);

    let m = 1_000_000.0;
    println!("Blaze call throughput — trivial leaf `score(a, b)`\n");
    println!("  call(name)      single-thread   {:>8.2} M calls/s", named / m);
    println!("  call_handle     single-thread   {:>8.2} M calls/s", handle / m);
    println!("  call_handle     {threads}-thread aggregate {:>8.2} M calls/s", threaded / m);
    println!();
    println!("  handle speedup over named lookup: {:>4.1}x", handle / named);
    println!("  per-thread (threaded / {threads}):       {:>8.2} M calls/s", threaded / threads as f64 / m);

    // The H5 target: > 5M calls/s/thread on the fast path.
    assert!(
        handle >= 5.0 * m,
        "FuncHandle fast path {:.2} M calls/s is below the 5M/s/thread target",
        handle / m,
    );
    println!("\n  target met: FuncHandle >= 5M calls/s/thread \u{2713}");
}
