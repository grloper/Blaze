//! Proofs for the reload journal and `rollback` (P2).
//!
//! | Claim                                                        | Test |
//! |--------------------------------------------------------------|------|
//! | The initial load is journaled as committed generation 1      | [`initial_load_is_journaled`] |
//! | Every reload event is journaled in order, with its class     | [`every_event_is_journaled_in_order`] |
//! | A `Rejected` event is journaled with diagnostics, uncommitted| [`rejected_events_are_journaled_but_not_committed`] |
//! | Rollback reverts behavior via the normal swap protocol       | [`rollback_reverts_a_body_edit`] |
//! | Rollback across an ABI change is a `Relink`                   | [`rollback_across_an_abi_change_is_a_relink`] |
//! | Rollback to the current generation is a `NoEffect`            | [`rollback_to_current_generation_is_no_effect`] |
//! | Rollback to a non-committed generation errors                | [`rollback_to_a_bad_generation_errors`] |
//! | Rollback is torn-free under a second thread                  | [`rollback_is_sound_under_concurrent_execution`] |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blaze_jit::{EditClass, LiveRuntime};

/// `main` calls `rate`; editing `rate`'s body changes `main`'s result without
/// re-lowering `main` (the firewall) — a clean SafeSwap to roll back.
const RATE_V1: &str = "\
int rate(int x) {
    return x + 1;
}

int main() {
    return rate(10);
}
";
const RATE_V2: &str = "\
int rate(int x) {
    return x + 100;
}

int main() {
    return rate(10);
}
";

#[test]
fn initial_load_is_journaled() {
    let rt = LiveRuntime::new(RATE_V1).expect("compile");
    let log = rt.journal();
    assert_eq!(log.len(), 1);
    let e = &log[0];
    assert_eq!(e.sequence, 0);
    assert_eq!(e.generation, 1, "the initial load is generation 1");
    assert!(e.is_committed());
    assert_eq!(e.class, EditClass::SafeSwap);
    assert!(e.source.contains("return x + 1;"), "the entry retains its exact source");
}

#[test]
fn every_event_is_journaled_in_order() {
    let rt = LiveRuntime::new(RATE_V1).expect("compile");
    rt.reload(RATE_V2).expect("reload"); // SafeSwap, gen 2
    rt.reload(RATE_V2).expect("reload"); // NoEffect (identical), gen stays 2

    let log = rt.journal();
    assert_eq!(log.len(), 3);

    assert_eq!(log[0].generation, 1);
    assert_eq!(log[0].class, EditClass::SafeSwap);

    assert_eq!(log[1].sequence, 1);
    assert_eq!(log[1].generation, 2);
    assert_eq!(log[1].class, EditClass::SafeSwap);
    assert_eq!(log[1].changed, vec!["rate".to_string()], "the radius is recorded");

    assert_eq!(log[2].sequence, 2);
    assert_eq!(log[2].class, EditClass::NoEffect);
    assert_eq!(log[2].generation, 2, "a NoEffect event does not advance the generation");
    assert!(!log[2].is_committed());
}

#[test]
fn rejected_events_are_journaled_but_not_committed() {
    let rt = LiveRuntime::new(RATE_V1).expect("compile");
    // A call to an undefined function — Rejected, holds last-good.
    let report = rt.reload("int main() { return ghost(1); }\n").expect("reload call ok");
    assert_eq!(report.class, EditClass::Rejected);

    let log = rt.journal();
    assert_eq!(log.len(), 2);
    let e = &log[1];
    assert_eq!(e.class, EditClass::Rejected);
    assert!(!e.is_committed(), "a rejected event installed nothing");
    assert_eq!(e.generation, 1, "the generation did not advance");
    assert!(
        e.diagnostics.iter().any(|(_, d)| d.message.contains("undefined function `ghost`")),
        "the journal records why it was rejected: {:?}",
        e.diagnostics,
    );
    // The rejected source is not a rollback target.
    assert!(rt.rollback(1).is_ok(), "generation 1 is still committed and reachable");
}

#[test]
fn rollback_reverts_a_body_edit() {
    let rt = LiveRuntime::new(RATE_V1).expect("compile");
    assert_eq!(rt.call("main", &[]), Ok(11));
    rt.reload(RATE_V2).expect("reload");
    assert_eq!(rt.call("main", &[]), Ok(110));

    // Roll back to generation 1's source. It runs through the ordinary reload
    // protocol — reverting a body edit is itself a body swap.
    let report = rt.rollback(1).expect("rollback");
    assert_eq!(report.class, EditClass::SafeSwap);
    assert_eq!(rt.call("main", &[]), Ok(11), "behavior reverted to generation 1");

    // The rollback is journaled as a new event (generation 3), not a mutation
    // of history.
    let log = rt.journal();
    assert_eq!(log.len(), 3);
    assert_eq!(log[2].generation, 3);
    assert!(log[2].source.contains("return x + 1;"));
}

#[test]
fn rollback_across_an_abi_change_is_a_relink() {
    let v2 = "int add(int a, int b) { return a + b; }\nint main() { return add(1, 2); }\n";
    let v3 = "int add(int a, int b, int c) { return a + b + c; }\nint main() { return add(1, 2, 3); }\n";
    let rt = LiveRuntime::new(v2).expect("compile");
    assert_eq!(rt.call("main", &[]), Ok(3));
    rt.reload(v3).expect("reload");
    assert_eq!(rt.call("main", &[]), Ok(6));

    // Reverting an ABI change is an ABI change back — the same Relink protocol.
    let report = rt.rollback(1).expect("rollback");
    assert_eq!(report.class, EditClass::Relink);
    assert_eq!(rt.call("main", &[]), Ok(3));
}

#[test]
fn rollback_to_current_generation_is_no_effect() {
    let rt = LiveRuntime::new("int main() { return 1; }\n").expect("compile");
    // Reinstalling the current source changes nothing — proven, costs nothing.
    let report = rt.rollback(1).expect("rollback");
    assert_eq!(report.class, EditClass::NoEffect);
    assert_eq!(rt.call("main", &[]), Ok(1));
}

#[test]
fn rollback_to_a_bad_generation_errors() {
    let rt = LiveRuntime::new("int main() { return 1; }\n").expect("compile");
    // Generation 0 is the pre-init sentinel; 99 never happened.
    assert!(rt.rollback(0).is_err());
    assert!(rt.rollback(99).is_err());
}

#[test]
fn rollback_is_sound_under_concurrent_execution() {
    let rt = Arc::new(LiveRuntime::new(RATE_V1).expect("compile"));
    rt.reload(RATE_V2).expect("reload"); // main is now 110
    assert_eq!(rt.call("main", &[]), Ok(110));

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let rt = rt.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match rt.call("main", &[]) {
                    Ok(v) => seen.push(v),
                    Err(e) => panic!("call failed during rollback: {e}"),
                }
            }
            seen
        })
    };

    thread::sleep(Duration::from_millis(50));
    // Roll back to generation 1 while the hammer runs. Rollback inherits the
    // swap protocol, so this is the same soundness theorem as any reload.
    let report = rt.rollback(1).expect("rollback");
    assert_eq!(report.class, EditClass::SafeSwap);

    let deadline = Instant::now() + Duration::from_secs(10);
    while rt.call("main", &[]) != Ok(11) {
        assert!(Instant::now() < deadline, "rollback never became visible");
        thread::yield_now();
    }
    thread::sleep(Duration::from_millis(30));
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic");

    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|v| *v == 110 || *v == 11),
        "observed a torn value during rollback: {:?}",
        seen.iter().find(|v| **v != 110 && **v != 11),
    );
    assert!(seen.contains(&110), "the hammer must have run before the rollback");
    assert_eq!(rt.call("main", &[]), Ok(11));
}
