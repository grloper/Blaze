//! The live demo: a terminal particle fountain whose physics is a `.blaze`
//! file, hot-swapped while it runs.
//!
//! ```sh
//! cargo run -p blaze-jit --example live            # watch examples/live.blaze
//! cargo run -p blaze-jit --example live -- --script  # self-driving demo (CI-safe)
//! ```
//!
//! In watch mode, open `blaze-jit/examples/live.blaze` in your editor and save
//! edits: every save is classified (SafeSwap / Relink / NoEffect) and applied
//! to the running simulation without restarting it — particle state (positions,
//! velocities) lives in this host and survives every reload.
//!
//! `--script` mode copies the script to a temp file and performs the edits
//! itself on a schedule, demonstrating all three edit classes end-to-end, then
//! exits. Useful for CI and for recording GIFs deterministically.

use std::io::Write as _;
use std::time::Duration;

use blaze_jit::{EditClass, ReloadReport, ScriptHost};

const W: i64 = 72; // columns
const H: i64 = 22; // rows
const SCALE: i64 = 1000; // fixed-point factor
const N_PARTICLES: usize = 90;

struct Particle {
    x: i64,
    y: i64,
    vx: i64,
    vy: i64,
}

/// Deterministic LCG so runs are reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
}

fn spawn(rng: &mut Rng) -> Particle {
    Particle {
        x: (W / 2) * SCALE + rng.range(-2000, 2000),
        y: (H - 2) * SCALE,
        vx: rng.range(-900, 900),
        vy: rng.range(-2600, -1400), // upward burst; gravity pulls back down
    }
}

fn class_tag(class: EditClass) -> &'static str {
    match class {
        EditClass::Rejected => "Rejected ",
        EditClass::NoEffect => "NoEffect ",
        EditClass::SafeSwap => "SafeSwap ",
        EditClass::Relink => "Relink   ",
        EditClass::StateMigration => "Migration",
    }
}

fn describe(report: &ReloadReport) -> String {
    if report.class == EditClass::Rejected {
        let first = report
            .diagnostics
            .first()
            .map(|(f, d)| format!("{f}: {}", d.message))
            .unwrap_or_default();
        return format!(
            "[gen {}] Rejected  {} problem(s), e.g. {first} — last-good still serving",
            report.generation,
            report.diagnostics.len(),
        );
    }
    let radius = if report.changed.is_empty() {
        "∅".to_string()
    } else {
        report.changed.join(", ")
    };
    format!(
        "[gen {}] {} radius {{{}}} in {:?} — state preserved",
        report.generation,
        class_tag(report.class),
        radius,
        report.latency,
    )
}

fn render(particles: &[Particle], tick: u64, status: &str, out: &mut impl std::io::Write) {
    let mut grid = vec![b' '; (W * H) as usize];
    for p in particles {
        let (col, row) = (p.x / SCALE, p.y / SCALE);
        if (0..W).contains(&col) && (0..H).contains(&row) {
            grid[(row * W + col) as usize] = b'*';
        }
    }
    let mut frame = String::with_capacity((W as usize + 3) * (H as usize + 4));
    frame.push_str("\x1b[H"); // cursor home; no full clear = no flicker
    frame.push_str(&format!(
        "  Blaze live fountain — tick {tick:<6} (edit the .blaze file; Ctrl-C quits)\x1b[K\n"
    ));
    frame.push_str(&format!("  {status}\x1b[K\n"));
    frame.push('+');
    frame.push_str(&"-".repeat(W as usize));
    frame.push_str("+\n");
    for row in 0..H {
        frame.push('|');
        let line = &grid[(row * W) as usize..((row + 1) * W) as usize];
        frame.push_str(std::str::from_utf8(line).unwrap());
        frame.push_str("|\n");
    }
    frame.push('+');
    frame.push_str(&"-".repeat(W as usize));
    frame.push_str("+\n");
    let _ = out.write_all(frame.as_bytes());
    let _ = out.flush();
}

/// One physics step for every particle, entirely through hot-swappable code.
fn step_world(host: &ScriptHost, particles: &mut [Particle], rng: &mut Rng, tick: u64) {
    let rt = host.runtime();
    for p in particles.iter_mut() {
        // Any of these calls may hit brand-new machine code the frame after a
        // save; the runtime guarantees each call is old-consistent or
        // new-consistent, never torn.
        p.vx = rt.call("step_vx", &[p.vx]).unwrap_or(p.vx);
        p.vy = rt.call("step_vy", &[p.vy, tick as i64]).unwrap_or(p.vy);
        p.x = rt.call("step_x", &[p.x, p.vx]).unwrap_or(p.x);
        p.y = rt.call("step_y", &[p.y, p.vy]).unwrap_or(p.y);

        // Walls and floor are host-side (host owns the state and the rules of
        // the world; the script owns the motion).
        if p.x < 0 {
            p.x = 0;
            p.vx = -p.vx * 7 / 10;
        }
        if p.x >= W * SCALE {
            p.x = W * SCALE - 1;
            p.vx = -p.vx * 7 / 10;
        }
        if p.y < 0 {
            p.y = 0;
            p.vy = -p.vy * 7 / 10;
        }
        if p.y >= H * SCALE {
            *p = spawn(rng);
        }
    }
}

/// One scripted edit: fire at `.0` ticks, described by `.1`, applied by `.2`.
type ScriptedEdit = (u64, &'static str, fn(&str) -> String);

/// The scripted edit schedule for `--script` mode.
fn scripted_edits() -> Vec<ScriptedEdit> {
    vec![
        (80, "flip gravity (body-only edit)", |src: &str| {
            src.replace("return 55;", "return -45;")
        }),
        (160, "add a comment (no semantic change)", |src: &str| {
            src.replace("int drag(int v) {", "int drag(int v) {\n    // slow down a touch\n")
        }),
        (240, "give gravity a time parameter (ABI change)", |src: &str| {
            src.replace("int gravity() {\n    return -45;\n}", "int gravity(int t) {\n    return 55 - t / 4;\n}")
                .replace("drag(vy + gravity())", "drag(vy + gravity(t))")
        }),
    ]
}

fn main() {
    let script_mode = std::env::args().any(|a| a == "--script");

    // Locate the source script relative to this crate.
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/live.blaze");

    // In script mode, work on a temp copy so the repo file is untouched.
    let watch_path = if script_mode {
        let tmp = std::env::temp_dir().join("blaze-live-demo.blaze");
        std::fs::copy(&source_path, &tmp).expect("copy demo script to temp");
        tmp
    } else {
        source_path
    };

    let mut host = ScriptHost::new(&watch_path).unwrap_or_else(|e| {
        eprintln!("failed to start: {e}");
        std::process::exit(1);
    });

    let mut rng = Rng(0xB1A2E);
    let mut particles: Vec<Particle> = (0..N_PARTICLES).map(|_| spawn(&mut rng)).collect();
    // Stagger initial ages so the fountain looks alive immediately.
    for (i, p) in particles.iter_mut().enumerate() {
        p.y -= (i as i64 % 14) * 900;
    }

    let mut status = format!(
        "loaded {} — edit classes will appear here as you save",
        watch_path.display()
    );
    let mut reports: Vec<ReloadReport> = Vec::new();
    let edits = scripted_edits();
    let mut next_edit = 0usize;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(b"\x1b[2J\x1b[?25l"); // clear once, hide cursor

    let max_ticks: u64 = if script_mode { 320 } else { u64::MAX };
    for tick in 0..max_ticks {
        // Apply a scheduled edit (script mode only).
        if script_mode && next_edit < edits.len() && tick == edits[next_edit].0 {
            let (_, what, edit) = &edits[next_edit];
            let src = std::fs::read_to_string(&watch_path).expect("read script");
            std::fs::write(&watch_path, edit(&src)).expect("write script");
            status = format!("editing: {what} ...");
            next_edit += 1;
        }

        // Poll the file; a change reloads through the incremental graph.
        match host.poll() {
            Ok(Some(report)) => {
                status = describe(&report);
                reports.push(report);
            }
            Ok(None) => {}
            Err(e) => status = format!("reload error: {e}"),
        }

        step_world(&host, &mut particles, &mut rng, tick);
        render(&particles, tick, &status, &mut out);
        std::thread::sleep(Duration::from_millis(if script_mode { 5 } else { 33 }));
    }

    let _ = out.write_all(b"\x1b[?25h\n"); // show cursor again
    if script_mode {
        println!("--- scripted run complete: {} reloads observed ---", reports.len());
        for r in &reports {
            println!("{}", describe(r));
        }
        let classes: Vec<EditClass> = reports.iter().map(|r| r.class).collect();
        assert_eq!(
            classes,
            vec![EditClass::SafeSwap, EditClass::NoEffect, EditClass::Relink],
            "the scripted edits must classify as SafeSwap, NoEffect, Relink"
        );
        println!("all edit classes verified: SafeSwap, NoEffect, Relink ✓");
    }
}
