//! Proofs for the in-process canary (P3).
//!
//! A canary shadows a candidate program against the live one: a sampled
//! fraction of calls run through both, and the candidate's result is compared
//! to the live answer — which is the *only* answer the caller ever sees. That
//! last clause is the whole soundness story, so it is tested hardest.
//!
//! | Claim                                                        | Test |
//! |--------------------------------------------------------------|------|
//! | The caller always gets the live answer, never the candidate  | [`the_caller_never_sees_the_candidate`] |
//! | …even under concurrent traffic from many threads             | [`the_shield_holds_under_concurrent_traffic`] |
//! | A wrong candidate auto-aborts on divergence                  | [`a_diverging_candidate_auto_aborts`] |
//! | A faulting candidate is a divergence and auto-aborts         | [`a_faulting_candidate_auto_aborts`] |
//! | A too-slow candidate auto-aborts on latency                  | [`a_slow_candidate_auto_aborts_on_latency`] |
//! | A matching candidate stays healthy and can be promoted       | [`a_matching_candidate_promotes_through_the_swap_protocol`] |
//! | Promotion is seamless under concurrent traffic               | [`promote_is_seamless_under_concurrent_traffic`] |
//! | An auto-aborted candidate cannot be promoted                 | [`an_aborted_candidate_cannot_be_promoted`] |
//! | A defective candidate never starts a canary                  | [`a_defective_candidate_is_rejected`] |
//! | Sampling is a deterministic 1-in-N                            | [`sampling_is_one_in_n`] |
//! | With no canary, `call_canary` is just `call`                 | [`call_canary_without_a_canary_is_just_call`] |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blaze_jit::{CanaryPolicy, CanaryVerdict, EditClass, LiveRuntime};

/// A policy that never auto-aborts, so a canary keeps shadowing indefinitely —
/// the strongest setting for proving the candidate's answer can never leak.
fn never_abort() -> CanaryPolicy {
    CanaryPolicy { sample_every: 1, max_divergences: u64::MAX, ..Default::default() }
}

#[test]
fn the_caller_never_sees_the_candidate() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    // A candidate that returns a *different* (wrong) answer, kept alive by a
    // never-abort policy so it shadows every single call.
    rt.canary("int score(int x) { return x + 1; }\n", never_abort()).expect("start canary");

    for _ in 0..1000 {
        // score(5) is 10 live, 6 in the candidate. The caller must always get 10.
        assert_eq!(rt.call_canary("score", &[5]), Ok(10), "the live answer, never the candidate's");
    }

    let st = rt.canary_status().expect("canary active");
    assert_eq!(st.samples, 1000);
    assert_eq!(st.divergences, 1000, "every sampled call diverged");
    assert_eq!(st.verdict, CanaryVerdict::Running, "never-abort policy keeps it shadowing");
    // The live program is untouched.
    assert_eq!(rt.call("score", &[5]), Ok(10));
}

#[test]
fn the_shield_holds_under_concurrent_traffic() {
    let rt = Arc::new(LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile"));
    rt.canary("int score(int x) { return x + 1; }\n", never_abort()).expect("start canary");

    const THREADS: usize = 4;
    const PER: usize = 2000;
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
                    // Under a storm of mirrored calls, not one may return the
                    // candidate's wrong answer.
                    assert_eq!(rt.call_canary("score", &[5]), Ok(10), "candidate answer leaked");
                }
            })
        })
        .collect();
    start.store(true, Ordering::Relaxed);
    for w in workers {
        w.join().expect("worker");
    }

    let st = rt.canary_status().expect("canary active");
    assert_eq!(st.samples, (THREADS * PER) as u64, "every call was mirrored, none lost");
    assert_eq!(rt.call("score", &[5]), Ok(10), "the live program is untouched");
}

#[test]
fn a_diverging_candidate_auto_aborts() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    // Default policy: abort on the first divergence (a strict shield).
    rt.canary("int score(int x) { return x + 1; }\n", CanaryPolicy::default()).expect("start");

    // The first mirrored call diverges (10 vs 6) and trips the shield.
    assert_eq!(rt.call_canary("score", &[5]), Ok(10));
    let st = rt.canary_status().expect("active");
    assert_eq!(st.verdict, CanaryVerdict::AbortedOnDivergence);
    assert_eq!(st.divergences, 1);

    // Once aborted, later calls stop being mirrored, and the live answer still
    // holds.
    assert_eq!(rt.call_canary("score", &[7]), Ok(14));
    assert_eq!(rt.canary_status().unwrap().samples, 1, "no further mirroring after abort");
}

#[test]
fn a_faulting_candidate_auto_aborts() {
    let rt = LiveRuntime::new("int f(int n) { return n; }\n").expect("compile");
    // The candidate recurses forever — it faults where the live version returns.
    rt.canary("int f(int n) { return f(n) + 1; }\n", CanaryPolicy::default()).expect("start");

    assert_eq!(rt.call_canary("f", &[5]), Ok(5), "the caller gets the healthy live result");
    let st = rt.canary_status().expect("active");
    // A candidate fault (Err vs Ok) is both a fault and a divergence.
    assert_eq!(st.candidate_faults, 1);
    assert_eq!(st.divergences, 1);
    assert_eq!(st.verdict, CanaryVerdict::AbortedOnDivergence);
}

#[test]
fn a_slow_candidate_auto_aborts_on_latency() {
    let rt = LiveRuntime::new("int f(int n) { return n; }\n").expect("compile");
    // Same result as live, but far slower (a big loop). Divergence is disabled
    // so only the latency guard can trip.
    let policy = CanaryPolicy {
        sample_every: 1,
        max_divergences: u64::MAX,
        min_samples_for_latency: 5,
        max_latency_ratio: 2.0,
    };
    let slow = "int f(int n) { int i = 0; while (i < 200000) { i = i + 1; } return n; }\n";
    rt.canary(slow, policy).expect("start");

    // Drive enough samples to pass the latency threshold; the candidate is
    // orders of magnitude slower than a bare return, so the ratio trips.
    for _ in 0..20 {
        assert_eq!(rt.call_canary("f", &[9]), Ok(9));
        if rt.canary_status().unwrap().verdict == CanaryVerdict::AbortedOnLatency {
            break;
        }
    }
    let st = rt.canary_status().expect("active");
    assert_eq!(st.verdict, CanaryVerdict::AbortedOnLatency, "status: {st:?}");
    assert_eq!(st.divergences, 0, "results matched — only latency failed");
}

#[test]
fn a_matching_candidate_promotes_through_the_swap_protocol() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    // `x * 2` is equivalent to `x + x` — a clean refactor. It lowers to
    // different IR, so promoting it is a real (Safe) swap.
    rt.canary("int score(int x) { return x * 2; }\n", CanaryPolicy::default()).expect("start");

    for x in 0..25 {
        assert_eq!(rt.call_canary("score", &[x]), Ok(x + x));
    }
    let st = rt.canary_status().expect("active");
    assert_eq!(st.divergences, 0, "an equivalent candidate never diverges");
    assert_eq!(st.verdict, CanaryVerdict::Running);

    let journal_before = rt.journal().len();
    let report = rt.promote().expect("promote");
    // Promotion is the ordinary classified swap — here a body-only SafeSwap.
    assert_eq!(report.class, EditClass::SafeSwap);
    assert_eq!(report.changed, vec!["score".to_string()]);
    assert_eq!(rt.journal().len(), journal_before + 1, "promotion is journaled");

    // The canary is consumed, and the live program computes the promoted code.
    assert!(rt.canary_status().is_none());
    assert_eq!(rt.call("score", &[6]), Ok(12));
}

#[test]
fn promote_is_seamless_under_concurrent_traffic() {
    let rt = Arc::new(LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile"));
    // Equivalent candidate, so the answer is 10 before *and* after promotion.
    rt.canary("int score(int x) { return x * 2; }\n", CanaryPolicy::default()).expect("start");

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let rt = rt.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match rt.call_canary("score", &[5]) {
                    Ok(v) => seen.push(v),
                    Err(e) => panic!("call failed during promotion: {e}"),
                }
            }
            seen
        })
    };

    thread::sleep(Duration::from_millis(50));
    let report = rt.promote().expect("promote");
    assert_eq!(report.class, EditClass::SafeSwap);

    // Let traffic run against the promoted program, then stop.
    let deadline = Instant::now() + Duration::from_secs(5);
    while rt.canary_status().is_some() && Instant::now() < deadline {
        thread::yield_now();
    }
    thread::sleep(Duration::from_millis(30));
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic");

    assert!(!seen.is_empty());
    // Equivalent code before and after: every observation is 10, never torn.
    assert!(seen.iter().all(|v| *v == 10), "torn value across promotion: {seen:?}");
    assert!(rt.canary_status().is_none(), "the canary was consumed by promotion");
    assert_eq!(rt.call("score", &[5]), Ok(10));
}

#[test]
fn an_aborted_candidate_cannot_be_promoted() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    rt.canary("int score(int x) { return x + 1; }\n", CanaryPolicy::default()).expect("start");
    assert_eq!(rt.call_canary("score", &[5]), Ok(10)); // diverges → aborts
    assert_eq!(rt.canary_status().unwrap().verdict, CanaryVerdict::AbortedOnDivergence);

    // The shield already rejected this candidate; promotion must be refused.
    assert!(rt.promote().is_err());
    // The live program is unchanged, and the operator can discard the canary.
    assert_eq!(rt.call("score", &[5]), Ok(10));
    rt.abort_canary();
    assert!(rt.canary_status().is_none());
}

#[test]
fn a_defective_candidate_is_rejected() {
    let rt = LiveRuntime::new("int main() { return 1; }\n").expect("compile");
    // A candidate that calls an undefined function is rejected by the same gate
    // as any program — no canary starts.
    let err = rt.canary("int main() { return ghost(1); }\n", CanaryPolicy::default());
    assert!(err.is_err(), "a defective candidate must not start a canary");
    assert!(rt.canary_status().is_none());
    // Only one canary at a time.
    rt.canary("int main() { return 2; }\n", CanaryPolicy::default()).expect("start");
    assert!(rt.canary("int main() { return 3; }\n", CanaryPolicy::default()).is_err());
    rt.abort_canary();
    // After aborting, a fresh canary can start again.
    assert!(rt.canary("int main() { return 4; }\n", CanaryPolicy::default()).is_ok());
}

#[test]
fn sampling_is_one_in_n() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    // Sample one call in four; never abort, so nothing stops the counting.
    let policy = CanaryPolicy { sample_every: 4, max_divergences: u64::MAX, ..Default::default() };
    rt.canary("int score(int x) { return x + 1; }\n", policy).expect("start");

    for _ in 0..100 {
        assert_eq!(rt.call_canary("score", &[5]), Ok(10));
    }
    // Exactly one in four calls was mirrored.
    assert_eq!(rt.canary_status().unwrap().samples, 25);
}

#[test]
fn call_canary_without_a_canary_is_just_call() {
    let rt = LiveRuntime::new("int score(int x) { return x + x; }\n").expect("compile");
    // With no canary active, call_canary is a straight call.
    assert_eq!(rt.call_canary("score", &[21]), Ok(42));
    assert!(rt.canary_status().is_none());
}
