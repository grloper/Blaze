//! What does the canary cost the live request path? (P3's claim: "live p99 never
//! moves more than the mirroring cost.")
//!
//! ```sh
//! cargo run -p blaze-jit --release --example bench_canary
//! ```
//!
//! `call_canary` is the request entry point you leave in place whether or not a
//! canary is running. This measures its overhead across the spectrum: idle (no
//! candidate), shadowing at a realistic 1% sample rate, and shadowing every
//! single call. The takeaway is that an idle canary is free, and an active one
//! costs exactly the fraction of calls you choose to mirror.

use std::time::Instant;

use blaze_jit::{CanaryPolicy, LiveRuntime};

const RULE: &str = "\
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

/// A behavior-preserving candidate (so a never-abort canary shadows forever).
const CANDIDATE: &str = "\
int score(int amount, int velocity, int age) {
    int r = velocity * 5;
    if (amount > 100000) {
        r = r + 45;
    } else if (amount > 10000) {
        r = r + 20;
    }
    if (age < 30) {
        r = r + 25;
    }
    return r;
}
";

const INPUTS: [(i64, i64, i64); 4] =
    [(5_000, 3, 40), (50_000, 4, 20), (150_000, 8, 10), (250_000, 11, 60)];

const WARMUP: u64 = 100_000;
const ITERS: u64 = 5_000_000;

/// A never-abort policy at the given sample rate, so the canary keeps shadowing
/// for the whole measurement.
fn policy(sample_every: u64) -> CanaryPolicy {
    CanaryPolicy { sample_every, max_divergences: u64::MAX, ..Default::default() }
}

/// Time `ITERS` calls of `f` and return calls/second.
fn bench(mut f: impl FnMut(usize) -> i64) -> f64 {
    for i in 0..WARMUP {
        std::hint::black_box(f((i % 4) as usize));
    }
    let t = Instant::now();
    let mut acc = 0i64;
    for i in 0..ITERS {
        acc = acc.wrapping_add(f((i % 4) as usize));
    }
    std::hint::black_box(acc);
    ITERS as f64 / t.elapsed().as_secs_f64()
}

fn main() {
    let rt = LiveRuntime::new(RULE).expect("rule compiles");

    // 1. Plain `call` — no canary machinery at all (the reference).
    let plain = bench(|i| {
        let (a, v, g) = INPUTS[i];
        rt.call("score", &[a, v, g]).unwrap()
    });

    // 2. `call_canary` with no candidate — the flag load and nothing else.
    let idle = bench(|i| {
        let (a, v, g) = INPUTS[i];
        rt.call_canary("score", &[a, v, g]).unwrap()
    });

    // 3. `call_canary` shadowing a candidate at 1% (a realistic canary rate).
    rt.canary(CANDIDATE, policy(100)).expect("start 1% canary");
    let sampled_1pct = bench(|i| {
        let (a, v, g) = INPUTS[i];
        rt.call_canary("score", &[a, v, g]).unwrap()
    });
    rt.abort_canary();

    // 4. `call_canary` shadowing every call — the upper bound on mirroring cost.
    rt.canary(CANDIDATE, policy(1)).expect("start 100% canary");
    let sampled_full = bench(|i| {
        let (a, v, g) = INPUTS[i];
        rt.call_canary("score", &[a, v, g]).unwrap()
    });
    rt.abort_canary();

    let m = 1_000_000.0;
    // Absolute ns/call is the honest lens here: percentages look large only
    // because the base call is a trivial ~50 ns leaf. The deltas are a few ns.
    let ns = |x: f64| 1e9 / x;
    let plain_ns = ns(plain);
    println!("Canary overhead on the live request path — `call_canary`\n");
    println!("  plain call (no canary)                {:>7.2} M/s   {:>6.1} ns/call   (reference)", plain / m, plain_ns);
    println!("  call_canary, idle (no candidate)      {:>7.2} M/s   {:>6.1} ns/call   {:+.1} ns", idle / m, ns(idle), ns(idle) - plain_ns);
    println!("  call_canary, shadowing  1% of calls   {:>7.2} M/s   {:>6.1} ns/call   {:+.1} ns", sampled_1pct / m, ns(sampled_1pct), ns(sampled_1pct) - plain_ns);
    println!("  call_canary, shadowing 100% of calls  {:>7.2} M/s   {:>6.1} ns/call   {:+.1} ns", sampled_full / m, ns(sampled_full), ns(sampled_full) - plain_ns);
    println!();
    println!("  An idle canary adds a single atomic load. An active one adds a lock-free");
    println!("  counter increment per call (a few ns — noise for any real handler, visible");
    println!("  here only because the call itself is trivial), plus one shadow execution on");
    println!("  the sampled fraction. The sampler is lock-free, so mirroring never serializes");
    println!("  concurrent traffic — and the caller always gets the live answer regardless.");
}
