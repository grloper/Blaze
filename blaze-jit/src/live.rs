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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlagsData, Signature as ClifSig, UserFuncName, Value};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa};
use cranelift_codegen::ir::types;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{default_libcall_names, Linkage, Module};

use blaze_ir::db::{BlazeDatabaseImpl, ExecTrace, FnKey, HostFunctions, SourceProgram};
use blaze_ir::diag::{format_diagnostics, program_diagnostics};
use blaze_ir::lower::{lowered_dev_ir, program_outline};
use blaze_ir::{Diagnostic, FunctionId, FunctionNode};
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

/// A page-aligned, `mmap`-allocated array of atomic function-pointer slots.
///
/// The table's address is stable for the life of the runtime — generated code
/// bakes `&table[slot]` in as an absolute constant and performs an atomic load
/// on every call, which is what makes pointer hot-swapping a single
/// release-store from the reloading thread.
struct SwapTable {
    base: *mut AtomicU64,
    bytes: usize,
}

// SAFETY: the table is a fixed allocation of atomics; all mutation goes through
// atomic operations.
unsafe impl Send for SwapTable {}
unsafe impl Sync for SwapTable {}

impl SwapTable {
    fn new() -> Result<Self, String> {
        let bytes = TABLE_CAPACITY * std::mem::size_of::<AtomicU64>();
        // SAFETY: anonymous private mapping, checked for MAP_FAILED below.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err("mmap of the swap table failed".to_string());
        }
        let table = SwapTable { base: base.cast::<AtomicU64>(), bytes };
        // Every slot starts as the missing stub, so even a call emitted against
        // a never-defined function lands somewhere harmless.
        for i in 0..TABLE_CAPACITY {
            table.slot(i).store(missing_stub as extern "C" fn(*mut CallState) -> i64 as usize as u64, Ordering::Release);
        }
        Ok(table)
    }

    #[inline]
    fn slot(&self, index: usize) -> &AtomicU64 {
        assert!(index < TABLE_CAPACITY);
        // SAFETY: `base` points at TABLE_CAPACITY zero-initialized AtomicU64s,
        // properly aligned by mmap's page alignment; index is bounds-checked.
        unsafe { &*self.base.add(index) }
    }

    /// Absolute address of a slot, for baking into generated code.
    #[inline]
    fn slot_addr(&self, index: usize) -> usize {
        assert!(index < TABLE_CAPACITY);
        self.base as usize + index * std::mem::size_of::<AtomicU64>()
    }
}

impl Drop for SwapTable {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly the region mapped in `new`.
        unsafe { libc::munmap(self.base.cast(), self.bytes) };
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
        ctx: Value,
        callee: FunctionId,
        args: &[Value],
    ) -> Value {
        let slot_index = *self
            .slots
            .get(&callee)
            .expect("slot allocated for every callee before compilation");
        let slot_addr = self.table.slot_addr(slot_index) as i64;

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

/// Why a [`LiveRuntime::call`] could not run, or could not run to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    UnknownFunction(String),
    ArityMismatch { name: String, expected: usize, got: usize },
    UnsupportedArity(usize),
    /// The call exceeded the call-nesting limit (runaway recursion). The call
    /// was aborted and the process/runtime left consistent; nothing faulted.
    ResourceExhausted,
    /// The call exhausted its execution budget (e.g. an infinite loop). The
    /// call was aborted deterministically; the runtime remains usable. (H3)
    FuelExhausted,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::UnknownFunction(name) => write!(f, "unknown function `{name}`"),
            CallError::ArityMismatch { name, expected, got } => {
                write!(f, "`{name}` takes {expected} argument(s), got {got}")
            }
            CallError::UnsupportedArity(n) => write!(f, "arity {n} exceeds the dispatch limit"),
            CallError::ResourceExhausted => {
                write!(f, "call aborted: exceeded the call-depth limit (runaway recursion)")
            }
            CallError::FuelExhausted => {
                write!(f, "call aborted: exhausted its execution budget")
            }
        }
    }
}

impl std::error::Error for CallError {}

// ---------------------------------------------------------------------------
// The runtime
// ---------------------------------------------------------------------------

/// Per-name dispatch data used by [`LiveRuntime::call`].
#[derive(Debug, Clone, Copy)]
struct DispatchEntry {
    slot: usize,
    arity: usize,
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
    inner: Mutex<LiveInner>,
}

impl LiveRuntime {
    /// Compile `source` and stand the program up, ready to call.
    pub fn new(source: &str) -> Result<Self, String> {
        let db = BlazeDatabaseImpl::default();
        let src = SourceProgram::new(&db, source.to_string());
        let host_functions = HostFunctions::new(&db, std::collections::BTreeMap::new());
        let isa = host_isa()?;
        let trampolines =
            JITModule::new(JITBuilder::with_isa(isa.clone(), default_libcall_names()));

        let runtime = LiveRuntime {
            table: SwapTable::new()?,
            dispatch: RwLock::new(HashMap::new()),
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
            }),
        };
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
        let id = blaze_ir::function_id_of(name);
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
        self.table.slot(slot).store(trampoline as u64, Ordering::Release);
        self.dispatch
            .write()
            .unwrap()
            .insert(name.to_string(), DispatchEntry { slot, arity });
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
            .copied()
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

        let code = self.table.slot(entry.slot).load(Ordering::Acquire) as usize as *const u8;

        // Fresh per-call state on this thread's stack — automatically per-thread
        // and per-call, so concurrent callers never share counters. Fuel is
        // disabled here (H2); the depth guard is always active.
        let mut state = CallState::new(u64::MAX);

        // SAFETY: `code` is either the missing stub or a finalized function
        // compiled with the context-threading `(*mut CallState, i64 × arity)`
        // signature; the arity was checked against the dispatch table, which is
        // updated atomically with the slots it describes.
        let result = unsafe { abi::invoke(code, &mut state, args) };

        match state.trap as i64 {
            TRAP_STACK => Err(CallError::ResourceExhausted),
            TRAP_FUEL => Err(CallError::FuelExhausted),
            _ => Ok(result),
        }
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
            return Ok(ReloadReport {
                class: EditClass::Rejected,
                changed: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                diagnostics: diags.as_ref().clone(),
                latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
                generation: inner.generation,
            });
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
            return Ok(ReloadReport {
                class: EditClass::NoEffect,
                changed: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
                diagnostics: Vec::new(),
                latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
                generation: inner.generation,
            });
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
                let code = module.get_finalized_function(*func_id);
                self.table.slot(*slot).store(code as u64, Ordering::Release);
                let arity = current
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, node)| node.signature.arity())
                    .unwrap_or(0);
                dispatch.insert(name.clone(), DispatchEntry { slot: *slot, arity });
            }
            for name in &removed {
                let id = blaze_ir::function_id_of(name);
                if let Some(&slot) = inner.slots.get(&id) {
                    self.table
                        .slot(slot)
                        .store(missing_stub as extern "C" fn(*mut CallState) -> i64 as usize as u64, Ordering::Release);
                }
                dispatch.remove(name);
            }
        };

        if lock_free {
            // The dispatch map itself is untouched (same name, same slot, same
            // arity) — only the slot's pointer moves, atomically.
            for (_, slot, func_id) in &compiled {
                let code = module.get_finalized_function(*func_id);
                self.table.slot(*slot).store(code as u64, Ordering::Release);
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

        Ok(ReloadReport {
            class: if is_initial { EditClass::SafeSwap } else { class },
            changed: changed.iter().map(|(n, _)| n.clone()).collect(),
            added,
            removed,
            diagnostics: Vec::new(),
            latency: edit_started.map(|t| t.elapsed()).unwrap_or_default(),
            generation: inner.generation,
        })
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
