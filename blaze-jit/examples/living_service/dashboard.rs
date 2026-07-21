//! The live TUI for the living-service demo: a flicker-free terminal dashboard
//! showing throughput, tail latency, the generation timeline, the last reload
//! report, and — when one is running — the canary's divergence tally. It reads
//! only the lock-free stats the service already keeps, so drawing it costs the
//! request path nothing.

use std::io::Write;
use std::time::Instant;

use blaze_jit::{CanaryStatus, CanaryVerdict, EditClass};

use super::service::{Service, StoryView};

/// One committed generation on the timeline, tagged with the edit class that
/// produced it (and, for a rejected save, a marker that no generation moved).
struct Milestone {
    generation: usize,
    class: EditClass,
}

/// Holds the small amount of state a live dashboard needs between frames:
/// the smoothed request rate and the generation timeline.
pub struct Dashboard {
    endpoint: String,
    /// Story mode narrates "beat N/6 …"; interactive mode shows the edit hint.
    interactive: bool,
    prev_requests: u64,
    prev_instant: Instant,
    req_per_s: f64,
    /// The last generation seen; `None` until the first frame establishes the
    /// baseline (so the initial load isn't drawn as an inbound transition).
    last_generation: Option<usize>,
    timeline: Vec<Milestone>,
}

impl Dashboard {
    pub fn new(endpoint: String, interactive: bool) -> Self {
        Dashboard {
            endpoint,
            interactive,
            prev_requests: 0,
            prev_instant: Instant::now(),
            req_per_s: 0.0,
            last_generation: None,
            timeline: Vec::new(),
        }
    }

    /// Clear the screen and hide the cursor. Call once before the first frame.
    pub fn enter(out: &mut impl Write) {
        let _ = out.write_all(b"\x1b[2J\x1b[?25l");
    }

    /// Show the cursor again. Call once after the last frame.
    pub fn leave(out: &mut impl Write) {
        let _ = out.write_all(b"\x1b[?25h\n");
    }

    /// Draw one frame from the current service stats and story view.
    pub fn render(&mut self, service: &Service, view: &StoryView, out: &mut impl Write) {
        self.sample_rate(service);
        self.track_generation(service, view);

        let served = service.stats.requests.load(std::sync::atomic::Ordering::Relaxed);
        let dropped = service.stats.errors.load(std::sync::atomic::Ordering::Relaxed);
        let p50 = service.stats.hist.percentile_nanos(0.50);
        let p99 = service.stats.hist.percentile_nanos(0.99);
        let canary = view.canary;

        let bar = "─".repeat(70);
        let mut f = String::with_capacity(2048);
        f.push_str("\x1b[H"); // home; per-line \x1b[K erases stale tails

        line(&mut f, &format!(
            "\x1b[1mBlaze — the live logic runtime\x1b[0m   risk service @ {}   (Ctrl-C quits)",
            self.endpoint
        ));
        if self.interactive {
            line(&mut f, "edit \x1b[1mrules.blaze\x1b[0m and save — every save is classified & applied live");
        } else {
            line(&mut f, &format!(
                "beat {}/{}: {}",
                view.beat_index + 1,
                view.total_beats,
                view.beat.title()
            ));
        }
        line(&mut f, &format!("\x1b[2m{}\x1b[0m", view.phase));
        line(&mut f, &bar);

        line(&mut f, &format!(
            "  throughput   \x1b[1m{:>9}\x1b[0m req/s      served {:>11}      dropped {}",
            thousands(self.req_per_s as u64),
            thousands(served),
            if dropped == 0 { "\x1b[32m0\x1b[0m".to_string() } else { format!("\x1b[31m{dropped}\x1b[0m") },
        ));
        line(&mut f, &format!(
            "  latency      p50 {:>8}     p99 {:>8}     (scoring compute)",
            fmt_nanos(p50),
            fmt_nanos(p99),
        ));
        line(&mut f, &format!(
            "  generation   \x1b[1mgen {}\x1b[0m",
            service.runtime().generation()
        ));
        line(&mut f, &format!("  last reload  {}", describe_report(view)));
        line(&mut f, &format!("  canary       {}", describe_canary(canary)));
        line(&mut f, &bar);
        line(&mut f, &format!("  timeline     {}", self.render_timeline()));

        f.push_str("\x1b[J"); // erase anything below
        let _ = out.write_all(f.as_bytes());
        let _ = out.flush();
    }

    /// Recompute the smoothed request rate roughly five times a second.
    fn sample_rate(&mut self, service: &Service) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.prev_instant).as_secs_f64();
        if elapsed >= 0.2 {
            let requests = service.stats.requests.load(std::sync::atomic::Ordering::Relaxed);
            self.req_per_s = (requests - self.prev_requests) as f64 / elapsed;
            self.prev_requests = requests;
            self.prev_instant = now;
        }
    }

    /// Append a milestone whenever a new generation commits (the initial load is
    /// the timeline's baseline, not a transition).
    fn track_generation(&mut self, service: &Service, view: &StoryView) {
        let generation = service.runtime().generation();
        match self.last_generation {
            None => self.last_generation = Some(generation),
            Some(last) if generation != last => {
                let class = view.last_report.map(|r| r.class).unwrap_or(EditClass::SafeSwap);
                self.timeline.push(Milestone { generation, class });
                self.last_generation = Some(generation);
            }
            _ => {}
        }
    }

    fn render_timeline(&self) -> String {
        if self.timeline.is_empty() {
            return "gen 1 ●".to_string();
        }
        let mut out = String::from("gen 1 ●");
        for m in &self.timeline {
            out.push_str(&format!(" ──{}──▶ \x1b[1m{}\x1b[0m", class_tag(m.class), m.generation));
        }
        out
    }
}

/// Push a line with an erase-to-end-of-line so shorter frames don't leave tails.
fn line(f: &mut String, text: &str) {
    f.push_str(text);
    f.push_str("\x1b[K\n");
}

fn describe_report(view: &StoryView) -> String {
    match view.last_report {
        None => "\x1b[2m(none yet)\x1b[0m".to_string(),
        Some(r) if r.class == EditClass::Rejected => format!(
            "\x1b[31mRejected\x1b[0m  {} diagnostic(s): {}",
            r.diagnostics.len(),
            r.diagnostics.first().map(|(fnname, d)| format!("{fnname}: {}", d.message)).unwrap_or_default(),
        ),
        Some(r) => {
            let radius = if r.changed.is_empty() { "∅".to_string() } else { r.changed.join(", ") };
            format!(
                "{}  radius {{{}}}  in {}",
                class_tag(r.class),
                radius,
                fmt_nanos(r.latency.as_nanos() as u64),
            )
        }
    }
}

fn describe_canary(status: Option<CanaryStatus>) -> String {
    match status {
        None => "\x1b[2m(idle — no candidate shadowing)\x1b[0m".to_string(),
        Some(s) => {
            let verdict = match s.verdict {
                CanaryVerdict::Running => "\x1b[33mshadowing\x1b[0m",
                CanaryVerdict::AbortedOnDivergence => "\x1b[31maborted: divergence\x1b[0m",
                CanaryVerdict::AbortedOnLatency => "\x1b[31maborted: latency\x1b[0m",
            };
            format!(
                "{verdict}  {} samples, {} diverged, {} faulted",
                s.samples, s.divergences, s.candidate_faults
            )
        }
    }
}

fn class_tag(class: EditClass) -> &'static str {
    match class {
        EditClass::Rejected => "\x1b[31mRejected\x1b[0m",
        EditClass::NoEffect => "\x1b[2mNoEffect\x1b[0m",
        EditClass::SafeSwap => "\x1b[32mSafeSwap\x1b[0m",
        EditClass::Relink => "\x1b[36mRelink\x1b[0m",
        EditClass::StateMigration => "Migration",
    }
}

/// Format a nanosecond count as µs/ms/s with one decimal (or ns below 1µs).
fn fmt_nanos(nanos: u64) -> String {
    if nanos < 1_000 {
        format!("{nanos}ns")
    } else if nanos < 1_000_000 {
        format!("{:.1}µs", nanos as f64 / 1_000.0)
    } else if nanos < 1_000_000_000 {
        format!("{:.1}ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{:.2}s", nanos as f64 / 1_000_000_000.0)
    }
}

/// Group a number with thousands separators.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}
