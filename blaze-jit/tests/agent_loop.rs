//! CI gate for the `agent_loop` example (P4).
//!
//! The example's `main` drives the whole propose → gate → offline-eval → canary
//! → promote/abort pipeline and asserts the outcome (the runaway is caught by
//! the canary and never promoted; the target metric is reached; the guardrail
//! holds). Running it here makes that a `cargo test` gate, not just something you
//! have to run by hand — so the "AI agents safely evolving live logic" story
//! can't silently rot.

#[allow(dead_code)]
#[path = "../examples/agent_loop.rs"]
mod agent_loop;

#[test]
fn the_agent_pipeline_keeps_every_bad_edit_out_of_production() {
    // `main` panics if any guarantee is violated; reaching the end is the pass.
    agent_loop::main();
}
