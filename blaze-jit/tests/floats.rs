//! End-to-end proofs for `float` (IEEE-754 `f64`) support.
//!
//! Blaze carries every value through the machine ABI as a raw 64-bit word and
//! bit-casts `i64 ↔ f64` only at parameter, return, and call boundaries inside
//! generated code (see `codegen`). These tests pin that scheme — and the type
//! system that keeps it sound — to observable behavior:
//!
//! | Claim                                                          | Test |
//! |----------------------------------------------------------------|------|
//! | Float arithmetic runs parse→lower→codegen→execute, exactly     | [`float_arithmetic_executes_end_to_end`] |
//! | Float args and returns round-trip across a call boundary       | [`float_args_and_returns_round_trip_across_calls`] |
//! | Float comparison drives control flow                           | [`float_comparison_drives_control_flow`] |
//! | Float division never faults (`x/0.0` is a defined ±inf)        | [`float_division_never_faults`] |
//! | The typed API decodes results by the declared return type      | [`typed_call_decodes_by_return_type`] |
//! | The typed API rejects an argument of the wrong type            | [`typed_call_rejects_a_type_mismatch`] |
//! | The raw `i64` path and the typed path agree on the bits        | [`raw_and_typed_paths_agree_on_float_bits`] |
//! | The lock-free typed handle path matches the locked one         | [`typed_handle_matches_typed_call`] |
//! | A float body edit hot-swaps live, under a second thread        | [`float_body_swap_is_sound_under_concurrent_execution`] |
//! | Retyping a parameter is a Relink, atomic under a second thread  | [`retyping_a_parameter_relinks_atomically_under_fire`] |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blaze_jit::{CallError, EditClass, LiveRuntime, Value};

#[test]
fn float_arithmetic_executes_end_to_end() {
    // `2.5` and `2.0` are exactly representable, so the product is exact.
    let runtime = LiveRuntime::new("float scale(float x) { return x * 2.5; }\n").expect("compile");
    assert_eq!(runtime.call_typed("scale", &[Value::Float(2.0)]), Ok(Value::Float(5.0)));
    assert_eq!(runtime.call_typed("scale", &[Value::Float(-4.0)]), Ok(Value::Float(-10.0)));
    // Unary minus in float context, and float subtraction.
    let rt2 = LiveRuntime::new("float neg(float x) { return -x - 1.5; }\n").expect("compile");
    assert_eq!(rt2.call_typed("neg", &[Value::Float(2.0)]), Ok(Value::Float(-3.5)));
}

#[test]
fn float_args_and_returns_round_trip_across_calls() {
    // `main` passes a float to `half`, gets a float back, and combines it —
    // exercising the bit-cast at the argument boundary, the return boundary,
    // and the caller's re-interpretation of the result. If any bit-cast were
    // wrong, the raw integer pattern of the float would surface as garbage.
    let src = "\
float half(float x) {
    return x / 2.0;
}

float describe(float x) {
    return half(x) + 1.0;
}
";
    let runtime = LiveRuntime::new(src).expect("compile");
    assert_eq!(runtime.call_typed("half", &[Value::Float(9.0)]), Ok(Value::Float(4.5)));
    assert_eq!(runtime.call_typed("describe", &[Value::Float(9.0)]), Ok(Value::Float(5.5)));
}

#[test]
fn float_comparison_drives_control_flow() {
    // A float comparison yields an `int` boolean, so it is a valid condition;
    // the two arms return different floats.
    let src = "\
float clamp(float x) {
    if (x > 1.0) {
        return 1.0;
    }
    return x;
}
";
    let runtime = LiveRuntime::new(src).expect("compile");
    assert_eq!(runtime.call_typed("clamp", &[Value::Float(2.5)]), Ok(Value::Float(1.0)));
    assert_eq!(runtime.call_typed("clamp", &[Value::Float(0.25)]), Ok(Value::Float(0.25)));
}

#[test]
fn float_division_never_faults() {
    // IEEE division by zero is defined (±inf / NaN), never a trap — a
    // live-edited script must never be able to fault the host, and floats get
    // that for free (no guard needed, unlike integer division).
    let runtime = LiveRuntime::new("float div(float a, float b) { return a / b; }\n").expect("compile");
    let got = runtime.call_typed("div", &[Value::Float(1.0), Value::Float(0.0)]).expect("no fault");
    match got {
        Value::Float(f) => assert!(f.is_infinite() && f > 0.0, "1.0/0.0 must be +inf, got {f}"),
        other => panic!("expected a float, got {other:?}"),
    }
    // Zero-over-zero is NaN — still defined, still no fault.
    let nan = runtime.call_typed("div", &[Value::Float(0.0), Value::Float(0.0)]).expect("no fault");
    match nan {
        Value::Float(f) => assert!(f.is_nan(), "0.0/0.0 must be NaN, got {f}"),
        other => panic!("expected a float, got {other:?}"),
    }
}

#[test]
fn typed_call_decodes_by_return_type() {
    // The same runtime holds an int function and a float function; the typed
    // API decodes each result according to its declared return type.
    let src = "\
int inc(int n) {
    return n + 1;
}

float scale(float x) {
    return x * 3.0;
}
";
    let runtime = LiveRuntime::new(src).expect("compile");
    assert_eq!(runtime.call_typed("inc", &[Value::Int(41)]), Ok(Value::Int(42)));
    assert_eq!(runtime.call_typed("scale", &[Value::Float(2.0)]), Ok(Value::Float(6.0)));
}

#[test]
fn typed_call_rejects_a_type_mismatch() {
    let runtime = LiveRuntime::new("float scale(float x) { return x * 2.0; }\n").expect("compile");
    // Passing an `int` where a `float` is declared must be a typed error, not a
    // silent bit-reinterpretation of the integer pattern as an f64.
    let err = runtime.call_typed("scale", &[Value::Int(2)]);
    assert!(
        matches!(err, Err(CallError::TypeMismatch { position: 0, .. })),
        "expected a TypeMismatch at argument 0, got {err:?}",
    );
    // The message names both types, so a host sees exactly what went wrong.
    let msg = match err {
        Err(e) => e.to_string(),
        Ok(_) => unreachable!(),
    };
    assert!(msg.contains("expects float") && msg.contains("got int"), "{msg}");
}

#[test]
fn raw_and_typed_paths_agree_on_float_bits() {
    // The raw `i64` ABI path carries a float as its bit pattern; the typed path
    // decodes it. Both must describe the same value.
    let runtime = LiveRuntime::new("float scale(float x) { return x * 2.0; }\n").expect("compile");
    let x = 3.5f64;
    let raw = runtime.call("scale", &[x.to_bits() as i64]).expect("raw call");
    assert_eq!(f64::from_bits(raw as u64), 7.0);
    assert_eq!(runtime.call_typed("scale", &[Value::Float(x)]), Ok(Value::Float(7.0)));
}

#[test]
fn typed_handle_matches_typed_call() {
    let runtime = LiveRuntime::new("float scale(float x) { return x * 2.0; }\n").expect("compile");
    let mut h = runtime.handle("scale").expect("resolve");
    assert_eq!(runtime.call_handle_typed(&mut h, &[Value::Float(4.0)]), Ok(Value::Float(8.0)));
    assert_eq!(
        runtime.call_handle_typed(&mut h, &[Value::Float(4.0)]),
        runtime.call_typed("scale", &[Value::Float(4.0)]),
    );
    // Wrong argument type on the fast path is a defined error too.
    assert!(matches!(
        runtime.call_handle_typed(&mut h, &[Value::Int(4)]),
        Err(CallError::TypeMismatch { .. })
    ));
}

/// Body-only float edit under a second thread: `score` scales by 2.0, then 3.0.
const SCORE_V1: &str = "float score(float x) { return x * 2.0; }\n";
const SCORE_V2: &str = "float score(float x) { return x * 3.0; }\n";

#[test]
fn float_body_swap_is_sound_under_concurrent_execution() {
    let runtime = Arc::new(LiveRuntime::new(SCORE_V1).expect("compile"));
    assert_eq!(runtime.call_typed("score", &[Value::Float(3.0)]), Ok(Value::Float(6.0)));

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let runtime = runtime.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match runtime.call_typed("score", &[Value::Float(3.0)]) {
                    Ok(v) => seen.push(v),
                    Err(e) => panic!("typed float call failed mid-swap: {e}"),
                }
            }
            seen
        })
    };

    thread::sleep(Duration::from_millis(50));
    let report = runtime.reload(SCORE_V2).expect("reload");
    assert_eq!(report.class, EditClass::SafeSwap, "same (float)->float signature is a body swap");
    assert_eq!(report.changed, vec!["score".to_string()]);

    // Spin until the new value is visible.
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.call_typed("score", &[Value::Float(3.0)]) != Ok(Value::Float(9.0)) {
        assert!(Instant::now() < deadline, "swap never became visible");
        thread::yield_now();
    }
    thread::sleep(Duration::from_millis(30));
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic");

    // The soundness theorem for floats: every concurrent observation is either
    // old-correct (6.0) or new-correct (9.0) — never a torn bit pattern that a
    // wrong or half-applied bit-cast would produce.
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|v| *v == Value::Float(6.0) || *v == Value::Float(9.0)),
        "observed a torn float value: {:?}",
        seen.iter().find(|v| **v != Value::Float(6.0) && **v != Value::Float(9.0)),
    );
    assert!(seen.contains(&Value::Float(6.0)), "the hammer must have run before the swap");
    assert_eq!(runtime.call_typed("score", &[Value::Float(3.0)]), Ok(Value::Float(9.0)));
}

/// V1 computes in `float`; V2 retypes the whole pipeline to `int`. Every
/// signature changes, so this is a Relink across a representation change — the
/// scariest transition, because the same 64 bits mean utterly different values.
const PIPE_FLOAT: &str = "\
float compute(float x) {
    return x * 2.0;
}

float main() {
    return compute(3.0);
}
";
const PIPE_INT: &str = "\
int compute(int x) {
    return x * 2;
}

int main() {
    return compute(3);
}
";

#[test]
fn retyping_a_parameter_relinks_atomically_under_fire() {
    let runtime = Arc::new(LiveRuntime::new(PIPE_FLOAT).expect("compile"));
    assert_eq!(runtime.call_typed("main", &[]), Ok(Value::Float(6.0)));

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let runtime = runtime.clone();
        let stop = stop.clone();
        thread::spawn(move || {
            let mut seen = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match runtime.call_typed("main", &[]) {
                    Ok(v) => seen.push(v),
                    Err(e) => panic!("typed call failed across a retype: {e}"),
                }
            }
            seen
        })
    };

    thread::sleep(Duration::from_millis(50));
    // Retype float→int. The signatures of both `compute` and `main` change, so
    // the graph forces both into the blast radius and commits them together
    // under the quiescence barrier — a reader can never see int `main` calling
    // float `compute`, or decode a float result with an int return type.
    let report = runtime.reload(PIPE_INT).expect("reload");
    assert_eq!(report.class, EditClass::Relink, "a parameter/return retype must be a Relink");
    let mut radius = report.changed.clone();
    radius.sort();
    assert_eq!(radius, vec!["compute".to_string(), "main".to_string()]);

    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.call_typed("main", &[]) != Ok(Value::Int(6)) {
        assert!(Instant::now() < deadline, "retype never became visible");
        thread::yield_now();
    }
    stop.store(true, Ordering::Relaxed);
    let seen = worker.join().expect("worker must never panic");

    // Atomicity across the representation change: every observation is either
    // fully-old float 6.0 or fully-new int 6 — decoded by the matching return
    // type, never mixed. A non-atomic commit would let a reader decode new int
    // bits as a float (or vice versa), producing neither value.
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|v| *v == Value::Float(6.0) || *v == Value::Int(6)),
        "observed a torn retype: {:?}",
        seen.iter().find(|v| **v != Value::Float(6.0) && **v != Value::Int(6)),
    );
    assert_eq!(runtime.call_typed("main", &[]), Ok(Value::Int(6)));
}
