//! **The future-facing pitch: an AI agent safely evolving live business logic.**
//!
//! ```sh
//! cargo run -p blaze-jit --example agent_loop
//! ```
//!
//! A scripted "agent" iterates toward a target metric — here, lowering the mean
//! risk score of a card-authorization model so fewer legitimate charges are
//! declined — by proposing edits to the live `rules.blaze` logic. Each proposal
//! runs the full safety pipeline Blaze gives an autonomous editor:
//!
//! ```text
//!   propose ─▶ GATE ─▶ offline eval ─▶ CANARY on live traffic ─▶ promote / abort ─▶ read metrics
//! ```
//!
//!   * **Gate** — a proposal that doesn't compile (a typo, a type error) is
//!     refused by the diagnostics gate before anything runs. It never reaches
//!     production.
//!   * **Offline eval** — the agent measures the candidate against a held-out
//!     request set: does it move the objective, and does it still flag the
//!     known-fraud guardrail cases? A regression or a guardrail breach is
//!     declined without touching live traffic.
//!   * **Canary** — the survivor is shadowed against *live* traffic. This is the
//!     step that catches what the offline eval can't: the second proposal below
//!     looks great offline, but it loops forever on a request shape the eval set
//!     never included — and the canary traps that on real traffic *before*
//!     promotion. This is the whole safety argument for letting an agent touch
//!     production logic: the blast radius of a bad idea is a shadow execution,
//!     not an outage.
//!   * **Promote / read metrics** — a clean, improving candidate is promoted
//!     through the ordinary classified swap, and the agent reads the live
//!     flight-recorder metrics to confirm progress before proposing again.
//!
//! There is no LLM and no network here — that is deliberate, so this runs in CI
//! and tells the story deterministically. In a real deployment the `PROPOSALS`
//! list below is replaced by a model: prompt it with the current rules, the
//! objective, and the last metrics; ask for a unified diff or a full new source;
//! feed its answer into the *same* pipeline. The safety guarantees are the
//! pipeline's, not the model's — which is exactly the point. See
//! `propose_with_llm` at the bottom for the wiring sketch.

use blaze_jit::{CanaryPolicy, CanaryVerdict, LiveRuntime};

// ---------------------------------------------------------------------------
// The domain: a risk model the agent is trying to improve
// ---------------------------------------------------------------------------

/// A scoring request: `(amount_in_cents, velocity_txns_per_hour, account_age_days)`.
type Req = (i64, i64, i64);

/// The launch rules. `velocity` is weighted heavily (×6), which the business
/// suspects is over-declining good customers — the agent's job is to bring the
/// mean score down without letting genuine fraud through.
const BASE_RULES: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) { return 45; }
    if (amount > 10000) { return 20; }
    return 0;
}

int velocity_risk(int velocity) {
    return velocity * 6;
}

int age_risk(int age) {
    if (age < 30) { return 25; }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// A candidate the agent proposes, as it would come back from a model: a short
/// rationale and a full new program source.
struct Proposal {
    rationale: &'static str,
    source: &'static str,
}

/// The scripted proposal stream (an LLM's outputs, canned for CI). In order:
/// a gate-rejected typo; a plausible-but-runaway rule; a clean win; an
/// over-permissive rule that breaks the guardrail; and the rule that hits target.
const PROPOSALS: &[Proposal] = &[
    // 1. A typo — the gate must refuse this before it can run anywhere.
    Proposal {
        rationale: "lower the velocity weight to 4 (typo: `velcity`)",
        source: "\
int amount_risk(int amount) {
    if (amount > 100000) { return 45; }
    if (amount > 10000) { return 20; }
    return 0;
}
int velocity_risk(int velocity) { return velcity * 4; }
int age_risk(int age) { if (age < 30) { return 25; } return 0; }
int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
",
    },
    // 2. Looks like a fine improvement offline (velocity ×3) — but it loops
    //    forever when velocity > 10, a shape the eval set never covers. Only the
    //    canary, on live traffic, sees it.
    Proposal {
        rationale: "lower the velocity weight to 3 (hidden runaway for velocity > 10)",
        source: "\
int amount_risk(int amount) {
    if (amount > 100000) { return 45; }
    if (amount > 10000) { return 20; }
    return 0;
}
int velocity_risk(int velocity) {
    while (velocity > 10) { velocity = velocity + 1; }
    return velocity * 3;
}
int age_risk(int age) { if (age < 30) { return 25; } return 0; }
int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
",
    },
    // 3. A clean win: velocity ×4. Improves the objective, keeps the guardrail.
    Proposal {
        rationale: "lower the velocity weight to 4",
        source: "\
int amount_risk(int amount) {
    if (amount > 100000) { return 45; }
    if (amount > 10000) { return 20; }
    return 0;
}
int velocity_risk(int velocity) { return velocity * 4; }
int age_risk(int age) { if (age < 30) { return 25; } return 0; }
int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
",
    },
    // 4. Over-permissive: zero out amount risk and barely weight velocity. Mean
    //    plummets — but the known-fraud guardrail cases stop being flagged, so
    //    the offline eval declines it without ever going live.
    Proposal {
        rationale: "zero amount risk, velocity ×1 (maximizes approvals)",
        source: "\
int amount_risk(int amount) { return 0; }
int velocity_risk(int velocity) { return velocity; }
int age_risk(int age) { if (age < 30) { return 25; } return 0; }
int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
",
    },
    // 5. The rule that reaches target: velocity ×2, guardrail intact.
    Proposal {
        rationale: "lower the velocity weight to 2",
        source: "\
int amount_risk(int amount) {
    if (amount > 100000) { return 45; }
    if (amount > 10000) { return 20; }
    return 0;
}
int velocity_risk(int velocity) { return velocity * 2; }
int age_risk(int age) { if (age < 30) { return 25; } return 0; }
int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
",
    },
];

/// The objective: drive the mean score on the eval set at or below this. The
/// ×6 → ×4 → ×2 velocity-weight progression walks the mean 63.3 → 53.3 → 43.3,
/// so target is reached only after the second clean promotion.
const TARGET_MEAN: f64 = 45.0;
/// The guardrail: every known-fraud case must still score at least this high.
const FLAG_MIN: i64 = 60;
/// A tight per-call fuel budget, so a runaway shadow traps in well under a ms.
const FUEL: u64 = 200_000;

// ---------------------------------------------------------------------------
// The eval + live request sets
// ---------------------------------------------------------------------------

/// The held-out evaluation set the agent scores candidates against — a spread of
/// realistic requests, but with `velocity` capped at 10. That cap is the point:
/// it is exactly the blind spot the runaway candidate hides in.
fn eval_set() -> Vec<Req> {
    let mut reqs = Vec::new();
    for i in 0..220u64 {
        let amount = ((i * 1373) % 200_000) as i64;
        let velocity = (i % 11) as i64; // 0..=10 — never triggers the runaway
        let age = ((i * 7) % 120) as i64;
        reqs.push((amount, velocity, age));
    }
    reqs
}

/// The live traffic the canary shadows against — the *same* shape, but drawn
/// from the real world, where `velocity` reaches 12. This is where the runaway
/// the eval set missed actually happens.
fn live_set() -> Vec<Req> {
    let mut reqs = Vec::new();
    for i in 0..300u64 {
        let amount = ((i * 1373) % 200_000) as i64;
        let velocity = (i % 13) as i64; // 0..=12 — includes the runaway trigger
        let age = ((i * 7) % 120) as i64;
        reqs.push((amount, velocity, age));
    }
    reqs
}

/// Known-fraud cases the model must always flag (big amount, brand-new account),
/// all with `velocity <= 10` so the offline guardrail check itself never trips
/// the hidden runaway.
const GUARDRAIL: &[Req] = &[(180_000, 8, 3), (250_000, 9, 1), (150_000, 10, 10)];

// ---------------------------------------------------------------------------
// The eval harness
// ---------------------------------------------------------------------------

/// Mean `score` over `reqs`, or `None` if any call faulted (e.g. a runaway).
fn mean_score(rt: &LiveRuntime, reqs: &[Req]) -> Option<f64> {
    let mut sum = 0i64;
    for &(a, v, g) in reqs {
        sum += rt.call("score", &[a, v, g]).ok()?;
    }
    Some(sum as f64 / reqs.len() as f64)
}

/// Whether every guardrail case still scores at or above [`FLAG_MIN`].
fn holds_guardrail(rt: &LiveRuntime) -> bool {
    GUARDRAIL.iter().all(|&(a, v, g)| rt.call("score", &[a, v, g]).map(|s| s >= FLAG_MIN).unwrap_or(false))
}

/// The offline verdict on a candidate: it compiled, and here is how it scores.
struct OfflineEval {
    mean: f64,
    guardrail_ok: bool,
}

/// Compile a candidate in isolation and evaluate it offline. `Err` means the
/// gate rejected it (or it faulted on the eval set — which, note, it won't for
/// the runaway, because the eval set's `velocity` never exceeds 10).
fn offline_eval(source: &str) -> Result<OfflineEval, String> {
    let candidate = LiveRuntime::new(source)?; // ← the gate; Err on any diagnostic
    candidate.set_fuel_budget(FUEL);
    let mean = mean_score(&candidate, &eval_set()).ok_or("candidate faulted on the eval set")?;
    let guardrail_ok = holds_guardrail(&candidate);
    Ok(OfflineEval { mean, guardrail_ok })
}

// ---------------------------------------------------------------------------
// The agent loop
// ---------------------------------------------------------------------------

/// What happened to a proposal — collected into a trace the run asserts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The diagnostics gate refused it (never ran anywhere).
    GateRejected,
    /// Offline eval declined it (regression or guardrail breach) before canary.
    DeclinedOffline,
    /// The canary caught it faulting/regressing on live traffic; not promoted.
    CanaryAborted,
    /// Promoted to live through the classified swap.
    Promoted,
}

// `pub` so the CI gate in `tests/agent_loop.rs` can drive the same run; as the
// example binary's entry point it behaves exactly as a normal `main`.
pub fn main() {
    let rt = LiveRuntime::new(BASE_RULES).expect("base rules compile");
    rt.set_fuel_budget(FUEL);
    rt.set_metrics_enabled(true); // the flight recorder — read after each promote

    let mut current_mean = mean_score(&rt, &eval_set()).expect("base rules score cleanly");
    println!("agent objective: lower mean risk score to ≤ {TARGET_MEAN:.0} (guardrail: fraud cases ≥ {FLAG_MIN})");
    println!("starting live mean: {current_mean:.1}\n");

    let mut trace = Vec::new();
    for (i, proposal) in PROPOSALS.iter().enumerate() {
        println!("proposal {}: {}", i + 1, proposal.rationale);
        let outcome = evaluate_proposal(&rt, proposal, &mut current_mean);
        trace.push(outcome);
        if current_mean <= TARGET_MEAN {
            println!("\n✓ target reached: live mean {current_mean:.1} ≤ {TARGET_MEAN:.0}");
            break;
        }
    }

    report_metrics(&rt);
    assert_run(&rt, &trace, current_mean);
    println!("\n\x1b[1;32magent run verified: gate + offline eval + canary kept every bad edit out of production ✓\x1b[0m");
}

/// Run one proposal through the whole pipeline, mutating `current_mean` on a
/// promote, and return what happened.
fn evaluate_proposal(rt: &LiveRuntime, proposal: &Proposal, current_mean: &mut f64) -> Outcome {
    // 1. GATE + offline objective.
    let eval = match offline_eval(proposal.source) {
        Err(diag) => {
            println!("   ✗ gate rejected — {}", diagnostic_detail(&diag));
            return Outcome::GateRejected;
        }
        Ok(eval) => eval,
    };
    if !eval.guardrail_ok {
        println!("   ✗ declined offline — breaks the fraud guardrail (mean would be {:.1})", eval.mean);
        return Outcome::DeclinedOffline;
    }
    if eval.mean >= *current_mean {
        println!("   ✗ declined offline — no improvement (mean {:.1} ≥ current {:.1})", eval.mean, *current_mean);
        return Outcome::DeclinedOffline;
    }
    println!("   offline: mean {:.1} (↓ from {:.1}), guardrail ok — canarying on live traffic", eval.mean, *current_mean);

    // 2. CANARY against live traffic. The policy tolerates *intended* score
    //    changes (this is a behavior edit) but the agent treats any live fault —
    //    or a latency blow-up — as disqualifying: a shadow that traps on a real
    //    request shape must never be promoted.
    let policy = CanaryPolicy { max_divergences: u64::MAX, min_samples_for_latency: 20, max_latency_ratio: 5.0, sample_every: 1 };
    rt.canary(proposal.source, policy).expect("candidate already compiled in offline eval");
    for &(a, v, g) in &live_set() {
        let _ = rt.call_canary("score", &[a, v, g]); // caller always gets the live answer
    }
    let status = rt.canary_status().expect("canary active");
    if status.candidate_faults > 0 || status.verdict != CanaryVerdict::Running {
        println!(
            "   ✗ canary aborted on live traffic — {} fault(s), verdict {:?}; never promoted",
            status.candidate_faults, status.verdict
        );
        rt.abort_canary();
        return Outcome::CanaryAborted;
    }

    // 3. PROMOTE through the ordinary classified swap.
    let report = rt.promote().expect("a healthy candidate promotes");
    *current_mean = mean_score(rt, &eval_set()).expect("promoted rules score cleanly");
    println!(
        "   ✓ promoted as {:?} (radius {{{}}}); live mean now {:.1}",
        report.class,
        report.changed.join(", "),
        current_mean
    );
    Outcome::Promoted
}

/// Read the live flight-recorder metrics for `score` — the same lock-free
/// counters an operator (or the agent) watches in production.
fn report_metrics(rt: &LiveRuntime) {
    if let Some(m) = rt.metrics("score") {
        println!(
            "\nflight recorder — score: {} calls, {} faults, mean {} ns/call",
            m.calls,
            m.faults,
            m.mean_nanos()
        );
    }
}

/// Assert the run did exactly what the safety pipeline promises — this makes the
/// example a CI gate, not just a story.
fn assert_run(rt: &LiveRuntime, trace: &[Outcome], final_mean: f64) {
    use Outcome::*;
    assert_eq!(
        trace,
        &[GateRejected, CanaryAborted, Promoted, DeclinedOffline, Promoted],
        "the agent must gate the typo, let the canary catch the runaway, promote the two clean wins, \
         and decline the guardrail-breaking overshoot"
    );
    // The runaway was proposal 2; it must never have reached production.
    assert_eq!(trace[1], CanaryAborted, "the runaway must be caught by the canary, not promoted");
    // The objective was met, and the guardrail still holds on the live program.
    assert!(final_mean <= TARGET_MEAN, "the agent must reach the target mean (got {final_mean:.1})");
    assert!(holds_guardrail(rt), "the live program must still flag every guardrail fraud case");
    // The live program is healthy and computes a sane score.
    assert!(rt.call("score", &[50_000, 3, 40]).is_ok(), "the live program serves after the run");
}

/// Pull the human-readable detail out of a rejection (its last, most specific
/// line — the actual diagnostic, e.g. "undefined variable `velcity`").
fn diagnostic_detail(s: &str) -> String {
    s.lines().last().unwrap_or(s).trim().to_string()
}

// ---------------------------------------------------------------------------
// Wiring a real model in (documentation, not run)
// ---------------------------------------------------------------------------

/// Sketch of how the canned `PROPOSALS` stream is replaced by a real LLM. The
/// pipeline in `main` does not change at all — the model only supplies the
/// candidate source; the gate, the offline eval, and the canary are what keep it
/// safe. That separation is the entire pitch.
///
/// ```ignore
/// async fn propose_with_llm(current_source: &str, objective: &str, last_metrics: &str) -> String {
///     let prompt = format!(
///         "Here is the current risk model:\n{current_source}\n\
///          Objective: {objective}. Last live metrics: {last_metrics}.\n\
///          Return ONLY a full replacement program, same `score(int,int,int)` entry point.");
///     // Call your model of choice; return its completion verbatim as the new
///     // source. Anthropic's Messages API, for example:
///     //   let msg = client.messages().create(..).await?;
///     //   msg.content_text()
///     llm_complete(&prompt).await
/// }
/// ```
#[allow(dead_code)]
fn propose_with_llm() {}
