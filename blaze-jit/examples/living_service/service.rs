//! The socket-free heart of the living-service demo, shared verbatim by the
//! runnable example (`examples/living_service/main.rs`) and the CI gate
//! (`tests/living_service.rs`).
//!
//! It embeds a Blaze program as the scoring logic of a request handler, and
//! encodes the six-beat story the demo tells under sustained load:
//!
//!   1. a body edit         → `SafeSwap`, mid-load, zero dropped calls
//!   2. a broken save       → `Rejected`, last-good keeps serving
//!   3. a runaway `while(1)` → the *canary* catches `FuelExhausted`, auto-aborts
//!   4. a risky rule change → canary, divergence report, then promote
//!   5. an ABI change       → `Relink` under load, zero dropped calls
//!   6. a rollback to gen 1 → the original rules, back in microseconds
//!
//! Every beat is *asserted* ([`assert_story`]) against the exact scores the
//! rules must produce and the exact edit class the firewall must prove — so the
//! story is a theorem, not a screenshot. The transport (real HTTP over
//! `TcpListener`) and the dashboard live in the example; the test drives the
//! same [`run_story`] with an in-process load generator so the guarantees are
//! hammered from many threads without any socket flakiness.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use blaze_jit::{
    CallError, CanaryPolicy, CanaryStatus, CanaryVerdict, EditClass, FuncHandle, LiveRuntime,
    ReloadReport,
};

// ---------------------------------------------------------------------------
// The rules, and every version of them the story installs
// ---------------------------------------------------------------------------

/// Generation 1: the risk model the service launches with.
///
/// `score` is the entry point the host calls on every request; it keeps its
/// `(int, int, int) -> int` signature through *every* edit below, which is what
/// lets the request handler hold one `FuncHandle` across the whole story and
/// never drop a call. The interesting churn — new weights, a widened rule, an
/// internal ABI change — all happens *behind* that stable entry point.
pub const RULES: &str = "\
// Risk scoring for a card-authorization request.
//   amount   — transaction size, in cents
//   velocity — the account's transactions in the last hour
//   age      — the account's age, in days
// Returns a risk score; the host declines the charge above its threshold.

int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity) {
    return velocity * 5;
}

int age_risk(int age) {
    if (age < 30) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// Beat 1 — a body-only edit: retune the velocity weight 5 → 8. No signature
/// moves, so the firewall proves `SafeSwap`: one atomic pointer store.
pub const RULES_V2_WEIGHT: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity) {
    return velocity * 8;
}

int age_risk(int age) {
    if (age < 30) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// Beat 2 — a broken save: `velcity` is a typo. The diagnostics gate proves the
/// undefined variable and refuses the edit; the last-good generation keeps
/// serving. (Held only in the story, never installed.)
pub const RULES_BROKEN: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity) {
    return velcity * 8;
}

int age_risk(int age) {
    if (age < 30) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// Beat 3 — a runaway: `velocity_risk` loops forever. Compiles clean (it is a
/// valid program), so only a *canary* can catch it — the shadow burns the
/// inherited fuel budget, traps `FuelExhausted`, and auto-aborts. (Canaried,
/// never promoted.)
pub const RULES_RUNAWAY: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity) {
    int i = 0;
    while (i >= 0) {
        i = i + 1;
    }
    return velocity * 8;
}

int age_risk(int age) {
    if (age < 30) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// Beat 4 — a risky but intended change: widen the "new account" window
/// 30 → 90 days. It diverges from live for accounts aged 30..90, so the canary
/// reports divergences; the operator judges them intended and promotes.
pub const RULES_V3_AGE: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity) {
    return velocity * 8;
}

int age_risk(int age) {
    if (age < 90) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity) + age_risk(age);
}
";

/// Beat 5 — an ABI change *behind* the entry point: `velocity_risk` gains a
/// `cap` parameter and `score` passes it. `score`'s own signature is untouched,
/// so the graph pulls exactly `{velocity_risk, score}` into the blast radius and
/// commits the pair atomically as a `Relink` — while the handler keeps calling
/// the same 3-arg `score` throughout, dropping nothing.
pub const RULES_V4_CAP: &str = "\
int amount_risk(int amount) {
    if (amount > 100000) {
        return 45;
    }
    if (amount > 10000) {
        return 20;
    }
    return 0;
}

int velocity_risk(int velocity, int cap) {
    int r = velocity * 8;
    if (r > cap) {
        return cap;
    }
    return r;
}

int age_risk(int age) {
    if (age < 90) {
        return 25;
    }
    return 0;
}

int score(int amount, int velocity, int age) {
    return amount_risk(amount) + velocity_risk(velocity, 50) + age_risk(age);
}
";

/// The per-call fuel budget the service runs under. The rules are loop-free, so
/// legitimate scoring spends a handful of units; this is tight enough that a
/// runaway `while(1)` candidate (beat 3) traps in well under a millisecond.
pub const SERVICE_FUEL: u64 = 200_000;

// ---------------------------------------------------------------------------
// The reference model — what each generation *must* compute
// ---------------------------------------------------------------------------

/// One scoring request.
#[derive(Debug, Clone, Copy)]
pub struct Inputs {
    pub amount: i64,
    pub velocity: i64,
    pub age: i64,
}

impl Inputs {
    pub fn args(&self) -> [i64; 3] {
        [self.amount, self.velocity, self.age]
    }
}

/// Which generation of the rules is live — used only by the test oracle to say
/// what `score` *should* return, so a wrong hot-swap is caught immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The launch rules (also the rollback target): velocity ×5, age < 30.
    Gen1,
    /// After beat 1: velocity ×8, age < 30.
    Weight,
    /// After beat 4: velocity ×8, age < 90.
    Age,
    /// After beat 5: velocity ×8 capped at 50, age < 90.
    Cap,
}

/// The reference risk model, in Rust — the oracle every beat is asserted
/// against. Kept deliberately in lockstep with the `.blaze` sources above.
pub fn expected(stage: Stage, inp: Inputs) -> i64 {
    let amount_risk = if inp.amount > 100_000 {
        45
    } else if inp.amount > 10_000 {
        20
    } else {
        0
    };
    let velocity_risk = match stage {
        Stage::Gen1 => inp.velocity * 5,
        Stage::Weight | Stage::Age => inp.velocity * 8,
        Stage::Cap => (inp.velocity * 8).min(50),
    };
    let age_cutoff = match stage {
        Stage::Gen1 | Stage::Weight => 30,
        Stage::Age | Stage::Cap => 90,
    };
    let age_risk = if inp.age < age_cutoff { 25 } else { 0 };
    amount_risk + velocity_risk + age_risk
}

/// Probe requests chosen to pin down exactly which generation is live: they
/// straddle every threshold that moves across the story (the $ tiers, the age
/// window at 30 and 90, the velocity cap at 50).
pub const PROBES: [Inputs; 4] = [
    Inputs { amount: 5_000, velocity: 3, age: 40 },
    Inputs { amount: 50_000, velocity: 4, age: 60 },
    Inputs { amount: 120_000, velocity: 10, age: 20 },
    Inputs { amount: 50_000, velocity: 10, age: 60 },
];

// ---------------------------------------------------------------------------
// Live stats — lock-free, read by the dashboard, asserted by the story
// ---------------------------------------------------------------------------

/// Number of latency buckets: bucket `i` covers `[2^i, 2^(i+1))` nanoseconds,
/// so the range spans 1 ns … ~18 minutes — comfortably past anything a scoring
/// call or a stalled swap could take.
const HIST_BUCKETS: usize = 40;

/// A lock-free latency histogram with power-of-two nanosecond buckets. Recording
/// a sample is one relaxed atomic add; percentiles are read off the hot path.
pub struct LatencyHist {
    buckets: Vec<AtomicU64>,
}

impl Default for LatencyHist {
    fn default() -> Self {
        LatencyHist { buckets: (0..HIST_BUCKETS).map(|_| AtomicU64::new(0)).collect() }
    }
}

impl LatencyHist {
    fn record(&self, nanos: u64) {
        let bucket = (64 - nanos.max(1).leading_zeros() as usize - 1).min(HIST_BUCKETS - 1);
        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
    }

    /// The lower edge, in nanoseconds, of the bucket holding the `p`-th
    /// percentile (0.0..=1.0). Coarse by construction (power-of-two buckets) —
    /// a dashboard signal, not a billing meter.
    pub fn percentile_nanos(&self, p: f64) -> u64 {
        let counts: Vec<u64> = self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        let total: u64 = counts.iter().sum();
        if total == 0 {
            return 0;
        }
        let target = (total as f64 * p).ceil() as u64;
        let mut cumulative = 0;
        for (i, c) in counts.iter().enumerate() {
            cumulative += c;
            if cumulative >= target {
                return 1u64 << i;
            }
        }
        1u64 << (HIST_BUCKETS - 1)
    }
}

/// Everything the request path records and the dashboard reads — all lock-free.
#[derive(Default)]
pub struct Stats {
    /// Scoring calls attempted.
    pub requests: AtomicU64,
    /// Scoring calls that returned `Err` — the "dropped" count the story pins to
    /// zero across every swap. The firewall's guarantee, made observable.
    pub errors: AtomicU64,
    /// Scoring-call latency distribution (the compute cost, sans transport).
    pub hist: LatencyHist,
}

// ---------------------------------------------------------------------------
// The service
// ---------------------------------------------------------------------------

/// A live risk-scoring service: a Blaze program behind a request handler, plus
/// the live stats and the canary switch. Shared (`Arc`) across every acceptor
/// and load-generator thread; the story thread reloads/canaries/rolls-back the
/// same `runtime` concurrently, which is the whole point.
pub struct Service {
    runtime: Arc<LiveRuntime>,
    pub stats: Stats,
    /// When set, the handler routes through `call_canary` (mirroring a sampled
    /// fraction through the shadow) instead of the bare `FuncHandle` fast path.
    /// The story flips this on only while a canary is live.
    canary_mode: AtomicBool,
}

impl Service {
    /// Stand up the service on `source`, under the tight service fuel budget.
    pub fn new(source: &str) -> Result<Arc<Self>, String> {
        let runtime = Arc::new(LiveRuntime::new(source)?);
        runtime.set_fuel_budget(SERVICE_FUEL);
        Ok(Arc::new(Service {
            runtime,
            stats: Stats::default(),
            canary_mode: AtomicBool::new(false),
        }))
    }

    /// The embedded runtime — for reloads, canaries, rollbacks, and metrics.
    pub fn runtime(&self) -> &Arc<LiveRuntime> {
        &self.runtime
    }

    /// Resolve the scoring entry point once, for a thread's hot serving loop.
    pub fn score_handle(&self) -> Result<FuncHandle, CallError> {
        self.runtime.handle("score")
    }

    /// Whether the handler is currently mirroring through a canary.
    pub fn canary_mode(&self) -> bool {
        self.canary_mode.load(Ordering::Acquire)
    }

    /// Serve one scoring request. This is the hot path a real deployment runs
    /// millions of times: the lock-free `FuncHandle` fast path normally, or the
    /// canary-mirroring path while a shadow is being evaluated. Either way the
    /// caller receives the *live* score; a candidate's answer never leaks.
    ///
    /// The returned score is recorded into the live stats; an `Err` (which the
    /// stable entry-point ABI makes impossible across the story's swaps, and
    /// which is the assertion) counts as a dropped call.
    pub fn handle_score(&self, handle: &mut FuncHandle, inp: Inputs) -> Result<i64, CallError> {
        let args = inp.args();
        let start = Instant::now();
        let result = if self.canary_mode() {
            // The request entry point while a canary may be running: returns the
            // live answer, shadows the candidate on a sampled fraction.
            self.runtime.call_canary("score", &args)
        } else {
            // The ordinary fast path: an arity check, one atomic load, an
            // indirect call — no lock, no lookup.
            self.runtime.call_handle(handle, &args)
        };
        let nanos = start.elapsed().as_nanos() as u64;
        self.stats.requests.fetch_add(1, Ordering::Relaxed);
        if result.is_err() {
            self.stats.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.stats.hist.record(nanos);
        result
    }
}

// ---------------------------------------------------------------------------
// The story
// ---------------------------------------------------------------------------

/// How the story paced itself and rendered — the test uses a fast, silent
/// config; the example a smooth, drawn one.
pub struct RunConfig {
    /// Calls to let flow before the first beat (warm the handles, fill the
    /// histogram).
    pub warmup_requests: u64,
    /// Calls to let flow through each beat's observation window before asserting.
    pub window_requests: u64,
    /// Delay between render ticks while waiting. `ZERO` busy-waits (the test).
    pub frame: Duration,
}

impl RunConfig {
    /// The test profile: large windows for a hard concurrency hammering, no
    /// frame delay.
    pub fn test() -> Self {
        RunConfig { warmup_requests: 5_000, window_requests: 20_000, frame: Duration::ZERO }
    }

    /// The example profile: smaller windows and a frame delay, so the dashboard
    /// animates at a watchable pace.
    pub fn demo() -> Self {
        RunConfig {
            warmup_requests: 2_000,
            window_requests: 6_000,
            frame: Duration::from_millis(33),
        }
    }
}

/// The six beats, in order, for labelling and for the dashboard timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    BodyEdit,
    BrokenSave,
    RunawayCanary,
    RiskyCanary,
    AbiRelink,
    Rollback,
}

impl Beat {
    pub fn title(self) -> &'static str {
        match self {
            Beat::BodyEdit => "body edit → SafeSwap, under load",
            Beat::BrokenSave => "broken save → Rejected, last-good serves on",
            Beat::RunawayCanary => "runaway while(1) → canary traps FuelExhausted",
            Beat::RiskyCanary => "risky rule → canary, divergence report, promote",
            Beat::AbiRelink => "ABI change → Relink, under load",
            Beat::Rollback => "rollback to gen 1, in microseconds",
        }
    }
}

/// The recorded, asserted outcome of one beat.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub beat: Beat,
    /// The reload report, if the beat performed a reload/promote/rollback.
    pub report: Option<ReloadReport>,
    /// The canary comparison, if the beat ran a canary.
    pub canary: Option<CanaryStatus>,
    /// Dropped (errored) scoring calls observed *during this beat's window*.
    pub dropped: u64,
    /// Scoring calls served during this beat's window.
    pub served: u64,
    /// The generation live after this beat.
    pub generation: usize,
    /// p50 / p99 scoring latency (ns) sampled at the end of the beat.
    pub p50_nanos: u64,
    pub p99_nanos: u64,
    /// The live `score` for each [`PROBES`] request, sampled at the *moment*
    /// this beat ended — captured here because the live program keeps changing,
    /// so the oracle must compare against what was live *then*, not at the end
    /// of the run.
    pub probe_scores: Vec<Result<i64, CallError>>,
}

/// A snapshot the render callback gets each tick, so a dashboard can draw the
/// live state without reaching into the story's internals.
pub struct StoryView<'a> {
    pub beat: Beat,
    pub beat_index: usize,
    pub total_beats: usize,
    /// A one-line human phase label ("steady", "reloading", "canary shadowing…").
    pub phase: &'a str,
    pub last_report: Option<&'a ReloadReport>,
    pub canary: Option<CanaryStatus>,
}

/// Spin until `stats.requests` has advanced by `delta`, rendering each tick.
/// Returns even if load stalls, after a generous safety timeout, so a wedged
/// demo can never hang forever.
#[allow(clippy::too_many_arguments)]
fn drain(
    service: &Service,
    cfg: &RunConfig,
    delta: u64,
    view_beat: Beat,
    view_index: usize,
    total: usize,
    phase: &str,
    last_report: Option<&ReloadReport>,
    render: &mut dyn FnMut(&Service, &StoryView),
) {
    let target = service.stats.requests.load(Ordering::Relaxed) + delta;
    let deadline = Instant::now() + Duration::from_secs(20);
    while service.stats.requests.load(Ordering::Relaxed) < target && Instant::now() < deadline {
        let view = StoryView {
            beat: view_beat,
            beat_index: view_index,
            total_beats: total,
            phase,
            last_report,
            canary: service.runtime.canary_status(),
        };
        render(service, &view);
        if cfg.frame.is_zero() {
            std::hint::spin_loop();
        } else {
            std::thread::sleep(cfg.frame);
        }
    }
}

/// Snapshot the current p50/p99 and dropped/served deltas into a `StepResult`.
fn finish_step(
    service: &Service,
    beat: Beat,
    report: Option<ReloadReport>,
    canary: Option<CanaryStatus>,
    served_from: u64,
    dropped_from: u64,
) -> StepResult {
    // Sample every probe against the program *as it is live right now* — this
    // beat's generation — so the oracle in `assert_story` compares against the
    // state that was actually serving during the beat.
    let probe_scores = PROBES.iter().map(|inp| service.runtime.call("score", &inp.args())).collect();
    StepResult {
        beat,
        report,
        canary,
        dropped: service.stats.errors.load(Ordering::Relaxed) - dropped_from,
        served: service.stats.requests.load(Ordering::Relaxed) - served_from,
        generation: service.runtime.generation(),
        p50_nanos: service.stats.hist.percentile_nanos(0.50),
        p99_nanos: service.stats.hist.percentile_nanos(0.99),
        probe_scores,
    }
}

/// Drive the whole six-beat story against a *live, loaded* service, returning
/// the per-beat results for [`assert_story`] to check.
///
/// The caller must already have load flowing (in-process threads for the test,
/// socket clients for the example) — every beat is applied while that traffic
/// hammers `score` from other threads. `render` is invoked many times per beat
/// so a dashboard can animate; pass a no-op for a silent run.
pub fn run_story(
    service: &Service,
    cfg: &RunConfig,
    render: &mut dyn FnMut(&Service, &StoryView),
) -> Vec<StepResult> {
    let rt = service.runtime().clone();
    let mut results = Vec::new();
    let total = 6;

    // Warm up: let the load generator resolve its handles and fill the
    // histogram before we start perturbing the world.
    drain(service, cfg, cfg.warmup_requests, Beat::BodyEdit, 0, total, "warming up", None, render);

    // ---- Beat 1: a body edit hot-swaps as SafeSwap, mid-load. ----
    {
        let (s0, d0) = snapshot(service);
        let report = rt.reload(RULES_V2_WEIGHT).expect("body edit must reload");
        drain(service, cfg, cfg.window_requests, Beat::BodyEdit, 0, total, "SafeSwap committed",
              Some(&report), render);
        results.push(finish_step(service, Beat::BodyEdit, Some(report), None, s0, d0));
    }

    // ---- Beat 2: a broken save is Rejected; last-good serves on. ----
    {
        let (s0, d0) = snapshot(service);
        let report = rt.reload(RULES_BROKEN).expect("a rejected edit still returns a report");
        drain(service, cfg, cfg.window_requests, Beat::BrokenSave, 1, total, "Rejected — holding last-good",
              Some(&report), render);
        results.push(finish_step(service, Beat::BrokenSave, Some(report), None, s0, d0));
    }

    // ---- Beat 3: a runaway while(1), caught by the canary as FuelExhausted. ----
    {
        let (s0, d0) = snapshot(service);
        service.canary_mode.store(true, Ordering::Release);
        rt.canary(RULES_RUNAWAY, CanaryPolicy::default()).expect("runaway candidate compiles");
        // Wait for the shadow to trap and auto-abort (or a bounded window).
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let st = rt.canary_status();
            let done = st.map(|s| s.verdict != CanaryVerdict::Running).unwrap_or(true);
            drain(service, cfg, 200, Beat::RunawayCanary, 2, total, "canary shadowing the runaway",
                  None, render);
            if done || Instant::now() >= deadline {
                break;
            }
        }
        let status = rt.canary_status();
        rt.abort_canary();
        service.canary_mode.store(false, Ordering::Release);
        results.push(finish_step(service, Beat::RunawayCanary, None, status, s0, d0));
    }

    // ---- Beat 4: a risky change — canary, observe divergence, promote. ----
    {
        let (s0, d0) = snapshot(service);
        service.canary_mode.store(true, Ordering::Release);
        // Lenient on divergence: this change is *meant* to differ, so we let it
        // run and lean on the fault/latency guards, then judge the divergence
        // report by hand. Sample 1-in-8 so live p99 barely moves under the
        // mirroring cost while the shadow gathers evidence.
        let policy =
            CanaryPolicy { max_divergences: u64::MAX, sample_every: 8, ..Default::default() };
        rt.canary(RULES_V3_AGE, policy).expect("age-window candidate compiles");
        // Let enough traffic mirror that divergences accumulate.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let enough = rt.canary_status().map(|s| s.samples >= 300).unwrap_or(false);
            drain(service, cfg, 400, Beat::RiskyCanary, 3, total, "canary shadowing the new rule",
                  None, render);
            if enough || Instant::now() >= deadline {
                break;
            }
        }
        let status = rt.canary_status();
        let report = rt.promote().expect("a healthy candidate promotes");
        service.canary_mode.store(false, Ordering::Release);
        drain(service, cfg, cfg.window_requests, Beat::RiskyCanary, 3, total, "promoted",
              Some(&report), render);
        results.push(finish_step(service, Beat::RiskyCanary, Some(report), status, s0, d0));
    }

    // ---- Beat 5: an internal ABI change is a Relink, under load. ----
    {
        let (s0, d0) = snapshot(service);
        let report = rt.reload(RULES_V4_CAP).expect("abi edit must reload");
        drain(service, cfg, cfg.window_requests, Beat::AbiRelink, 4, total, "Relink committed",
              Some(&report), render);
        results.push(finish_step(service, Beat::AbiRelink, Some(report), None, s0, d0));
    }

    // ---- Beat 6: rollback to generation 1, in microseconds. ----
    {
        let (s0, d0) = snapshot(service);
        let report = rt.rollback(1).expect("rollback to gen 1");
        drain(service, cfg, cfg.window_requests, Beat::Rollback, 5, total, "rolled back to gen 1",
              Some(&report), render);
        results.push(finish_step(service, Beat::Rollback, Some(report), None, s0, d0));
    }

    results
}

fn snapshot(service: &Service) -> (u64, u64) {
    (
        service.stats.requests.load(Ordering::Relaxed),
        service.stats.errors.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// The assertions — this is what makes the story a theorem
// ---------------------------------------------------------------------------

/// Assert every beat did exactly what the firewall promises, and that the live
/// program computes the exact reference scores at each stage. Panics on the
/// first violation with a message naming the beat — so the CI test and the
/// example's `--script` mode share one source of truth.
pub fn assert_story(results: &[StepResult]) {
    assert_eq!(results.len(), 6, "the story has six beats");

    // A generous "it committed promptly, it did not stall" ceiling. The precise
    // sub-millisecond / microsecond reload numbers are a *release* property and
    // are published by `bench_reload`; this bound only proves the swap is a
    // hot-swap and not a restart, and holds even in a debug build under load.
    const COMMITTED_PROMPTLY: Duration = Duration::from_millis(100);

    // Beat 1 — SafeSwap, radius {velocity_risk}, zero dropped, sub-millisecond.
    let b1 = &results[0];
    let r1 = b1.report.as_ref().expect("beat 1 reloads");
    assert_eq!(r1.class, EditClass::SafeSwap, "beat 1: a body edit is a SafeSwap");
    assert_eq!(r1.changed, vec!["velocity_risk".to_string()], "beat 1: radius is the one edited fn");
    assert_eq!(b1.dropped, 0, "beat 1: zero dropped calls across a SafeSwap under load");
    assert!(r1.latency < COMMITTED_PROMPTLY, "beat 1: swap latency {:?}", r1.latency);
    assert_scores(b1, Stage::Weight, "after beat 1");

    // Beat 2 — Rejected, diagnostics present, generation frozen, last-good serves.
    let b2 = &results[1];
    let r2 = b2.report.as_ref().expect("beat 2 reports");
    assert_eq!(r2.class, EditClass::Rejected, "beat 2: a broken save is Rejected");
    assert!(!r2.diagnostics.is_empty(), "beat 2: the rejection carries diagnostics");
    assert_eq!(b2.generation, b1.generation, "beat 2: a rejected edit commits no generation");
    assert_eq!(b2.dropped, 0, "beat 2: last-good drops nothing while a bad save is refused");
    assert_scores(b2, Stage::Weight, "after beat 2 — last-good is still beat 1's rules");

    // Beat 3 — the canary trapped the runaway; live is untouched; no promote.
    let b3 = &results[2];
    let c3 = b3.canary.expect("beat 3 ran a canary");
    assert!(c3.candidate_faults >= 1, "beat 3: the shadow trapped FuelExhausted at least once");
    assert_eq!(c3.verdict, CanaryVerdict::AbortedOnDivergence, "beat 3: the canary auto-aborted");
    assert_eq!(b3.dropped, 0, "beat 3: the shield keeps the live answer flowing, zero dropped");
    assert_scores(b3, Stage::Weight, "after beat 3 — the runaway never touched live");

    // Beat 4 — the canary saw divergence; promotion is a classified SafeSwap.
    let b4 = &results[3];
    let c4 = b4.canary.expect("beat 4 ran a canary");
    assert!(c4.divergences > 0, "beat 4: the intended change diverged, and we saw it");
    let r4 = b4.report.as_ref().expect("beat 4 promotes");
    assert_eq!(r4.class, EditClass::SafeSwap, "beat 4: promoting the age-window edit is a SafeSwap");
    assert_eq!(b4.dropped, 0, "beat 4: zero dropped across canary + promote");
    assert_scores(b4, Stage::Age, "after beat 4 — the widened age window is live");

    // Beat 5 — an internal ABI change is a Relink of exactly {velocity_risk, score}.
    let b5 = &results[4];
    let r5 = b5.report.as_ref().expect("beat 5 reloads");
    assert_eq!(r5.class, EditClass::Relink, "beat 5: an internal signature change is a Relink");
    let mut radius = r5.changed.clone();
    radius.sort();
    assert_eq!(radius, vec!["score".to_string(), "velocity_risk".to_string()],
               "beat 5: the graph pulls the caller into the radius");
    assert_eq!(b5.dropped, 0, "beat 5: zero dropped across a Relink under load");
    assert!(r5.latency < COMMITTED_PROMPTLY, "beat 5: relink latency {:?}", r5.latency);
    assert_scores(b5, Stage::Cap, "after beat 5 — the velocity cap is live");

    // Beat 6 — rollback to gen 1: classified, fast, exact, zero dropped.
    let b6 = &results[5];
    let r6 = b6.report.as_ref().expect("beat 6 rolls back");
    assert!(matches!(r6.class, EditClass::Relink | EditClass::SafeSwap),
            "beat 6: a rollback is an ordinary classified swap, not a special path");
    assert_eq!(b6.dropped, 0, "beat 6: zero dropped across the rollback");
    assert!(r6.latency < COMMITTED_PROMPTLY, "beat 6: rollback latency {:?}", r6.latency);
    assert_scores(b6, Stage::Gen1, "after beat 6 — the original rules are back, exactly");
}

/// Assert the scores this beat captured match the reference model for `stage` on
/// every probe. This is the "values revert/advance exactly" guarantee, checked
/// against what was live *during* the beat.
fn assert_scores(step: &StepResult, stage: Stage, when: &str) {
    for (inp, got) in PROBES.iter().zip(step.probe_scores.iter()) {
        assert_eq!(
            got,
            &Ok(expected(stage, *inp)),
            "{when}: score({}, {}, {}) must match the {stage:?} model",
            inp.amount, inp.velocity, inp.age,
        );
    }
}
