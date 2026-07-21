//! Proofs for per-function metrics (P2).
//!
//! Metrics are the observability half of a live runtime: which live function is
//! hot, how slow, how often it trips a guard. These tests pin the guarantees:
//!
//! | Claim                                                        | Test |
//! |--------------------------------------------------------------|------|
//! | Metrics are off by default — nothing is counted              | [`metrics_are_off_by_default`] |
//! | When on, per-function call counts are exact and isolated     | [`enabled_metrics_count_calls_per_function`] |
//! | Latency accrues per recorded call                            | [`latency_is_recorded`] |
//! | A trapped call is counted and marked a fault                 | [`faults_are_counted_separately`] |
//! | The handle fast path is metered too                          | [`handle_calls_are_metered`] |
//! | Counts are exact under concurrent callers (lock-free)        | [`counts_are_exact_under_concurrent_callers`] |
//! | Metrics survive a hot swap (the slot is stable)              | [`metrics_survive_a_body_swap`] |
//! | `reset_metrics` zeroes every counter                         | [`reset_zeroes_counters`] |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use blaze_jit::{CallError, LiveRuntime};

const PROG: &str = "\
int add(int a, int b) {
    return a + b;
}

int busy(int n) {
    int i = 0;
    int acc = 0;
    while (i < n) {
        acc = acc + i;
        i = i + 1;
    }
    return acc;
}
";

#[test]
fn metrics_are_off_by_default() {
    let rt = LiveRuntime::new(PROG).expect("compile");
    for _ in 0..5 {
        rt.call("add", &[1, 2]).expect("call");
    }
    // Nothing is recorded until collection is explicitly enabled.
    let m = rt.metrics("add").expect("known function");
    assert_eq!(m.calls, 0, "no calls counted while metrics are disabled");
    assert_eq!(m.total_nanos, 0);
    // An unknown function has no metrics.
    assert_eq!(rt.metrics("nope"), None);
}

#[test]
fn enabled_metrics_count_calls_per_function() {
    let rt = LiveRuntime::new(PROG).expect("compile");
    rt.set_metrics_enabled(true);
    for _ in 0..10 {
        rt.call("add", &[1, 2]).expect("call");
    }
    let m = rt.metrics("add").expect("known function");
    assert_eq!(m.calls, 10);
    assert_eq!(m.faults, 0);
    // `busy` was never called, so its counters stay zero — metrics are per
    // function, not global.
    assert_eq!(rt.metrics("busy").expect("known").calls, 0);
}

#[test]
fn latency_is_recorded() {
    let rt = LiveRuntime::new(PROG).expect("compile");
    rt.set_metrics_enabled(true);
    for _ in 0..50 {
        rt.call("busy", &[2000]).expect("call");
    }
    let m = rt.metrics("busy").expect("known");
    assert_eq!(m.calls, 50);
    assert!(m.total_nanos > 0, "wall-clock latency must accrue for real work");
    assert!(m.mean_nanos() > 0, "mean latency must be positive");
}

#[test]
fn faults_are_counted_separately() {
    let src = "\
int spin(int n) {
    return spin(n) + 1;
}

int ok(int a) {
    return a;
}
";
    let rt = LiveRuntime::new(src).expect("compile");
    rt.set_metrics_enabled(true);

    // A runaway call that trips the depth guard is still a call that *ran*, so
    // it is counted — and additionally marked a fault.
    assert_eq!(rt.call("spin", &[1]), Err(CallError::ResourceExhausted));
    let sm = rt.metrics("spin").expect("known");
    assert_eq!(sm.calls, 1);
    assert_eq!(sm.faults, 1);

    // A healthy call is counted but never a fault.
    rt.call("ok", &[5]).expect("call");
    let om = rt.metrics("ok").expect("known");
    assert_eq!(om.calls, 1);
    assert_eq!(om.faults, 0);
}

#[test]
fn handle_calls_are_metered() {
    let rt = LiveRuntime::new(PROG).expect("compile");
    rt.set_metrics_enabled(true);
    let mut add = rt.handle("add").expect("resolve");
    for _ in 0..7 {
        rt.call_handle(&mut add, &[1, 2]).expect("fast call");
    }
    assert_eq!(rt.metrics("add").expect("known").calls, 7, "the lock-free path is metered too");
}

#[test]
fn counts_are_exact_under_concurrent_callers() {
    let rt = Arc::new(LiveRuntime::new(PROG).expect("compile"));
    rt.set_metrics_enabled(true);

    const THREADS: u64 = 4;
    const PER: u64 = 25_000;
    let start = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..THREADS)
        .map(|_| {
            let rt = rt.clone();
            let start = start.clone();
            thread::spawn(move || {
                while !start.load(Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
                for _ in 0..PER {
                    rt.call("add", &[1, 2]).expect("call");
                }
            })
        })
        .collect();
    start.store(true, Ordering::Relaxed);
    for w in workers {
        w.join().expect("worker");
    }

    // Every increment lands: relaxed atomics can't lose or tear a count, even
    // with four threads hammering the same slot simultaneously.
    assert_eq!(rt.metrics("add").expect("known").calls, THREADS * PER);
}

#[test]
fn metrics_survive_a_body_swap() {
    let rt = LiveRuntime::new("int add(int a, int b) { return a + b; }\n").expect("compile");
    rt.set_metrics_enabled(true);
    rt.call("add", &[1, 2]).expect("call");

    // A body-only edit keeps the function's slot, so its counters carry over.
    rt.reload("int add(int a, int b) { return a + a + b; }\n").expect("reload");
    rt.call("add", &[1, 2]).expect("call");

    assert_eq!(
        rt.metrics("add").expect("known").calls,
        2,
        "the slot is stable across a hot swap, so metrics accumulate through it",
    );
}

#[test]
fn reset_zeroes_counters() {
    let rt = LiveRuntime::new(PROG).expect("compile");
    rt.set_metrics_enabled(true);
    rt.call("add", &[1, 2]).expect("call");
    assert_eq!(rt.metrics("add").expect("known").calls, 1);

    rt.reset_metrics();
    let m = rt.metrics("add").expect("known");
    assert_eq!(m.calls, 0);
    assert_eq!(m.total_nanos, 0);
    assert_eq!(m.faults, 0);
}
