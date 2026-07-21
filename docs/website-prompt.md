# Website prompt — the Blaze launch page

A ready-to-execute prompt for building the GitHub Pages site (`docs/index.html`).
Paste it into a Claude Code session in this repo (or run it directly). Every number in
it was measured in this repo's dev container — if you re-measure first with
`/bench-and-readme`, substitute the fresh figures before executing.

---

Build a single-page launch website for **Blaze — the live logic runtime**
(github.com/grloper/Blaze), served from GitHub Pages: one self-contained
`docs/index.html` on the default branch (all CSS/JS inline, no CDNs, no build
step, no external fonts). Target the caliber of a Linear/Vercel/Warp launch
page: dark-first, terminal-native, fast.

WHAT BLAZE IS
- An embeddable JIT-native scripting runtime that hot-swaps *functions* in a
  running production process in ~500 microseconds — provably safe, canaried
  against live traffic, instantly reversible. For humans and for AI agents.
- Hero quote: "LaunchDarkly flips booleans. Blaze swaps functions — in 500
  microseconds, with a proof, a canary, and an undo button. And it is how you
  let an AI touch production logic without fear."
- Mechanism: an incremental compiler (salsa query graph) where callers depend
  on callee *signatures*, never bodies — "the firewall." Every edit's blast
  radius is proven, and the swap protocol is derived from the proof:
  Rejected / NoEffect / SafeSwap (one atomic store) / Relink (quiesced commit).

REAL NUMBERS (measured on the repo's dev container — label them as such, and
never invent others):
- 496µs body-edit hot-swap under live HTTP load; 51,815 requests across 6 live
  edits, 0 dropped; scoring latency p50 64ns / p99 512ns
- ~50M calls/s/thread via FuncHandle (~188M on 4 threads); 51× an embedded
  interpreter (rhai) on the identical rule; reload medians: SafeSwap 0.74ms vs
  8.5ms full recompile
- Canary overhead: +~4ns/call idle or at 1% sampling (lock-free sampler)
- 110 tests; every claim traces to a named test that attacks it from a second
  thread

PAGE SECTIONS, IN ORDER
1. Hero — name, one-line value prop, the quote, two CTAs: a copy-to-clipboard
   `git clone … && cargo run -p blaze-jit --release --example living_service
   -- --script`, and "Read the proofs" anchoring to §7. Subtle CSS-only
   animated background; nothing heavy.
2. THE CENTERPIECE: an animated fake-terminal that auto-plays the six-beat
   living-service story on scroll into view (respect prefers-reduced-motion →
   show the final frame). Color the class badges: SafeSwap green, Rejected
   red, Relink cyan, canary amber. The beats, verbatim data:
     [1] body edit → SafeSwap, radius {velocity_risk}, 496µs, dropped 0
     [2] broken save → Rejected, 1 diagnostic, last-good serves on, dropped 0
     [3] runaway while(1) → canary traps FuelExhausted, auto-aborted, live untouched
     [4] risky rule → canary, 309 divergences observed, promoted as SafeSwap
     [5] ABI change → Relink, radius {velocity_risk, score}, 1.08ms, dropped 0
     [6] rollback to gen 1 → Relink, 1.28ms, values exact
   Terminal footer: "served 51,815 requests across 6 live edits, 0 dropped —
   all six guarantees asserted ✓"
3. How it can be safe — the firewall diagram (source → function_text →
   signature / lowered IR → machine code; "callers read the SIGNATURE, not
   the body"), then the four edit classes as cards with commit protocols.
4. The category — honest comparison table: LaunchDarkly (booleans, $20k+/yr) ·
   OPA/Rego (whole-bundle reload, no per-rule canary, no radius proof) ·
   sandbox/microVM execution (out-of-process, seconds) · dlopen/hot-patch
   (guessed diffs) · Blaze (functions, microseconds, in-process, proven
   radius, canary, rollback). Factual, not snarky.
5. For AI agents — horizontal pipeline: propose → gate → offline eval →
   canary on live traffic → promote/abort → metrics. Feature the killer
   scenario: a candidate that PASSED offline eval but loops forever on an
   input shape the eval set missed — caught by the canary before promotion.
   Pull-quote: "The blast radius of a bad idea is a shadow execution, not an
   outage."
6. Benchmarks — three compact tables (reload per class; throughput incl. the
   51× row; canary ns/call), each with its `cargo run` repro command and the
   machine caveat.
7. Claims → proofs — ~10 strongest claims, each row linking to its
   test-file::function on GitHub (blaze-jit/tests/…). Frame: "Every 'instant',
   'safe', and 'proven' on this page is a theorem with a test."
8. Honest limits — single file, i64/f64 only, host-owned state, retained
   generations, StateMigration reserved. Keep it; it builds trust.
9. Footer — Apache-2.0, GitHub link, `cargo test --workspace  # 110 tests`.

DESIGN BAR ("5 stars" means)
- Dark default, ember/flame-orange accent used sparingly; system font stack +
  true monospace for terminal/code; max-width ~1100px; flawless at 360px
  (terminal scales, tables collapse gracefully).
- Micro-interactions: copy buttons on every command, count-up on hero stats,
  hover on claim rows. No scroll-jacking; total JS a few KB.
- Semantic HTML, OG/Twitter meta ("Blaze — the live logic runtime"), inline
  SVG flame favicon, zero console errors, no horizontal scroll, Lighthouse
  ≥95 across the board.
- Every number on the page must come from the list above, sourced; no stock
  imagery, no fake logos, no testimonials.

DELIVERABLE: `docs/index.html`, committed with a note that enabling it is
Settings → Pages → Deploy from branch → main → /docs.
