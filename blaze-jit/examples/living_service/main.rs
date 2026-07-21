//! **The demo: a living service under fire.**
//!
//! A risk-scoring HTTP service whose scoring logic is a Blaze program, embedded
//! through `FuncHandle`s and hot-swapped while thousands of requests per second
//! pour through it — safely, provably, and with an undo button.
//!
//! ```sh
//! # Watch mode: serve on a loopback port, edit rules.blaze, watch it apply live.
//! cargo run -p blaze-jit --release --example living_service
//!
//! # Scripted mode: the six-beat story, self-driving, every beat asserted (CI/GIF).
//! cargo run -p blaze-jit --release --example living_service -- --script
//! ```
//!
//! The scripted story, in order — each beat applied under live load, each
//! asserted (see `service::assert_story`):
//!
//!   1. a body edit          → `SafeSwap`, mid-load, zero dropped calls
//!   2. a broken save        → `Rejected`, the last-good rules keep serving
//!   3. a runaway `while(1)`  → the canary traps `FuelExhausted`, auto-aborts
//!   4. a risky rule change  → canary, divergence report, then promote
//!   5. an internal ABI edit → `Relink` under load, zero dropped calls
//!   6. a rollback to gen 1  → the original rules, back in microseconds
//!
//! Watch mode uses the same file-watching mechanism as [`blaze_jit::ScriptHost`]
//! (mtime polling → `reload`), applied to a runtime shared across the acceptor
//! pool so the reload lands live for every connection at once.

mod dashboard;
mod http;
// Shared scaffolding: the example and the CI test each use a subset of this
// module (the example the `demo` profile and the dashboard fields; the test the
// `test` profile), so some items are unused in either single build.
#[allow(dead_code)]
mod service;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use blaze_jit::ReloadReport;

use dashboard::Dashboard;
use service::{Beat, RunConfig, Service, StepResult, StoryView};

/// How many keep-alive HTTP clients to point at the service.
const LOAD_CLIENTS: usize = 8;

fn main() {
    let script_mode = std::env::args().any(|a| a == "--script");
    let result = if script_mode { run_script() } else { run_interactive() };
    if let Err(e) = result {
        eprintln!("living_service: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Scripted mode — the CI-asserted, GIF-recordable story
// ---------------------------------------------------------------------------

fn run_script() -> Result<(), String> {
    let service = Service::new(service::RULES)?;
    let addr = http::serve(service.clone()).map_err(|e| format!("bind failed: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let load = http::spawn_load(addr, LOAD_CLIENTS, stop.clone());

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    Dashboard::enter(&mut out);
    let mut dash = Dashboard::new(format!("http://{addr}"), false);

    // Drive the story over real HTTP traffic, animating the dashboard each tick.
    let cfg = RunConfig::demo();
    let results = service::run_story(&service, &cfg, &mut |svc, view| {
        dash.render(svc, view, &mut out);
    });

    Dashboard::leave(&mut out);
    stop.store(true, Ordering::Relaxed);
    for h in load {
        let _ = h.join();
    }

    print_summary(&service, &results);
    // The theorem: every beat did exactly what the firewall promised. Panics
    // (non-zero exit) if any guarantee was violated — this is the CI gate.
    service::assert_story(&results);
    println!("\n\x1b[1;32mall six guarantees asserted ✓\x1b[0m");
    Ok(())
}

/// Print a quotable, plain-text summary of the scripted run.
fn print_summary(service: &Service, results: &[StepResult]) {
    let served = service.stats.requests.load(Ordering::Relaxed);
    let dropped = service.stats.errors.load(Ordering::Relaxed);
    println!("\n━━━ Blaze living-service — scripted run under load ━━━");
    println!(
        "served {} requests across 6 live edits, {} dropped\n",
        commas(served),
        dropped
    );
    for step in results {
        println!("  {}", summarize_beat(step));
    }
    if let Some(last) = results.last() {
        println!(
            "\nsteady-state scoring latency: p50 {}, p99 {} (in-process compute)",
            fmt_ns(last.p50_nanos),
            fmt_ns(last.p99_nanos),
        );
    }
}

fn summarize_beat(step: &StepResult) -> String {
    let prefix = format!("[{}] {}", step.beat as usize + 1, step.beat.title());
    let detail = match step.beat {
        Beat::BodyEdit | Beat::AbiRelink | Beat::Rollback => {
            let r = step.report.as_ref().unwrap();
            let radius = if r.changed.is_empty() { "∅".into() } else { r.changed.join(", ") };
            format!("{:?}  radius {{{}}}  in {}", r.class, radius, fmt_us(r.latency))
        }
        Beat::BrokenSave => {
            let r = step.report.as_ref().unwrap();
            format!(
                "{:?}  {} diagnostic(s); last-good served on",
                r.class,
                r.diagnostics.len()
            )
        }
        Beat::RunawayCanary => {
            let c = step.canary.unwrap();
            format!(
                "canary trapped FuelExhausted ({} fault(s)); auto-aborted; live untouched",
                c.candidate_faults
            )
        }
        Beat::RiskyCanary => {
            let c = step.canary.unwrap();
            let r = step.report.as_ref().unwrap();
            format!(
                "canary saw {} divergence(s); promoted as {:?}",
                c.divergences, r.class
            )
        }
    };
    format!(
        "{prefix:<48}  {detail}   ({} reqs, dropped {})",
        commas(step.served),
        step.dropped
    )
}

// ---------------------------------------------------------------------------
// Watch mode — serve, edit rules.blaze, watch it apply live
// ---------------------------------------------------------------------------

fn run_interactive() -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/living_service/rules.blaze");
    let source = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let service = Service::new(&source)?;
    let addr = http::serve(service.clone()).map_err(|e| format!("bind failed: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let _load = http::spawn_load(addr, LOAD_CLIENTS, stop.clone());

    // A file-watcher thread: the ScriptHost mechanism (mtime poll → reload)
    // applied to the shared runtime, so a save lands live for every connection.
    let last_report: Arc<Mutex<Option<ReloadReport>>> = Arc::new(Mutex::new(None));
    spawn_watcher(service.clone(), path, last_report.clone(), stop.clone());

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    Dashboard::enter(&mut out);
    let mut dash = Dashboard::new(format!("http://{addr}"), true);

    // Render until Ctrl-C. (Ctrl-C terminates the process; the OS reclaims the
    // detached server/load/watch threads.)
    loop {
        let report = last_report.lock().unwrap().clone();
        let view = StoryView {
            beat: Beat::BodyEdit,
            beat_index: 0,
            total_beats: 6,
            phase: "serving live — try: curl the endpoint, then edit rules.blaze",
            last_report: report.as_ref(),
            canary: service.runtime().canary_status(),
        };
        dash.render(&service, &view, &mut out);
        std::thread::sleep(Duration::from_millis(33));
    }
}

/// Poll `path`'s mtime and reload the shared runtime on change, recording the
/// latest report for the dashboard.
fn spawn_watcher(
    service: Arc<Service>,
    path: PathBuf,
    last_report: Arc<Mutex<Option<ReloadReport>>>,
    stop: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let mut last_modified: Option<SystemTime> = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            if modified == last_modified {
                continue;
            }
            last_modified = modified;
            if let Ok(source) = std::fs::read_to_string(&path) {
                if let Ok(report) = service.runtime().reload(&source) {
                    *last_report.lock().unwrap() = Some(report);
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn fmt_us(d: Duration) -> String {
    let nanos = d.as_nanos() as u64;
    if nanos < 1_000_000 {
        format!("{:.0}µs", nanos as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", nanos as f64 / 1_000_000.0)
    }
}

fn fmt_ns(nanos: u64) -> String {
    if nanos < 1_000 {
        format!("{nanos}ns")
    } else if nanos < 1_000_000 {
        format!("{:.1}µs", nanos as f64 / 1_000.0)
    } else {
        format!("{:.2}ms", nanos as f64 / 1_000_000.0)
    }
}

fn commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
