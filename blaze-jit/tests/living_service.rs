//! The living-service story, as a CI gate (P4).
//!
//! This drives the *exact* six-beat story the `living_service` example tells —
//! body edit, broken save, runaway canary, risky canary, ABI relink, rollback —
//! but with an in-process load generator hammering `score` from several threads
//! the whole time, and with no sockets, so the guarantees are tested hard and
//! deterministically. The story and its assertions live in the example's shared
//! core; the test just supplies the concurrent load and runs it.
//!
//! | Claim under load | Beat |
//! |---|---|
//! | A body edit hot-swaps as `SafeSwap`, zero dropped calls | 1 |
//! | A broken save is `Rejected`; last-good keeps serving | 2 |
//! | A runaway `while(1)` is caught by the canary as a fault, auto-aborted | 3 |
//! | A risky change canaries, its divergence is seen, then it promotes | 4 |
//! | An internal ABI change is a `Relink`, zero dropped calls | 5 |
//! | A rollback to gen 1 is a fast classified swap, values exact | 6 |

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

// The shared core carries presentation fields (dashboard timeline, phase
// labels, p50/p99) the runnable example reads but this test doesn't — it only
// hammers load and checks the guarantees. Allow that here.
#[allow(dead_code)]
#[path = "../examples/living_service/service.rs"]
mod service;

use service::{Inputs, Service};

/// A deterministic little LCG, so the load is reproducible run to run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo) as u64) as i64
    }
    /// A request spread wide enough to straddle every threshold the story moves:
    /// the two amount tiers, the age window (30 and 90), and the velocity cap.
    fn request(&mut self) -> Inputs {
        Inputs {
            amount: self.range(0, 200_000),
            velocity: self.range(0, 12),
            age: self.range(0, 120),
        }
    }
}

/// Spawn `n` load-generator threads, each resolving its own handle and hammering
/// `score` with random requests until `stop` is set. Returns their join handles.
fn spawn_load(service: &Arc<Service>, n: usize, stop: &Arc<AtomicBool>) -> Vec<thread::JoinHandle<()>> {
    (0..n)
        .map(|i| {
            let service = service.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                let mut handle = service.score_handle().expect("score exists at launch");
                let mut rng = Rng(0x5EED ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15));
                while !stop.load(Ordering::Relaxed) {
                    // The result is validated inside the story's assertions; here
                    // we just keep the pressure on. A dropped call would show up
                    // in `stats.errors`, which the story pins to zero.
                    let _ = service.handle_score(&mut handle, rng.request());
                }
            })
        })
        .collect()
}

#[test]
fn the_living_service_story_holds_under_load() {
    let service = Service::new(service::RULES).expect("the service compiles and starts");

    let stop = Arc::new(AtomicBool::new(false));
    let load = spawn_load(&service, 4, &stop);

    // Drive all six beats while four threads hammer `score`. Every beat is
    // asserted inside `run_story` / `assert_story`: the edit class the firewall
    // proved, the exact scores each generation computes, and zero dropped calls
    // across every swap.
    let cfg = service::RunConfig::test();
    let results = service::run_story(&service, &cfg, &mut |_, _| {});
    service::assert_story(&results);

    stop.store(true, Ordering::Relaxed);
    for h in load {
        h.join().expect("no load thread may panic — a panic means a dropped or wrong call");
    }

    // A final, whole-run invariant: not one of the (many) calls served across
    // the entire story was ever dropped. Every swap, reject, canary, and
    // rollback happened under live fire without a single failed request.
    assert_eq!(
        service.stats.errors.load(Ordering::Relaxed),
        0,
        "the entire story served without dropping a call"
    );
    assert!(
        service.stats.requests.load(Ordering::Relaxed) > 20_000,
        "the load generator kept real pressure on throughout"
    );
}
