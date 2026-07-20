//! Soundness proofs for the live-swap runtime.
//!
//! Each test pins one claim from the reload-as-a-theorem story to observable
//! behavior:
//!
//! | Claim                                                        | Test |
//! |--------------------------------------------------------------|------|
//! | Body edit swaps live, under fire, with zero missed calls     | [`body_edit_swaps_under_concurrent_execution`] |
//! | Body edit's blast radius is exactly the edited function      | same test, via `ReloadReport::changed` + query trace |
//! | ABI edit is classified `Relink` and commits atomically       | [`signature_edit_relinks_atomically_under_fire`] |
//! | Comment/formatting edit is proven `NoEffect` (zero codegen)  | [`comment_edit_is_proven_no_effect`] |
//! | Host functions dispatch through the same swap table          | [`host_functions_are_callable_and_hot_swappable`] |
//! | Undefined callees are memory-safe (missing stub)             | [`undefined_callee_is_memory_safe`] |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blaze_jit::{EditClass, LiveRuntime};

const V1: &str = "\
int add(int a, int b) {
    return a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// Body-only edit: `add` becomes `a + a + b`; ABI unchanged; `main` untouched.
const V2_BODY_ONLY: &str = "\
int add(int a, int b) {
    return a + a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// ABI edit: `add` gains a parameter and `main` calls it accordingly.
/// `main()` changes from 3 to 1 + 2 + 3 = 6.
const V3_SIGNATURE: &str = "\
int add(int a, int b, int c) {
    return a + b + c;
}

int main() {
    int x = add(1, 2, 3);
    return x;
}
";

/// V1 with a comment and whitespace inside `add` — lowers identically.
const V4_COMMENT: &str = "\
int add(int a, int b) {
    // the answer is the sum
    return a + b;
}

int main() {
    int x = add(1, 2);
    return x;
}
";

/// Spawn a hammer thread that calls `main()` in a tight loop until `stop`,
/// recording every result. Any `Err` or out-of-set value is a soundness bug.
fn hammer(runtime: Arc<LiveRuntime>, stop: Arc<AtomicBool>) -> thread::JoinHandle<Vec<i64>> {
    thread::spawn(move || {
        let mut seen = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            match runtime.call("main", &[]) {
                Ok(v) => seen.push(v),
                Err(e) => panic!("call failed during live execution: {e}"),
            }
        }
        seen
    })
}

/// Wait (bounded) until `main()` returns `expected` — i.e. the swap is visible.
fn wait_for_value(runtime: &LiveRuntime, expected: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if runtime.call("main", &[]) == Ok(expected) {
            return;
        }
        assert!(Instant::now() < deadline, "swap never became visible");
        thread::yield_now();
    }
}

#[test]
fn body_edit_swaps_under_concurrent_execution() {
    let runtime = Arc::new(LiveRuntime::new(V1).expect("initial compile"));
    let trace = runtime.enable_tracing();
    assert_eq!(runtime.call("main", &[]), Ok(3));
    let _ = trace.take();

    let stop = Arc::new(AtomicBool::new(false));
    let worker = hammer(runtime.clone(), stop.clone());

    // Let the hammer run hot, then swap `add`'s body out from under it.
    thread::sleep(Duration::from_millis(50));
    let report = runtime.reload(V2_BODY_ONLY).expect("reload");

    // The classifier proved this a body-only edit...
    assert_eq!(report.class, EditClass::SafeSwap);
    // ...with a blast radius of exactly the edited function.
    assert_eq!(report.changed, vec!["add".to_string()], "firewall bounds the radius");
    assert!(report.added.is_empty() && report.removed.is_empty());

    // The caller was never re-lowered: the query graph, not diff heuristics,
    // is what kept `main` out of the radius.
    let log = trace.take();
    assert!(
        !log.iter().any(|l| l == "lowered_dev_ir(main)"),
        "main must be a memo hit during a body-only reload; log = {log:?}",
    );

    wait_for_value(&runtime, 4);
    thread::sleep(Duration::from_millis(50));
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic");

    // Zero missed calls: every single concurrent call returned a value, and
    // every value is either old-correct (3) or new-correct (4) — never torn,
    // never garbage, never an error.
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|v| *v == 3 || *v == 4),
        "observed a torn value: {:?}",
        seen.iter().find(|v| **v != 3 && **v != 4),
    );
    assert_eq!(*seen.last().unwrap(), 4, "the edit must eventually win");
    assert!(seen.contains(&3), "the hammer must have run before the swap");
}

#[test]
fn signature_edit_relinks_atomically_under_fire() {
    let runtime = Arc::new(LiveRuntime::new(V1).expect("initial compile"));
    assert_eq!(runtime.call("main", &[]), Ok(3));

    let stop = Arc::new(AtomicBool::new(false));
    let worker = hammer(runtime.clone(), stop.clone());
    thread::sleep(Duration::from_millis(50));

    // Change `add`'s ABI while `main()` is being called in a loop. A naive
    // reloader that patched `add`'s pointer alone would let old `main` (2-arg
    // call) run into new `add` (3-arg body) — undefined behavior. Blaze's graph
    // forces `main` into the blast radius and commits both under quiescence.
    let report = runtime.reload(V3_SIGNATURE).expect("reload");
    assert_eq!(report.class, EditClass::Relink, "ABI change must be classified Relink");
    let mut radius = report.changed.clone();
    radius.sort();
    assert_eq!(
        radius,
        vec!["add".to_string(), "main".to_string()],
        "the graph must pull the caller into the blast radius",
    );

    wait_for_value(&runtime, 6);
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic — that is the whole point");

    // Atomicity: every observed value is either fully-old (3) or fully-new (6).
    // A mismatched caller/callee pair would produce garbage (e.g. reading an
    // uninitialized third argument register).
    assert!(
        seen.iter().all(|v| *v == 3 || *v == 6),
        "observed a torn ABI transition: {:?}",
        seen.iter().find(|v| **v != 3 && **v != 6),
    );
    assert_eq!(*seen.last().unwrap(), 6);
}

#[test]
fn comment_edit_is_proven_no_effect() {
    let runtime = LiveRuntime::new(V1).expect("initial compile");
    let generation_before = runtime.generation();

    let report = runtime.reload(V4_COMMENT).expect("reload");

    // The graph re-lowered `add` (its text changed) but the result was
    // identical, so the edit provably has no effect: nothing is compiled,
    // nothing is patched, no generation is spent.
    assert_eq!(report.class, EditClass::NoEffect);
    assert!(report.changed.is_empty());
    assert_eq!(runtime.generation(), generation_before, "no generation consumed");
    assert_eq!(runtime.call("main", &[]), Ok(3));
}

extern "C" fn triple(x: i64) -> i64 {
    x * 3
}

#[test]
fn host_functions_are_callable_and_hot_swappable() {
    // v1 does not use the host function at all.
    let v1 = "int main() { return 10; }\n";
    // v2 calls into native host code.
    let v2 = "int main() { return triple(10) + 1; }\n";

    let runtime = LiveRuntime::new(v1).expect("initial compile");
    // SAFETY: `triple` is extern "C", (i64) -> i64, lives for the program.
    unsafe { runtime.register_host_fn("triple", 1, triple as *const u8) };

    assert_eq!(runtime.call("main", &[]), Ok(10));
    assert_eq!(runtime.call("triple", &[7]), Ok(21), "host fns dispatch by name too");

    // Introducing a call to a host function is body-only: SafeSwap.
    let report = runtime.reload(v2).expect("reload");
    assert_eq!(report.class, EditClass::SafeSwap);
    assert_eq!(runtime.call("main", &[]), Ok(31));
}

#[test]
fn undefined_callee_is_memory_safe() {
    // `ghost` is never defined: its slot holds the missing stub, so the call
    // returns 0 instead of jumping into the void.
    let src = "int main() { return ghost(1, 2) + 5; }\n";
    let runtime = LiveRuntime::new(src).expect("compile");
    assert_eq!(runtime.call("main", &[]), Ok(5));
}

#[test]
fn removing_a_function_is_a_relink_and_stays_safe() {
    let src_two = "\
int helper(int x) {
    return x * 2;
}

int main() {
    return helper(21);
}
";
    // `helper` deleted while `main` still calls it: the classifier must go
    // Relink (main's DevIR changes — helper's signature read now defaults),
    // and the process must keep running with the missing-stub semantics.
    let src_one = "\
int main() {
    return helper(21);
}
";
    let runtime = LiveRuntime::new(src_two).expect("compile");
    assert_eq!(runtime.call("main", &[]), Ok(42));

    let report = runtime.reload(src_one).expect("reload");
    assert_eq!(report.class, EditClass::Relink);
    assert_eq!(report.removed, vec!["helper".to_string()]);
    assert_eq!(runtime.call("main", &[]), Ok(0), "missing stub returns 0");
    assert_eq!(
        runtime.call("helper", &[1]),
        Err(blaze_jit::CallError::UnknownFunction("helper".to_string())),
        "removed functions leave the dispatch table",
    );
}
