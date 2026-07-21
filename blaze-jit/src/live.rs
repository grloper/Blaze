//! The **live-swap runtime**: hot reload that is correct by construction.
//!
//! Every call between Blaze functions is compiled as an indirect call through a
//! process-stable **slot table** (one `mmap`'d atomic pointer per function).
//! Reloading therefore never rewrites existing code — it compiles the changed
//! functions into a fresh generation of executable pages and atomically
//! repoints their slots.
//!
//! What makes the reload *sound* rather than hopeful is that the swap strategy
//! is derived from the `salsa` query graph, not guessed:
//!
//!  * The **blast radius** of an edit is exactly the set of functions whose
//!    `lowered_dev_ir` changed. The graph's firewall guarantees a body-only
//!    edit keeps every caller's DevIR — and therefore its machine code —
//!    byte-identical, so patching the edited function's single slot is globally
//!    consistent. That is [`EditClass::SafeSwap`]: one atomic store, zero
//!    pause, valid under concurrent execution.
//!  * When a signature changes, the graph *forces* every caller into the blast
//!    radius (callers depend on callee signatures). The runtime then recompiles
//!    the whole radius and commits it under a quiescence barrier so no thread
//!    can ever observe a caller and callee with mismatched ABIs. That is
//!    [`EditClass::Relink`] — still crash-free, never a silent corruption.
//!
//! A naive reloader patches pointers and hopes the ABI didn't change. Blaze
//! *knows*, because the incremental graph proves it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlagsData, Signature as ClifSig, UserFuncName, Value as ClifValue,
};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa};
use cranelift_codegen::ir::types;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};

use blaze_ir::db::{BlazeDatabaseImpl, ExecTrace, FnKey, HostFunctions, SourceProgram};
use blaze_ir::diag::{format_diagnostics, program_diagnostics};
use blaze_ir::lower::{lowered_dev_ir, program_outline};
use blaze_ir::{Diagnostic, FunctionId, FunctionNode, Signature, Type};
use salsa::Setter;

use crate::abi::{self, CallState, MAX_ARITY, TRAP_FUEL, TRAP_STACK};
use crate::codegen::{build_body, clif_signature, host_isa, CallEmitter, DEFAULT_MAX_DEPTH};

/// Maximum number of distinct functions (source + host) a runtime can hold.
const TABLE_CAPACITY: usize = 1024;

/// Target of any slot whose function is missing (never defined, or removed by
/// an edit). Takes the hidden context pointer like every other slot target and
/// returns a defined `0`, so even a stray call lands somewhere harmless.
///
/// With the H1 diagnostics gate in place this is unreachable through the public
/// API (an undefined callee is rejected before any generation is committed); it
/// remains as defense-in-depth for slots allocated but never assigned.
extern "C" fn missing_stub(_ctx: *mut CallState) -> i64 {
    0
}

// ---------------------------------------------------------------------------
// Slot table
// ---------------------------------------------------------------------------

/// Sentinel arity for a slot that holds no live function (never assigned, or
/// removed by an edit). No real function has this arity, so a cached handle
/// whose slot becomes empty always detects it.
const ARITY_EMPTY: u64 = u64::MAX;

/// A page-aligned, `mmap`-allocated array of atomic function-pointer slots,
/// with a parallel array of the arity currently compiled into each slot.
///
/// The *code* array's address is stable for the life of the runtime — generated
/// code bakes `&code[slot]` in as an absolute constant and performs an atomic
/// load on every call, which is what makes pointer hot-swapping a single
/// release-store. The *arity* array is read only by the runtime (never by
/// generated code) and is what lets [`FuncHandle`]'s lock-free fast path detect
/// an ABI change: a body swap leaves a slot's arity untouched (so live handles
/// keep working and pick up the new body for free), while a relink that changes
/// arity publishes the new arity *before* the new code, so a reader that
/// double-checks arity around the code load can never call code with a
/// mismatched argument count.
struct SwapTable {
    code: *mut AtomicU64,
    bytes: usize,
    /// Arity compiled into each slot, or [`ARITY_EMPTY`]. Boxed (not `mmap`'d):
    /// only the runtime touches it, never generated code.
    arity: Box<[AtomicU64]>,
}

// SAFETY: the table is a fixed allocation of atomics; all mutation goes through
// atomic operations.
unsafe impl Send for SwapTable {}
unsafe impl Sync for SwapTable {}

impl SwapTable {
    fn new() -> Result<Self, String> {
        let bytes = TABLE_CAPACITY * std::mem::size_of::<AtomicU64>();
        // SAFETY: anonymous private mapping, checked for MAP_FAILED below.
        let code = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if code == libc::MAP_FAILED {
            return Err("mmap of the swap table failed".to_string());
        }
        let arity = (0..TABLE_CAPACITY).map(|_| AtomicU64::new(ARITY_EMPTY)).collect();
        let table = SwapTable { code: code.cast::<AtomicU64>(), bytes, arity };
        // Every slot starts as the missing stub, so even a call emitted against
        // a never-defined function lands somewhere harmless.
        for i in 0..TABLE_CAPACITY {
            table.code(i).store(missing_stub as extern "C" fn(*mut CallState) -> i64 as usize as u64, Ordering::Release);
        }
        Ok(table)
    }

    #[inline]
    fn code(&self, index: usize) -> &AtomicU64 {
        assert!(index < TABLE_CAPACITY);
        // SAFETY: `code` points at TABLE_CAPACITY zero-initialized AtomicU64s,
        // properly aligned by mmap's page alignment; index is bounds-checked.
        unsafe { &*self.code.add(index) }
    }

    #[inline]
    fn arity(&self, index: usize) -> &AtomicU64 {
        &self.arity[index]
    }

    /// Absolute address of a slot's code pointer, for baking into generated code.
    #[inline]
    fn code_addr(&self, index: usize) -> usize {
        assert!(index < TABLE_CAPACITY);
        self.code as usize + index * std::mem::size_of::<AtomicU64>()
    }

    /// Publish a function into a slot: store its arity *before* its code
    /// (release-ordered), the order the lock-free reader in [`FuncHandle`]
    /// relies on to never observe new code under an old arity.
    #[inline]
    fn publish(&self, index: usize, arity: usize, code_ptr: u64) {
        self.arity(index).store(arity as u64, Ordering::Release);
        self.code(index).store(code_ptr, Ordering::Release);
    }

    /// Mark a slot empty (its function was removed): arity first, then a harmless
    /// stub. A handle to it will see [`ARITY_EMPTY`] and refresh.
    #[inline]
    fn clear(&self, index: usize) {
        self.arity(index).store(ARITY_EMPTY, Ordering::Release);
        self.code(index)
            .store(missing_stub as extern "C" fn(*mut CallState) -> i64 as usize as u64, Ordering::Release);
    }
}

impl Drop for SwapTable {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the region mapped in `new`.
        unsafe { libc::munmap(self.code.cast(), self.bytes) };
    }
}

// ---------------------------------------------------------------------------
// Table-indirect call emission
// ---------------------------------------------------------------------------

/// Emits every Blaze→Blaze call as `call_indirect` through the callee's slot:
/// an absolute-address constant, an atomic load, and an indirect call. No
/// relocations exist between functions, which is why generations never need
/// relinking against each other.
struct TableEmitter<'a> {
    call_conv: CallConv,
    table: &'a SwapTable,
    slots: &'a HashMap<FunctionId, usize>,
}

impl CallEmitter for TableEmitter<'_> {
    fn emit_call(
        &mut self,
        builder: &mut FunctionBuilder,
        ctx: ClifValue,
        callee: FunctionId,
        args: &[ClifValue],
    ) -> ClifValue {
        let slot_index = *self
            .slots
            .get(&callee)
            .expect("slot allocated for every callee before compilation");
        let slot_addr = self.table.code_addr(slot_index) as i64;

        let addr = builder.ins().iconst(types::I64, slot_addr);
        // Acquire-load pairs with the reloader's release-store, so a caller
        // that observes a new pointer also observes the fully written code
        // behind it.
        let target = builder.ins().atomic_load(types::I64, MemFlagsData::trusted(), addr);
        let sig_ref = builder.import_signature(clif_signature(args.len(), self.call_conv));
        // Thread the context pointer as the callee's hidden leading argument.
        let mut call_args = Vec::with_capacity(args.len() + 1);
        call_args.push(ctx);
        call_args.extend_from_slice(args);
        let call = builder.ins().call_indirect(sig_ref, target, &call_args);
        builder.inst_results(call)[0]
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// How an edit may be applied, proved from the query graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditClass {
    /// The query graph proved a defect before any generation was touched:
    /// a syntax error, a call to an undefined function, a call-site arity
    /// mismatch, an undeclared assignment, or a read of an undefined
    /// variable. Nothing is compiled, nothing is patched, and the previous
    /// generation keeps serving every in-flight and future call unchanged —
    /// see [`ReloadReport::diagnostics`] for what was wrong.
    Rejected,
    /// The edit changed no function's DevIR (formatting, comments, or code
    /// that lowers identically). Nothing is compiled; nothing is patched.
    NoEffect,
    /// Every changed function kept its ABI signature. The firewall guarantees
    /// no caller's code changed, so repointing the changed slots is globally
    /// consistent — applied atomically, without pausing execution.
    SafeSwap,
    /// At least one signature changed (or a function was removed). The graph
    /// pulls every affected caller into the blast radius; the whole set is
    /// recompiled and committed under a quiescence barrier so callers and
    /// callees can never be observed with mismatched ABIs.
    Relink,
    /// Reserved: a change to persistent state layout, requiring a data
    /// migration before code can be swapped. Blaze functions are currently
    /// pure over `i64`s — state lives in the host, which is exactly what makes
    /// state survive every reload — so this classification cannot yet occur.
    StateMigration,
}

/// What a [`LiveRuntime::reload`] did, and what it cost.
#[derive(Debug, Clone)]
pub struct ReloadReport {
    pub class: EditClass,
    /// Functions whose DevIR changed (the blast radius), in outline order.
    pub changed: Vec<String>,
    /// Functions that did not exist before this reload.
    pub added: Vec<String>,
    /// Functions removed by this reload (their slots now hit the missing stub).
    pub removed: Vec<String>,
    /// Wall-clock time from source swap to fully committed pointers.
    pub latency: Duration,
    /// Monotonic generation counter (0 is the initial load). Unchanged from
    /// the previous report when `class == Rejected`.
    pub generation: usize,
    /// Every diagnostic the query graph proved, attributed by function name.
    /// Non-empty if and only if `class == Rejected`.
    pub diagnostics: Vec<(String, Diagnostic)>,
}

/// One entry in the [`LiveRuntime`]'s reload journal — a durable record of a
/// single reload event and the program state it produced.
///
/// The journal is the runtime's audit log *and* the substrate for
/// [`LiveRuntime::rollback`]: because each committed entry retains the exact
/// source it installed, reverting to any past generation is just a reload of
/// that stored source, classified and committed by the same protocol as any
/// other edit. Every event is recorded — including `Rejected` (with its
/// diagnostics) and `NoEffect` — so the log is a faithful history, not only a
/// list of successes.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    /// Monotonic event index across all reload attempts (0 is the initial load).
    pub sequence: usize,
    /// The generation in effect after this event. A `Rejected` or `NoEffect`
    /// event commits nothing, so it carries the *previous* generation number;
    /// a committed event carries its own, unique, increasing number.
    pub generation: usize,
    /// When the event was recorded.
    pub at: SystemTime,
    /// The full program source as of this event — the snapshot `rollback`
    /// reinstalls. (Kept verbatim; live-logic sources are small and edits few.)
    pub source: String,
    /// How the event was classified.
    pub class: EditClass,
    /// The blast radius: functions whose DevIR changed.
    pub changed: Vec<String>,
    /// Functions this event added.
    pub added: Vec<String>,
    /// Functions this event removed.
    pub removed: Vec<String>,
    /// Diagnostics proved for this event (non-empty iff `class == Rejected`).
    pub diagnostics: Vec<(String, Diagnostic)>,
    /// Wall-clock time the event took.
    pub latency: Duration,
}

impl JournalEntry {
    /// Whether this event actually installed a generation (i.e. is a valid
    /// [`LiveRuntime::rollback`] target). `Rejected` and `NoEffect` did not.
    pub fn is_committed(&self) -> bool {
        !matches!(self.class, EditClass::Rejected | EditClass::NoEffect)
    }
}

/// A Blaze value crossing the host boundary: an `int` or a `float`.
///
/// Inside generated code every value is carried as a raw 64-bit pattern (see
/// [`crate::codegen`]); this enum is the one place those bits are given a type,
/// so a host can pass a `float` argument and read a real `f64` result back from
/// a `float`-returning function — e.g. a live risk score — rather than
/// bit-punning by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
}

impl Value {
    /// This value's Blaze type.
    #[inline]
    pub fn ty(self) -> Type {
        match self {
            Value::Int(_) => Type::Int,
            Value::Float(_) => Type::Float,
        }
    }

    /// The raw 64-bit ABI word this value is carried as.
    #[inline]
    fn to_abi(self) -> i64 {
        match self {
            Value::Int(i) => i,
            Value::Float(f) => f.to_bits() as i64,
        }
    }

    /// Re-interpret a raw ABI word as a value of type `ty`.
    #[inline]
    fn from_abi(bits: i64, ty: Type) -> Value {
        match ty {
            Type::Int => Value::Int(bits),
            Type::Float => Value::Float(f64::from_bits(bits as u64)),
        }
    }
}

/// Why a [`LiveRuntime::call`] could not run, or could not run to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    UnknownFunction(String),
    ArityMismatch { name: String, expected: usize, got: usize },
    /// A typed call ([`LiveRuntime::call_typed`]) passed an argument whose type
    /// does not match the function's declared parameter type.
    TypeMismatch { name: String, position: usize, expected: Type, got: Type },
    UnsupportedArity(usize),
    /// The call exceeded the call-nesting limit (runaway recursion). The call
    /// was aborted and the process/runtime left consistent; nothing faulted.
    ResourceExhausted,
    /// The call exhausted its execution budget (e.g. an infinite loop). The
    /// call was aborted deterministically; the runtime remains usable. (H3)
    FuelExhausted,
    /// A [`FuncHandle`] could not be reconciled with the current program (its
    /// function kept changing arity across repeated refresh attempts). Re-resolve
    /// it with [`LiveRuntime::handle`]. Does not occur in normal use.
    HandleStale,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::UnknownFunction(name) => write!(f, "unknown function `{name}`"),
            CallError::ArityMismatch { name, expected, got } => {
                write!(f, "`{name}` takes {expected} argument(s), got {got}")
            }
            CallError::TypeMismatch { name, position, expected, got } => {
                let ty = |t: &Type| match t {
                    Type::Int => "int",
                    Type::Float => "float",
                };
                write!(
                    f,
                    "`{name}` argument {} expects {}, got {}",
                    position + 1,
                    ty(expected),
                    ty(got)
                )
            }
            CallError::UnsupportedArity(n) => write!(f, "arity {n} exceeds the dispatch limit"),
            CallError::ResourceExhausted => {
                write!(f, "call aborted: exceeded the call-depth limit (runaway recursion)")
            }
            CallError::FuelExhausted => {
                write!(f, "call aborted: exhausted its execution budget")
            }
            CallError::HandleStale => {
                write!(f, "function handle is stale; re-resolve it with `handle()`")
            }
        }
    }
}

impl std::error::Error for CallError {}

/// A resolved reference to a function, for the lock-free fast call path.
///
/// Obtain one with [`LiveRuntime::handle`] and invoke it with
/// [`LiveRuntime::call_handle`]. A handle caches the function's slot and arity
/// so a call skips the name lookup and lock the string-keyed [`LiveRuntime::call`]
/// pays; it self-heals across hot-swaps (body edits are transparent, arity
/// changes trigger one refresh). Cheap to clone; hold one per thread.
#[derive(Debug, Clone)]
pub struct FuncHandle {
    name: String,
    slot: usize,
    arity: usize,
    /// The function's signature at resolve time, for the typed fast path
    /// ([`LiveRuntime::call_handle_typed`]). Refreshed alongside slot/arity when
    /// a relink changes the ABI.
    sig: Arc<Signature>,
}

impl FuncHandle {
    /// The function this handle refers to.
    pub fn name(&self) -> &str {
        &self.name
    }
}

// ---------------------------------------------------------------------------
// Per-function metrics
// ---------------------------------------------------------------------------

/// Lock-free execution counters for one function, indexed by its stable slot.
///
/// Updated on the call path *only while metrics are enabled* (a single relaxed
/// flag load gates every write), so the fast path pays nothing by default. Every
/// field is a plain relaxed atomic: counts can never be lost or torn under
/// concurrent callers, and collection scales with the call path itself — no lock
/// is ever taken to record a call.
#[derive(Debug, Default)]
struct FnMetrics {
    calls: AtomicU64,
    total_nanos: AtomicU64,
    faults: AtomicU64,
}

/// A point-in-time read of one function's execution counters
/// ([`LiveRuntime::metrics`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FnMetricsSnapshot {
    /// Calls that ran to a result or a trap while metrics were enabled.
    pub calls: u64,
    /// Summed wall-clock execution time across those calls, in nanoseconds.
    pub total_nanos: u64,
    /// Calls that aborted with a resource/fuel trap (a subset of `calls`).
    pub faults: u64,
}

impl FnMetricsSnapshot {
    /// Mean execution time per recorded call, in nanoseconds (0 if none).
    pub fn mean_nanos(&self) -> u64 {
        if self.calls == 0 {
            0
        } else {
            self.total_nanos / self.calls
        }
    }
}

// ---------------------------------------------------------------------------
// Canary
// ---------------------------------------------------------------------------

/// When a shadowed candidate should auto-abort itself.
///
/// A canary runs a candidate program as a *shadow* of the live one: a fraction
/// of calls are mirrored through both, and the candidate's result is compared
/// against the live answer — which is the only answer the caller ever sees. The
/// policy decides when the candidate has failed the comparison badly enough to
/// pull itself.
#[derive(Debug, Clone, Copy)]
pub struct CanaryPolicy {
    /// Mirror one call in every `sample_every` (1 = shadow every call). A
    /// deterministic 1-in-N sampler, so a test can pin exactly which calls are
    /// compared.
    pub sample_every: u64,
    /// Auto-abort once this many *divergences* are seen — a divergence being any
    /// call where the candidate's result (value or error) differs from the live
    /// one. `1` (the default) is a strict differential shield: abort on the
    /// first disagreement, the right setting for a change believed to preserve
    /// behavior (a refactor, an optimization). Raise it for a canary of an
    /// intended behavior change, which will instead lean on the fault/latency
    /// checks.
    pub max_divergences: u64,
    /// Don't judge latency until at least this many calls have been sampled (so
    /// a couple of cold-start outliers can't trip it).
    pub min_samples_for_latency: u64,
    /// After `min_samples_for_latency`, auto-abort if the candidate's summed
    /// latency exceeds the live version's by more than this factor.
    pub max_latency_ratio: f64,
}

impl Default for CanaryPolicy {
    fn default() -> Self {
        CanaryPolicy {
            sample_every: 1,
            max_divergences: 1,
            min_samples_for_latency: 50,
            max_latency_ratio: 3.0,
        }
    }
}

/// A canary's current verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryVerdict {
    /// Still shadowing; no policy threshold has tripped.
    Running,
    /// Auto-aborted: the candidate disagreed with the live version too often.
    AbortedOnDivergence,
    /// Auto-aborted: the candidate was too much slower than the live version.
    AbortedOnLatency,
}

/// A point-in-time read of a canary's comparison ([`LiveRuntime::canary_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanaryStatus {
    /// Calls mirrored through both versions so far.
    pub samples: u64,
    /// Sampled calls where the candidate's result differed from the live one.
    pub divergences: u64,
    /// Sampled calls where the candidate faulted but the live version did not
    /// (a subset of `divergences`).
    pub candidate_faults: u64,
    /// Summed live-version latency across sampled calls, in nanoseconds.
    pub primary_total_nanos: u64,
    /// Summed candidate latency across sampled calls, in nanoseconds.
    pub candidate_total_nanos: u64,
    /// Whether the canary is still running or has auto-aborted (and why).
    pub verdict: CanaryVerdict,
}

/// Verdict codes stored in `Canary::aborted` (0 = running).
const CANARY_RUNNING: u8 = 0;
const CANARY_DIVERGED: u8 = 1;
const CANARY_SLOW: u8 = 2;

/// A candidate program shadowed against the live one, plus the running tally of
/// how it has compared. All counters are relaxed atomics: mirroring records
/// them without any lock beyond the brief one that guards the candidate's
/// lifetime against a concurrent `promote`/`abort`.
struct Canary {
    /// The candidate, compiled as a fully isolated program (its own slot table
    /// and dispatch) so shadow execution can never touch the live pointers.
    program: LiveRuntime,
    policy: CanaryPolicy,
    /// The candidate's source — the snapshot `promote` reinstalls into the live
    /// program through the ordinary reload protocol.
    source: String,
    samples: AtomicU64,
    divergences: AtomicU64,
    candidate_faults: AtomicU64,
    primary_nanos: AtomicU64,
    candidate_nanos: AtomicU64,
    /// `CANARY_RUNNING`, or the verdict code once auto-aborted.
    aborted: AtomicU8,
}

impl Canary {
    fn new(program: LiveRuntime, policy: CanaryPolicy, source: String) -> Self {
        Canary {
            program,
            policy,
            source,
            samples: AtomicU64::new(0),
            divergences: AtomicU64::new(0),
            candidate_faults: AtomicU64::new(0),
            primary_nanos: AtomicU64::new(0),
            candidate_nanos: AtomicU64::new(0),
            aborted: AtomicU8::new(CANARY_RUNNING),
        }
    }

    fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed) != CANARY_RUNNING
    }

    /// Run the candidate as a shadow of one sampled call and fold the comparison
    /// into the tally. `primary` is the live answer already computed by the
    /// caller (never affected by this); `primary_nanos` is how long it took.
    fn observe(&self, name: &str, args: &[i64], primary: &Result<i64, CallError>, primary_nanos: u64) {
        let started = Instant::now();
        let candidate = self.program.call(name, args);
        let candidate_nanos = started.elapsed().as_nanos() as u64;

        self.samples.fetch_add(1, Ordering::Relaxed);
        self.primary_nanos.fetch_add(primary_nanos, Ordering::Relaxed);
        self.candidate_nanos.fetch_add(candidate_nanos, Ordering::Relaxed);
        if *primary != candidate {
            self.divergences.fetch_add(1, Ordering::Relaxed);
        }
        if candidate.is_err() && primary.is_ok() {
            self.candidate_faults.fetch_add(1, Ordering::Relaxed);
        }
        self.evaluate();
    }

    /// Check the tally against the policy and auto-abort if a threshold tripped.
    /// The first tripped reason wins (a later thread's `compare_exchange` fails
    /// harmlessly).
    fn evaluate(&self) {
        if self.is_aborted() {
            return;
        }
        if self.divergences.load(Ordering::Relaxed) >= self.policy.max_divergences {
            let _ = self.aborted.compare_exchange(
                CANARY_RUNNING,
                CANARY_DIVERGED,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
            return;
        }
        if self.samples.load(Ordering::Relaxed) >= self.policy.min_samples_for_latency {
            let primary = self.primary_nanos.load(Ordering::Relaxed) as f64;
            let candidate = self.candidate_nanos.load(Ordering::Relaxed) as f64;
            if primary > 0.0 && candidate > primary * self.policy.max_latency_ratio {
                let _ = self.aborted.compare_exchange(
                    CANARY_RUNNING,
                    CANARY_SLOW,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
        }
    }

    fn verdict(&self) -> CanaryVerdict {
        match self.aborted.load(Ordering::Relaxed) {
            CANARY_DIVERGED => CanaryVerdict::AbortedOnDivergence,
            CANARY_SLOW => CanaryVerdict::AbortedOnLatency,
            _ => CanaryVerdict::Running,
        }
    }

    fn status(&self) -> CanaryStatus {
        CanaryStatus {
            samples: self.samples.load(Ordering::Relaxed),
            divergences: self.divergences.load(Ordering::Relaxed),
            candidate_faults: self.candidate_faults.load(Ordering::Relaxed),
            primary_total_nanos: self.primary_nanos.load(Ordering::Relaxed),
            candidate_total_nanos: self.candidate_nanos.load(Ordering::Relaxed),
            verdict: self.verdict(),
        }
    }
}

// ---------------------------------------------------------------------------
// The runtime
// ---------------------------------------------------------------------------

/// Per-name dispatch data used by [`LiveRuntime::call`]. The signature is
/// `Arc`-shared so cloning an entry (on the typed call path) is pointer-cheap.
#[derive(Debug, Clone)]
struct DispatchEntry {
    slot: usize,
    arity: usize,
    sig: Arc<Signature>,
}

/// Mutable compilation state, serialized behind one mutex. The call path never
/// touches it.
struct LiveInner {
    db: BlazeDatabaseImpl,
    src: SourceProgram,
    /// Salsa input mirroring `host_fns` below, so the diagnostics gate can
    /// resolve calls to host functions through the same query graph it uses
    /// for Blaze-defined ones.
    host_functions: HostFunctions,
    isa: OwnedTargetIsa,
    /// Call-nesting limit baked into every generation's entry guards (H2).
    max_depth: u64,
    /// Dedicated module holding the `(ctx, args…) -> ret` trampolines that let
    /// host-registered native functions (which take no context pointer) be
    /// called through the same context-threading ABI as Blaze functions.
    trampolines: JITModule,
    /// DevIR snapshot of the previous generation, for blast-radius diffing.
    prev: HashMap<String, std::sync::Arc<FunctionNode>>,
    /// Stable slot assignment. A name keeps its slot for the runtime's life.
    slots: HashMap<FunctionId, usize>,
    next_slot: usize,
    /// Host-registered native functions: name → (arity, fn pointer).
    host_fns: HashMap<String, (usize, u64)>,
    /// Retired generations. Old code pages are deliberately kept alive for the
    /// life of the runtime: a concurrent caller may still be executing them
    /// mid-swap, and for a dev-loop tool the cost (old versions of *edited
    /// functions only*) is negligible by design.
    generations: Vec<JITModule>,
    generation: usize,
    /// Append-only log of every reload event, oldest first. The audit trail and
    /// the source of truth for [`LiveRuntime::rollback`].
    journal: Vec<JournalEntry>,
}

/// An embeddable, hot-swappable Blaze program.
///
/// `call` is safe under concurrency with `reload`: body-only edits commit with
/// a single atomic store per function, and ABI edits quiesce in-flight calls
/// (the dispatch lock) before committing the whole blast radius at once.
pub struct LiveRuntime {
    table: SwapTable,
    /// Read-held for the duration of every root call; write-held while
    /// committing a relink or a multi-function swap. This is the quiescence
    /// barrier that makes ABI transitions atomic from every caller's view.
    dispatch: RwLock<HashMap<String, DispatchEntry>>,
    /// Per-call fuel budget (H3), read on the hot path without locking. Every
    /// call/back-edge spends one unit; exhaustion aborts the call with
    /// [`CallError::FuelExhausted`]. `u64::MAX` disables metering.
    fuel_budget: AtomicU64,
    /// Whether per-function metrics are being collected. Off by default; a
    /// single relaxed load on the call path gates all metric writes.
    metrics_enabled: AtomicBool,
    /// Per-function execution counters, indexed by slot (stable for the life of
    /// the runtime, so a function's metrics survive body swaps and relinks).
    metrics: Box<[FnMetrics]>,
    /// Fast-path hint for [`Self::call_canary`]: `false` means no canary is
    /// active, so the shadow machinery (and its lock) is skipped entirely. The
    /// authoritative state is `canary` below; this is only an optimization, and
    /// a stale read merely delays a sample by one call.
    canary_active: AtomicBool,
    /// The 1-in-N canary sampler, kept *off* the canary's lock so an un-sampled
    /// call on the canary path costs two relaxed loads (rate + this counter's
    /// increment) instead of a mutex acquisition. `canary_rate` is the N, or `0`
    /// when no candidate should be sampled — either idle, or auto-aborted (the
    /// sampler is switched off the moment a shadow trips a policy threshold). The
    /// lock is taken *only* on the sampled fraction, to run the shadow. This is
    /// what makes "live p99 never moves more than the mirroring cost" literally
    /// true: the calls you don't mirror pay no lock at all.
    canary_rate: AtomicU64,
    canary_counter: AtomicU64,
    /// The active canary, if any: a candidate program shadowed against this one.
    /// `Box`ed to break the `LiveRuntime` → `Canary` → `LiveRuntime` type cycle.
    canary: Mutex<Option<Box<Canary>>>,
    inner: Mutex<LiveInner>,
}

/// Default per-call fuel budget: generous enough that real live-logic (which
/// scores a request in thousands of ops) never trips it, small enough that a
/// true runaway loop aborts in a fraction of a second. Tune per workload with
/// [`LiveRuntime::set_fuel_budget`] — a request handler wants a much tighter
/// budget than this safety net.
pub const DEFAULT_FUEL_BUDGET: u64 = 500_000_000;

impl LiveRuntime {
    /// Compile `source` and stand the program up, ready to call.
    pub fn new(source: &str) -> Result<Self, String> {
        let runtime = Self::assemble(source)?;
        {
            let mut inner = runtime.inner.lock().unwrap();
            // There is no "last-good" to hold on the very first load, so a
            // proven defect fails construction outright rather than being
            // reported as a held-open `Rejected` reload.
            let report = runtime.load_generation(&mut inner, None)?;
            if report.class == EditClass::Rejected {
                return Err(format_diagnostics(&report.diagnostics));
            }
        }
        Ok(runtime)
    }

    /// Build a runtime holding `source` but *not yet compiled* — every field
    /// initialized, no generation loaded. Shared by [`Self::new`] (which loads
    /// immediately) and the canary path (which registers host functions before
    /// the first load, so the diagnostics gate resolves them).
    fn assemble(source: &str) -> Result<Self, String> {
        let db = BlazeDatabaseImpl::default();
        let src = SourceProgram::new(&db, source.to_string());
        let host_functions = HostFunctions::new(&db, std::collections::BTreeMap::new());
        let isa = host_isa()?;
        let trampolines =
            JITModule::new(JITBuilder::with_isa(isa.clone(), default_libcall_names()));

        Ok(LiveRuntime {
            table: SwapTable::new()?,
            dispatch: RwLock::new(HashMap::new()),
            fuel_budget: AtomicU64::new(DEFAULT_FUEL_BUDGET),
            metrics_enabled: AtomicBool::new(false),
            metrics: (0..TABLE_CAPACITY).map(|_| FnMetrics::default()).collect(),
            canary_active: AtomicBool::new(false),
            canary_rate: AtomicU64::new(0),
            canary_counter: AtomicU64::new(0),
            canary: Mutex::new(None),
            inner: Mutex::new(LiveInner {
                db,
                src,
                host_functions,
                isa,
                max_depth: DEFAULT_MAX_DEPTH,
                trampolines,
                prev: HashMap::new(),
                slots: HashMap::new(),
                next_slot: 0,
                host_fns: HashMap::new(),
                generations: Vec::new(),
                generation: 0,
                journal: Vec::new(),
            }),
        })
    }

    /// Build a candidate program for a canary: same source-loading path as
    /// [`Self::new`], but the primary's host functions are registered *before*
    /// the first load so a candidate that calls them is not spuriously rejected.
    /// A candidate that is genuinely defective returns its diagnostics as `Err`.
    ///
    /// The candidate is stood up under the primary's *current* resource limits —
    /// its call-depth limit (baked into the entry guards at compile time) and its
    /// per-call fuel budget — so the shadow evaluates the candidate under exactly
    /// the production conditions it would face if promoted. A candidate whose new
    /// logic recurses or loops away thus traps on the *same* budget the live
    /// program enforces, and the divergence is caught here rather than after it
    /// ships.
    fn new_candidate(
        source: &str,
        host_fns: &HashMap<String, (usize, u64)>,
        max_depth: u64,
        fuel_budget: u64,
    ) -> Result<Self, String> {
        let runtime = Self::assemble(source)?;
        // Fuel is read per call, so setting it now covers every shadow call.
        runtime.fuel_budget.store(fuel_budget, Ordering::Relaxed);
        // The depth limit is compiled into the entry guards, so it must be set
        // *before* the load below builds the candidate's code.
        runtime.inner.lock().unwrap().max_depth = max_depth;
        for (name, (arity, ptr)) in host_fns {
            // SAFETY: each pointer came from a valid registration on the primary
            // and must (by that call's contract) outlive the primary — which
            // owns this candidate — so it is valid for the candidate's life too.
            unsafe { runtime.register_host_fn(name, *arity, *ptr as *const u8) };
        }
        {
            let mut inner = runtime.inner.lock().unwrap();
            let report = runtime.load_generation(&mut inner, None)?;
            if report.class == EditClass::Rejected {
                return Err(format_diagnostics(&report.diagnostics));
            }
        }
        Ok(runtime)
    }

    /// Register a native function callable from Blaze code as `name(...)`.
    ///
    /// Takes effect immediately: any Blaze call site targeting `name` (current
    /// or from future reloads) dispatches to `ptr` through the slot table.
    ///
    /// # Safety
    ///
    /// `ptr` must be an `extern "C"` function taking exactly `arity` `i64`
    /// parameters and returning `i64`, and must remain valid for the life of
    /// the runtime. It must not call back into this runtime.
    pub unsafe fn register_host_fn(&self, name: &str, arity: usize, ptr: *const u8) {
        let mut inner = self.inner.lock().unwrap();
        let id = blaze_ir::function_id(&inner.db, name);
        let slot = Self::ensure_slot(&mut inner, id);
        inner.host_fns.insert(name.to_string(), (arity, ptr as u64));

        // Mirror into the salsa input so the diagnostics gate resolves calls
        // to this host function exactly like calls to a Blaze-defined one.
        let mut arities = inner.host_functions.arities(&inner.db).clone();
        arities.insert(name.to_string(), arity);
        inner.host_functions.set_arities(&mut inner.db).to(arities);

        // Compile a trampoline that accepts (and ignores) the hidden context
        // pointer, then tail-calls the real host function, so host functions
        // are invoked through the same ABI as Blaze functions.
        let trampoline = compile_host_trampoline(&mut inner.trampolines, arity, ptr);
        self.table.publish(slot, arity, trampoline as u64);
        // Host functions speak the C ABI the embedder registered them with:
        // `(int × arity) -> int`. That is their signature for typed dispatch.
        let sig = Arc::new(Signature { params: vec![Type::Int; arity], ret: Type::Int });
        self.dispatch
            .write()
            .unwrap()
            .insert(name.to_string(), DispatchEntry { slot, arity, sig });
    }

    /// Swap the program's source. Classifies the edit from the query graph,
    /// recompiles exactly the blast radius, and commits it with the weakest
    /// synchronization that is still sound for that class.
    pub fn reload(&self, new_source: &str) -> Result<ReloadReport, String> {
        let started = Instant::now();
        let mut inner = self.inner.lock().unwrap();

        let src = inner.src;
        src.set_text(&mut inner.db).to(new_source.to_string());

        let report = self.load_generation(&mut inner, Some(started))?;
        Ok(report)
    }

    /// Invoke `name` with `args`. Safe to call from any thread, concurrently
    /// with reloads.
    ///
    /// Runaway recursion and (once fuel is enabled) runaway loops abort the
    /// *call* with [`CallError::ResourceExhausted`] / [`CallError::FuelExhausted`]
    /// instead of faulting the process; the runtime stays fully usable and
    /// subsequent calls are unaffected.
    pub fn call(&self, name: &str, args: &[i64]) -> Result<i64, CallError> {
        // Root-call read lock: held while Blaze code runs, so a Relink commit
        // (write lock) can never interleave with an in-flight call.
        let dispatch = self.dispatch.read().unwrap();
        let entry = dispatch
            .get(name)
            .ok_or_else(|| CallError::UnknownFunction(name.to_string()))?;
        if entry.arity != args.len() {
            return Err(CallError::ArityMismatch {
                name: name.to_string(),
                expected: entry.arity,
                got: args.len(),
            });
        }
        if args.len() > MAX_ARITY {
            return Err(CallError::UnsupportedArity(args.len()));
        }

        let code = self.table.code(entry.slot).load(Ordering::Acquire) as usize as *const u8;
        // SAFETY: `code` is a finalized function compiled with the
        // context-threading `(*mut CallState, i64 × arity)` signature; the arity
        // was checked against the dispatch table, updated atomically under the
        // same lock held here.
        unsafe { self.run(entry.slot, code, args) }
    }

    /// Invoke `name` with *typed* arguments, returning a typed result.
    ///
    /// Where [`Self::call`] speaks the raw `i64` ABI, this validates each
    /// argument's type against the function's declared signature and decodes the
    /// result into [`Value::Int`] or [`Value::Float`] — the path a host uses to
    /// call a `float`-returning function (e.g. a risk score) without bit-punning
    /// by hand. It runs on the same sound footing as [`Self::call`]: safe under
    /// concurrent reloads (the dispatch read lock is the quiescence barrier), and
    /// resource/fuel traps surface as typed errors.
    pub fn call_typed(&self, name: &str, args: &[Value]) -> Result<Value, CallError> {
        let dispatch = self.dispatch.read().unwrap();
        let entry = dispatch
            .get(name)
            .ok_or_else(|| CallError::UnknownFunction(name.to_string()))?;
        if entry.arity != args.len() {
            return Err(CallError::ArityMismatch {
                name: name.to_string(),
                expected: entry.arity,
                got: args.len(),
            });
        }
        if args.len() > MAX_ARITY {
            return Err(CallError::UnsupportedArity(args.len()));
        }
        // Validate and encode each argument against its declared parameter type.
        let mut raw = [0i64; MAX_ARITY];
        for (i, (v, pty)) in args.iter().zip(entry.sig.params.iter()).enumerate() {
            if v.ty() != *pty {
                return Err(CallError::TypeMismatch {
                    name: name.to_string(),
                    position: i,
                    expected: *pty,
                    got: v.ty(),
                });
            }
            raw[i] = v.to_abi();
        }
        let ret_ty = entry.sig.ret;
        let slot = entry.slot;
        let code = self.table.code(slot).load(Ordering::Acquire) as usize as *const u8;
        // SAFETY: as in `call` — finalized context-threading code, arity checked
        // under the same lock; the raw words carry each value's exact ABI bits.
        let bits = unsafe { self.run(slot, code, &raw[..args.len()]) }?;
        Ok(Value::from_abi(bits, ret_ty))
    }

    /// Allocate this call's state, invoke `code`, record metrics for `slot` (if
    /// enabled), and translate a resource trap into a typed error. Shared by
    /// [`Self::call`], [`Self::call_typed`], and [`Self::call_handle`].
    ///
    /// # Safety
    ///
    /// `code` must point at a finalized function compiled with the
    /// context-threading signature for exactly `args.len()` arguments, and
    /// `slot` must be that function's slot.
    #[inline]
    unsafe fn run(&self, slot: usize, code: *const u8, args: &[i64]) -> Result<i64, CallError> {
        // One relaxed load gates all metric work; when metrics are off the call
        // path pays only this branch (no clock read, no atomic write).
        let metering = self.metrics_enabled.load(Ordering::Relaxed);
        let start = if metering { Some(Instant::now()) } else { None };

        // Fresh per-call state on this thread's stack — automatically per-thread
        // and per-call, so concurrent callers never share counters. Depth and
        // fuel guards are both active; the fuel budget is read lock-free.
        let mut state = CallState::new(self.fuel_budget.load(Ordering::Relaxed));
        let result = abi::invoke(code, &mut state, args);
        let outcome = match state.trap as i64 {
            TRAP_STACK => Err(CallError::ResourceExhausted),
            TRAP_FUEL => Err(CallError::FuelExhausted),
            _ => Ok(result),
        };

        if metering {
            // Lock-free, relaxed: counts can't be lost or torn under concurrent
            // callers, and a body swap doesn't disturb them (the slot is stable).
            let m = &self.metrics[slot];
            m.calls.fetch_add(1, Ordering::Relaxed);
            if let Some(started) = start {
                m.total_nanos.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
            if outcome.is_err() {
                m.faults.fetch_add(1, Ordering::Relaxed);
            }
        }
        outcome
    }

    /// Resolve `name` to a reusable [`FuncHandle`] once, so subsequent calls can
    /// take the lock-free fast path ([`Self::call_handle`]) instead of a
    /// name lookup under a lock every time. This is the path a hot serving loop
    /// (thousands of calls per request, per thread) should use.
    pub fn handle(&self, name: &str) -> Result<FuncHandle, CallError> {
        let dispatch = self.dispatch.read().unwrap();
        let entry = dispatch
            .get(name)
            .ok_or_else(|| CallError::UnknownFunction(name.to_string()))?;
        Ok(FuncHandle {
            name: name.to_string(),
            slot: entry.slot,
            arity: entry.arity,
            sig: entry.sig.clone(),
        })
    }

    /// Invoke a previously-[resolved](Self::handle) function on its lock-free
    /// fast path: an arity check, one acquire-load of the slot's code pointer
    /// (double-checked against the slot's arity), and the indirect call. No
    /// lock, no name lookup.
    ///
    /// The handle transparently survives body hot-swaps — it keeps calling and
    /// picks up the new code automatically. If a relink *changed the function's
    /// arity* (or removed it) since the handle was resolved, the handle is
    /// refreshed once against the current program; if the caller's argument
    /// count no longer matches the new signature the call returns
    /// [`CallError::ArityMismatch`] (never undefined behavior).
    pub fn call_handle(&self, handle: &mut FuncHandle, args: &[i64]) -> Result<i64, CallError> {
        if args.len() > MAX_ARITY {
            return Err(CallError::UnsupportedArity(args.len()));
        }
        // Bounded retry: a refresh re-reads the current program once; more than a
        // couple of iterations would require relinks racing every attempt, which
        // does not happen in practice (relinks are rare, human/CI-paced events).
        for _ in 0..8 {
            // Double-check the slot's arity around the code load. Because a
            // relink publishes arity *before* code (see `SwapTable::publish`),
            // observing the *handle's* arity both before and after the code load
            // guarantees the code we call was compiled for exactly that arity.
            let a1 = self.table.arity(handle.slot).load(Ordering::Acquire);
            if a1 == handle.arity as u64 {
                // The handle is current for its slot. Now validate the caller's
                // argument count against it — checking this *after* confirming
                // the handle isn't stale, so a caller adapting to a new arity
                // isn't rejected against an outdated cached arity.
                if args.len() != handle.arity {
                    return Err(CallError::ArityMismatch {
                        name: handle.name.clone(),
                        expected: handle.arity,
                        got: args.len(),
                    });
                }
                let code = self.table.code(handle.slot).load(Ordering::Acquire) as usize as *const u8;
                let a2 = self.table.arity(handle.slot).load(Ordering::Acquire);
                if a2 == handle.arity as u64 {
                    // SAFETY: `code` is finalized code compiled for exactly
                    // `handle.arity` arguments, which equals `args.len()`.
                    return unsafe { self.run(handle.slot, code, args) };
                }
            }
            // The slot's arity differs from the handle (a relink changed the
            // function's ABI, or removed it) — refresh once and retry.
            *handle = self.handle(&handle.name)?;
        }
        Err(CallError::HandleStale)
    }

    /// The typed counterpart of [`Self::call_handle`]: the lock-free fast path,
    /// but validating argument types and decoding the result against the
    /// handle's signature. Use this for a hot serving loop over a
    /// `float`-returning function.
    pub fn call_handle_typed(
        &self,
        handle: &mut FuncHandle,
        args: &[Value],
    ) -> Result<Value, CallError> {
        if args.len() > MAX_ARITY {
            return Err(CallError::UnsupportedArity(args.len()));
        }
        // Validate and encode against the handle's *current* signature. A relink
        // that changes arity is caught by `call_handle` below (which refreshes);
        // one that changes only types is as rare and as non-faulting as on the
        // raw path — the encoded bits are still a valid 64-bit word either way.
        let mut raw = [0i64; MAX_ARITY];
        for (i, (v, pty)) in args.iter().zip(handle.sig.params.iter()).enumerate() {
            if v.ty() != *pty {
                return Err(CallError::TypeMismatch {
                    name: handle.name.clone(),
                    position: i,
                    expected: *pty,
                    got: v.ty(),
                });
            }
            raw[i] = v.to_abi();
        }
        // `call_handle` enforces arity (refreshing the handle if a relink changed
        // it), runs the code, and leaves `handle.sig` current — so decode the
        // result against the possibly-refreshed return type.
        let bits = self.call_handle(handle, &raw[..args.len()])?;
        Ok(Value::from_abi(bits, handle.sig.ret))
    }

    /// Number of completed generations (0 after `new`, +1 per effective reload).
    pub fn generation(&self) -> usize {
        self.inner.lock().unwrap().generation
    }

    /// Set the call-nesting depth limit baked into future generations' entry
    /// guards (H2). Takes effect on the next reload; existing compiled code
    /// keeps the limit it was built with. Must be chosen so that
    /// `limit × worst-case-frame-size` stays comfortably within the stack of
    /// every thread that will call into the runtime.
    pub fn set_max_depth(&self, limit: u64) {
        self.inner.lock().unwrap().max_depth = limit;
    }

    /// Set the per-call fuel budget (H3). Takes effect on the *next* call — no
    /// recompilation needed, since the budget is a property of each call's
    /// state, not the compiled code. Every call and loop back-edge spends one
    /// unit; a call that runs out aborts with [`CallError::FuelExhausted`].
    /// Pass [`u64::MAX`] to disable metering entirely.
    pub fn set_fuel_budget(&self, budget: u64) {
        self.fuel_budget.store(budget, Ordering::Relaxed);
    }

    /// Turn per-function metrics collection on or off. Off by default: the call
    /// path then pays only a single relaxed flag load, so the throughput numbers
    /// hold. When on, every call records its count, wall-clock latency, and
    /// whether it faulted — all through lock-free atomics that scale with the
    /// call path. Read them with [`Self::metrics`].
    pub fn set_metrics_enabled(&self, on: bool) {
        self.metrics_enabled.store(on, Ordering::Relaxed);
    }

    /// A snapshot of `name`'s execution counters, or `None` if the runtime has
    /// no such function. The read is off the hot path (it resolves the slot
    /// under the dispatch read lock, then loads relaxed atomics), so gathering
    /// metrics never blocks or slows the calls being measured.
    pub fn metrics(&self, name: &str) -> Option<FnMetricsSnapshot> {
        let slot = self.dispatch.read().unwrap().get(name).map(|e| e.slot)?;
        let m = &self.metrics[slot];
        Some(FnMetricsSnapshot {
            calls: m.calls.load(Ordering::Relaxed),
            total_nanos: m.total_nanos.load(Ordering::Relaxed),
            faults: m.faults.load(Ordering::Relaxed),
        })
    }

    /// Zero every function's counters — e.g. to start a fresh measurement
    /// window. Racing with in-flight calls is safe (each field is atomic); a
    /// call landing mid-reset is simply counted in the new window.
    pub fn reset_metrics(&self) {
        for m in self.metrics.iter() {
            m.calls.store(0, Ordering::Relaxed);
            m.total_nanos.store(0, Ordering::Relaxed);
            m.faults.store(0, Ordering::Relaxed);
        }
    }

    /// The reload journal: every reload event so far, oldest first — the audit
    /// trail of what changed, how it was classified, its blast radius, its
    /// diagnostics, and its latency. Committed entries also carry the exact
    /// source they installed (the snapshot [`Self::rollback`] reinstalls).
    pub fn journal(&self) -> Vec<JournalEntry> {
        self.inner.lock().unwrap().journal.clone()
    }

    /// Revert the program to the source that produced a past `generation`.
    ///
    /// Rollback is **not** a special code path: it reinstalls the stored source
    /// of that generation and runs it through the ordinary reload protocol, so
    /// the revert is itself classified (`SafeSwap` / `Relink` / `NoEffect`),
    /// committed with exactly the synchronization that class proves sound, and
    /// journaled as a new event. Because that source was already accepted once —
    /// and host functions are only ever added — a rollback to a committed
    /// generation can never be `Rejected`.
    ///
    /// `generation` must be a committed generation number seen in the
    /// [`Self::journal`] (the initial load is generation 1). A `Rejected` or
    /// `NoEffect` event installed nothing and is not a valid target.
    pub fn rollback(&self, generation: usize) -> Result<ReloadReport, String> {
        let started = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        let source = inner
            .journal
            .iter()
            .find(|e| e.generation == generation && e.is_committed())
            .map(|e| e.source.clone())
            .ok_or_else(|| {
                format!(
                    "no committed generation {generation} to roll back to (current generation is {})",
                    inner.generation
                )
            })?;
        let src = inner.src;
        src.set_text(&mut inner.db).to(source);
        self.load_generation(&mut inner, Some(started))
    }

    /// Stand up a **canary**: compile `new_source` as an isolated candidate and
    /// begin shadowing it against the live program under `policy`. Calls routed
    /// through [`Self::call_canary`] are then mirrored (a sampled fraction)
    /// through both versions and compared — but the caller *always* receives the
    /// live answer, so a bad candidate can never reach a real request.
    ///
    /// The candidate goes through the same diagnostics gate as any program: a
    /// defective candidate is rejected here (returned as `Err`) and no canary
    /// starts. Only one canary may be active at a time; [`Self::promote`] or
    /// [`Self::abort_canary`] it first.
    pub fn canary(&self, new_source: &str, policy: CanaryPolicy) -> Result<(), String> {
        // The candidate must know the primary's host functions, or the gate
        // would reject any call to them as undefined; and it inherits the
        // primary's live resource limits so the shadow runs under production
        // conditions (a runaway candidate traps on the same budget it would
        // face if promoted).
        let (host_fns, max_depth, fuel_budget) = {
            let inner = self.inner.lock().unwrap();
            (inner.host_fns.clone(), inner.max_depth, self.fuel_budget.load(Ordering::Relaxed))
        };
        let program = Self::new_candidate(new_source, &host_fns, max_depth, fuel_budget)?;

        let mut guard = self.canary.lock().unwrap();
        if guard.is_some() {
            return Err("a canary is already active; promote or abort it first".to_string());
        }
        *guard = Some(Box::new(Canary::new(program, policy, new_source.to_string())));
        // Arm the lock-free sampler: reset the counter and set the rate (N, at
        // least 1 — a rate of 0 or 1 in the policy both mean "mirror every
        // call"). These are written *before* the `Release` store of
        // `canary_active` below, so a reader that acquires `canary_active == true`
        // also sees the rate and a fresh counter — never a stale one.
        self.canary_counter.store(0, Ordering::Relaxed);
        self.canary_rate.store(policy.sample_every.max(1), Ordering::Relaxed);
        // Publish *after* the candidate and sampler are in place, so `call_canary`
        // never observes the fast-path flag set with no canary behind it.
        self.canary_active.store(true, Ordering::Release);
        Ok(())
    }

    /// Invoke `name` like [`Self::call`], but if a canary is active, mirror a
    /// sampled fraction of these calls through the candidate too and fold the
    /// comparison into the canary's tally.
    ///
    /// The returned value is **always** the live program's — the candidate is a
    /// pure shadow. Use this as the request entry point while a canary may be
    /// running; with no canary active it is [`Self::call`] plus one relaxed load.
    ///
    /// Note: the candidate re-executes the call, so mirror only logic that is
    /// free of observable host side effects (the norm for scoring/decision
    /// functions) — otherwise the shadow would double those effects.
    pub fn call_canary(&self, name: &str, args: &[i64]) -> Result<i64, CallError> {
        // Fast path: no canary → exactly `call` plus a relaxed load.
        if !self.canary_active.load(Ordering::Acquire) {
            return self.call(name, args);
        }
        // A canary is active. Decide whether *this* call is sampled with two
        // relaxed atomics and no lock — so the calls we don't mirror pay nothing
        // beyond that. `rate == 0` means idle or auto-aborted: skip straight to
        // the live call.
        let rate = self.canary_rate.load(Ordering::Relaxed);
        let sampling = rate != 0 && {
            let n = self.canary_counter.fetch_add(1, Ordering::Relaxed);
            n % rate == 0
        };
        let primary_start = if sampling { Some(Instant::now()) } else { None };
        let primary = self.call(name, args);
        if let Some(start) = primary_start {
            let primary_nanos = start.elapsed().as_nanos() as u64;
            // Only the sampled fraction takes the lock. Holding it across the
            // shadow call keeps the candidate alive for its duration (a
            // concurrent promote/abort waits behind it). The candidate may have
            // been consumed between the decision and here — then it is simply
            // `None` and we skip, having already served the live answer.
            let guard = self.canary.lock().unwrap();
            if let Some(canary) = guard.as_ref() {
                canary.observe(name, args, &primary, primary_nanos);
                // If this observation tripped a policy threshold, switch the
                // sampler off so no further calls mirror a dead candidate.
                if canary.is_aborted() {
                    self.canary_rate.store(0, Ordering::Relaxed);
                }
            }
        }
        primary
    }

    /// Promote the active canary: reinstall the candidate's source into the live
    /// program through the ordinary reload protocol (so the promotion is itself
    /// classified, committed with the synchronization its class proves sound,
    /// and journaled), then clear the canary.
    ///
    /// Refused if there is no canary, or if it has auto-aborted — a candidate
    /// the shield already rejected must not be promoted.
    pub fn promote(&self) -> Result<ReloadReport, String> {
        let source = {
            let mut guard = self.canary.lock().unwrap();
            let canary = guard.as_ref().ok_or("no active canary to promote")?;
            if canary.is_aborted() {
                return Err("the canary auto-aborted and cannot be promoted".to_string());
            }
            let source = canary.source.clone();
            *guard = None;
            self.canary_rate.store(0, Ordering::Relaxed);
            self.canary_active.store(false, Ordering::Release);
            source
        };
        // Promote through the same classified swap as any other edit.
        self.reload(&source)
    }

    /// Discard the active canary without changing the live program. A no-op if
    /// none is active.
    pub fn abort_canary(&self) {
        self.canary_rate.store(0, Ordering::Relaxed);
        self.canary_active.store(false, Ordering::Release);
        *self.canary.lock().unwrap() = None;
    }

    /// A snapshot of the active canary's comparison, or `None` if none is
    /// active. Includes the verdict (running, or which threshold auto-aborted).
    pub fn canary_status(&self) -> Option<CanaryStatus> {
        self.canary.lock().unwrap().as_ref().map(|c| c.status())
    }

    /// Enable query-execution tracing on the underlying database (testing:
    /// proves which queries re-ran during a reload).
    pub fn enable_tracing(&self) -> ExecTrace {
        self.inner.lock().unwrap().db.enable_tracing()
    }

    // -- internals ----------------------------------------------------------

    fn ensure_slot(inner: &mut LiveInner, id: FunctionId) -> usize {
        if let Some(&slot) = inner.slots.get(&id) {
            return slot;
        }
        let slot = inner.next_slot;
        assert!(slot < TABLE_CAPACITY, "swap table exhausted");
        inner.next_slot += 1;
        inner.slots.insert(id, slot);
        slot
    }

    /// Pull the current DevIR through the incremental graph, diff it against
    /// the previous generation, compile the blast radius, and commit.
    ///
    /// `edit_started` is `None` for the initial load (no report classification
    /// is meaningful) and `Some` for reloads.
    fn load_generation(
        &self,
        inner: &mut LiveInner,
        edit_started: Option<Instant>,
    ) -> Result<ReloadReport, String> {
        let src = inner.src;
        let host = inner.host_functions;
        // The exact source this event installs (or rejects) — journaled with
        // every outcome so `rollback` can reinstall any past generation.
        let source_text = src.text(&inner.db).to_string();

        // 0. THE GATE. Before touching anything live, ask the query graph to
        //    prove the whole program is free of statically-detectable defects
        //    (syntax errors, undefined callees, arity mismatches, undefined
        //    variables — see `blaze_ir::diag`). A non-empty result means the
        //    edit is rejected: no generation is compiled, no slot is patched,
        //    and the previous generation keeps serving every call untouched.
        //    `LiveRuntime::new` upgrades this into a hard `Err` since there is
        //    no "previous generation" to hold on the very first load.
        let diags = program_diagnostics(&inner.db, src, host);
        if !diags.is_empty() {
            let report = ReloadReport {
                class: EditClass::Rejected,
                changed: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                diagnostics: diags.as_ref().clone(),
                latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
                generation: inner.generation,
            };
            return Ok(Self::record_event(inner, &source_text, report));
        }

        // 1. Demand the whole program through the query graph. Unchanged
        //    functions are memo hits; the firewall keeps body edits contained.
        let names = program_outline(&inner.db, src);
        let mut current: Vec<(String, std::sync::Arc<FunctionNode>)> = Vec::with_capacity(names.len());
        for name in names.iter() {
            let key = FnKey::new(&inner.db, name.clone());
            current.push((name.clone(), lowered_dev_ir(&inner.db, src, key)));
        }

        // 2. Blast radius = functions whose DevIR *content* differs from the
        //    previous generation. The graph already minimized this set: a
        //    body-only edit re-lowers one function; an ABI edit re-lowers the
        //    callee and every caller (they depend on its signature).
        let mut changed: Vec<(String, std::sync::Arc<FunctionNode>)> = Vec::new();
        let mut added: Vec<String> = Vec::new();
        let mut signature_changed = false;
        for (name, node) in &current {
            match inner.prev.get(name) {
                Some(prev_node) => {
                    if **prev_node != **node {
                        if prev_node.signature != node.signature {
                            signature_changed = true;
                        }
                        changed.push((name.clone(), node.clone()));
                    }
                }
                None => {
                    if !inner.host_fns.contains_key(name) {
                        added.push(name.clone());
                        changed.push((name.clone(), node.clone()));
                    }
                }
            }
        }
        let current_names: std::collections::HashSet<&str> =
            current.iter().map(|(n, _)| n.as_str()).collect();
        let removed: Vec<String> = inner
            .prev
            .keys()
            .filter(|n| !current_names.contains(n.as_str()))
            .cloned()
            .collect();

        let is_initial = edit_started.is_none();
        if changed.is_empty() && removed.is_empty() && !is_initial {
            let report = ReloadReport {
                class: EditClass::NoEffect,
                changed: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                diagnostics: Vec::new(),
                latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
                generation: inner.generation,
            };
            return Ok(Self::record_event(inner, &source_text, report));
        }

        let class = if signature_changed || !removed.is_empty() {
            EditClass::Relink
        } else {
            EditClass::SafeSwap
        };

        // 3. Allocate slots for everything the new code mentions, *before*
        //    compiling, so emitted slot addresses are final.
        for (_, node) in &changed {
            Self::ensure_slot(inner, node.id);
            for dep in &node.dependencies {
                Self::ensure_slot(inner, *dep);
            }
        }

        // 4. Compile only the blast radius into a fresh generation of
        //    executable pages (JITModule handles the PROT_WRITE→PROT_EXEC
        //    transition and instruction-cache coherence).
        let mut compiled: Vec<(String, usize, cranelift_module::FuncId)> = Vec::new();
        let mut module = JITModule::new(JITBuilder::with_isa(
            inner.isa.clone(),
            default_libcall_names(),
        ));
        let call_conv = inner.isa.default_call_conv();
        let max_depth = inner.max_depth;
        let mut fb_ctx = FunctionBuilderContext::new();
        for (name, node) in &changed {
            let sig = clif_signature(node.signature.arity(), call_conv);
            let func_id = module
                .declare_function(name, Linkage::Export, &sig)
                .map_err(|e| format!("declare `{name}` failed: {e}"))?;

            let mut ctx = module.make_context();
            ctx.func.signature = sig;
            ctx.func.name = UserFuncName::user(0, node.id.0);
            {
                let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
                let mut emitter = TableEmitter {
                    call_conv,
                    table: &self.table,
                    slots: &inner.slots,
                };
                build_body(&mut builder, node, max_depth, &mut emitter);
                builder.finalize(inner.isa.frontend_config());
            }
            module
                .define_function(func_id, &mut ctx)
                .map_err(|e| format!("define `{name}` failed: {e}"))?;
            module.clear_context(&mut ctx);

            compiled.push((name.clone(), inner.slots[&node.id], func_id));
        }
        module
            .finalize_definitions()
            .map_err(|e| format!("finalize failed: {e}"))?;

        // 5. Commit. The pointer stores are identical in both classes; what
        //    differs is the synchronization the class *proves* is sufficient:
        //     - SafeSwap of a single function: the firewall guarantees no other
        //       code changed, so one release-store is globally consistent even
        //       against concurrent callers. Lock-free.
        //     - Relink (or a multi-function swap, where cross-function
        //       consistency of the *set* matters): quiesce root calls via the
        //       dispatch write lock, then commit everything at once.
        // Lock-free only for the pure case: one existing function, body-only.
        // Anything that touches the dispatch map (new names, removals, ABI
        // changes, initial load) or patches multiple slots commits under the
        // quiescence barrier so callers observe the set atomically.
        let lock_free = class == EditClass::SafeSwap
            && compiled.len() <= 1
            && added.is_empty()
            && removed.is_empty()
            && !is_initial;
        let commit = |dispatch: &mut HashMap<String, DispatchEntry>| {
            for (name, slot, func_id) in &compiled {
                let sig = current
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, node)| Arc::new(node.signature.clone()))
                    .unwrap_or_else(|| Arc::new(Signature::default()));
                let arity = sig.arity();
                let code = module.get_finalized_function(*func_id);
                // Arity before code — the ordering the lock-free handle path relies on.
                self.table.publish(*slot, arity, code as u64);
                dispatch.insert(name.clone(), DispatchEntry { slot: *slot, arity, sig });
            }
            for name in &removed {
                let id = blaze_ir::function_id(&inner.db, name);
                if let Some(&slot) = inner.slots.get(&id) {
                    self.table.clear(slot);
                }
                dispatch.remove(name);
            }
        };

        if lock_free {
            // A single body-only edit: arity is unchanged, so the dispatch map
            // and the slot's published arity both stay put — only the code
            // pointer moves, atomically. Live handles keep working and pick up
            // the new body on their next call for free.
            for (name, slot, func_id) in &compiled {
                let arity = current
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, node)| node.signature.arity())
                    .unwrap_or(0);
                let code = module.get_finalized_function(*func_id);
                self.table.publish(*slot, arity, code as u64);
            }
        } else {
            let mut dispatch = self.dispatch.write().unwrap();
            commit(&mut dispatch);
        }

        // 6. Retire the generation (kept alive: in-flight callers may still be
        //    executing previous generations) and refresh the snapshot.
        inner.generations.push(module);
        inner.generation += 1;
        inner.prev = current.into_iter().collect();

        let report = ReloadReport {
            class: if is_initial { EditClass::SafeSwap } else { class },
            changed: changed.iter().map(|(n, _)| n.clone()).collect(),
            added,
            removed,
            diagnostics: Vec::new(),
            latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
            generation: inner.generation,
        };
        Ok(Self::record_event(inner, &source_text, report))
    }

    /// Append a reload event to the journal and return the report unchanged.
    /// The journaled snapshot is what [`Self::rollback`] reinstalls.
    fn record_event(inner: &mut LiveInner, source: &str, report: ReloadReport) -> ReloadReport {
        inner.journal.push(JournalEntry {
            sequence: inner.journal.len(),
            generation: report.generation,
            at: SystemTime::now(),
            source: source.to_string(),
            class: report.class,
            changed: report.changed.clone(),
            added: report.added.clone(),
            removed: report.removed.clone(),
            diagnostics: report.diagnostics.clone(),
            latency: report.latency,
        });
        report
    }
}

/// Compile a `(ctx, args…) -> i64` trampoline that drops the hidden context
/// pointer and calls the real host function `host_ptr` (which takes only the
/// `args`), returning the trampoline's finalized address.
///
/// This bridges host functions — written by the embedder with a plain
/// `(i64 × arity) -> i64` C ABI — into the context-threading ABI every Blaze
/// call site uses, so a host function is dispatched exactly like a Blaze one.
fn compile_host_trampoline(module: &mut JITModule, arity: usize, host_ptr: *const u8) -> *const u8 {
    let call_conv = module.target_config().default_call_conv;
    // The trampoline's own signature: (ctx, args…) -> i64, like any Blaze fn.
    let tramp_sig = clif_signature(arity, call_conv);
    let func_id = module
        .declare_anonymous_function(&tramp_sig)
        .expect("declare host trampoline");

    let mut ctx = module.make_context();
    ctx.func.signature = tramp_sig;
    {
        let mut fb_ctx = FunctionBuilderContext::new();
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        b.seal_block(entry);

        // Params: [ctx, arg0, … arg{arity-1}]. Drop ctx, forward the rest.
        let params = b.block_params(entry).to_vec();
        let real_args = &params[1..];

        // The host function's real signature has no context pointer.
        let mut host_sig = ClifSig::new(call_conv);
        for _ in 0..arity {
            host_sig.params.push(AbiParam::new(types::I64));
        }
        host_sig.returns.push(AbiParam::new(types::I64));
        let host_sig_ref = b.import_signature(host_sig);

        let target = b.ins().iconst(types::I64, host_ptr as i64);
        let call = b.ins().call_indirect(host_sig_ref, target, real_args);
        let result = b.inst_results(call)[0];
        b.ins().return_(&[result]);
        b.finalize(module.target_config());
    }

    module.define_function(func_id, &mut ctx).expect("define host trampoline");
    module.clear_context(&mut ctx);
    module.finalize_definitions().expect("finalize host trampoline");
    module.get_finalized_function(func_id)
}

// ---------------------------------------------------------------------------
// File-watching host
// ---------------------------------------------------------------------------

/// A [`LiveRuntime`] bound to a `.blaze` file on disk, reloading on change.
///
/// Watching is dependency-free mtime polling — call [`ScriptHost::poll`] once
/// per frame (or at any cadence you like) and apply the report it returns.
pub struct ScriptHost {
    runtime: LiveRuntime,
    path: PathBuf,
    last_modified: Option<SystemTime>,
}

impl ScriptHost {
    /// Load `path` and compile it.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let source = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let last_modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Ok(ScriptHost { runtime: LiveRuntime::new(&source)?, path, last_modified })
    }

    /// The underlying runtime, for calls and host-function registration.
    pub fn runtime(&self) -> &LiveRuntime {
        &self.runtime
    }

    /// If the file changed since the last poll, reload it and return the
    /// report. Returns `Ok(None)` when nothing changed.
    pub fn poll(&mut self) -> Result<Option<ReloadReport>, String> {
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .map_err(|e| format!("failed to stat {}: {e}", self.path.display()))?;
        if self.last_modified == Some(modified) {
            return Ok(None);
        }
        self.last_modified = Some(modified);
        let source = std::fs::read_to_string(&self.path)
            .map_err(|e| format!("failed to read {}: {e}", self.path.display()))?;
        self.runtime.reload(&source).map(Some)
    }
}
