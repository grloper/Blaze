//! The `salsa` database: inputs, interned keys, the [`BlazeDatabase`] trait, and
//! a concrete implementation with an opt-in execution tracer.
//!
//! Modern `salsa` (the 2022 rewrite, published as `salsa` 0.28) models a query
//! group as a set of `#[salsa::tracked]` free functions over a database trait —
//! there is no `#[salsa::query_group]` macro anymore. The blueprint's
//! `BlazeDatabase` trait survives as the `#[salsa::db]` trait below; its derived
//! queries live in [`crate::lower`].

use std::sync::{Arc, Mutex};

/// The whole-file source input — the coarse "raw source" the frontend feeds in.
///
/// Fine-grained, per-function incrementality is *derived* from this single input
/// by the tracked queries, which slice out and memoize each function
/// independently (see [`crate::lower`]).
#[salsa::input]
pub struct SourceProgram {
    /// The full text of the translation unit.
    #[returns(ref)]
    pub text: String,
}

/// An interned function name, used as a stable, `Copy` query key.
///
/// Interning is idempotent: `FnKey::new(db, "add")` always yields the same
/// handle, so two queries keyed by the same name share memoized results.
#[salsa::interned]
pub struct FnKey<'db> {
    #[returns(ref)]
    pub name: String,
}

/// Native functions the embedding host has registered, by name → arity.
///
/// A `salsa` input (like [`SourceProgram`]) rather than plain runtime state, so
/// the diagnostics gate (`crate::diag`) can validate calls to host functions
/// exactly like calls to Blaze-defined ones — unknown-callee and arity checks
/// see one unified namespace. One instance exists per database, updated via
/// its `salsa::Setter` whenever the host registers a new function.
#[salsa::input]
pub struct HostFunctions {
    #[returns(ref)]
    pub arities: std::collections::BTreeMap<String, usize>,
}

/// A shared, opt-in log of which query *bodies* actually executed.
///
/// Disabled by default (`None`), so production databases pay nothing. Tests
/// enable it to prove the incremental firewall: after an edit, the callee's
/// `lowered_dev_ir` must re-execute while the caller's stays memoized.
#[derive(Clone, Default)]
pub struct ExecTrace {
    inner: Arc<Mutex<Option<Vec<String>>>>,
}

impl ExecTrace {
    /// Record that a query body ran. No-op while tracing is disabled.
    pub fn record(&self, label: impl Into<String>) {
        if let Some(log) = self.inner.lock().unwrap().as_mut() {
            log.push(label.into());
        }
    }

    /// Begin capturing execution labels.
    pub fn enable(&self) {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_none() {
            *guard = Some(Vec::new());
        }
    }

    /// Drain and return the labels recorded since the last drain.
    pub fn take(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
}

/// Databases that expose an [`ExecTrace`]. Split out from [`BlazeDatabase`] so
/// the tracing hook can be a default trait method (mirroring `salsa`'s own
/// `HasLogger`/`LogDatabase` test infrastructure).
pub trait HasExecTrace {
    fn exec_trace(&self) -> &ExecTrace;
}

/// The Blaze query database. Any `salsa` database that can trace execution is a
/// `BlazeDatabase`; the derived queries in [`crate::lower`] take `&dyn BlazeDatabase`.
#[salsa::db]
pub trait BlazeDatabase: HasExecTrace + salsa::Database {
    /// Note that a query body executed (used only when tracing is enabled).
    fn record_exec(&self, label: String) {
        self.exec_trace().record(label);
    }
}

#[salsa::db]
impl<DB: HasExecTrace + salsa::Database> BlazeDatabase for DB {}

/// The default, ready-to-use Blaze database.
#[salsa::db]
#[derive(Clone, Default)]
pub struct BlazeDatabaseImpl {
    storage: salsa::Storage<Self>,
    trace: ExecTrace,
}

#[salsa::db]
impl salsa::Database for BlazeDatabaseImpl {}

impl HasExecTrace for BlazeDatabaseImpl {
    fn exec_trace(&self) -> &ExecTrace {
        &self.trace
    }
}

impl BlazeDatabaseImpl {
    /// Turn on execution tracing and hand back the shared trace handle so a test
    /// can inspect exactly which queries re-ran after an edit.
    pub fn enable_tracing(&self) -> ExecTrace {
        self.trace.enable();
        self.trace.clone()
    }
}
