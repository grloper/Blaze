//! The calling-convention contract shared by codegen and the runtime.
//!
//! Every Blaze function is compiled with a hidden leading argument: a pointer
//! to a [`CallState`] (wasmtime calls its equivalent the `vmctx`). The runtime
//! allocates one `CallState` *on the caller's own stack* per top-level
//! invocation, so the per-call counters below are automatically per-thread and
//! per-call — no thread-locals, no atomics, no contention, correct under
//! arbitrary concurrency for free.
//!
//! Generated code reads and writes the fields by fixed byte offset, so the
//! `#[repr(C)]` layout here and the [`OFF_*`] constants must stay in lockstep.
//! A change to one without the other is memory-unsafe; the test at the bottom
//! pins the offsets so that can't drift silently.

use std::mem::transmute as t;

/// Per-call execution state threaded through generated code.
///
/// Lives on the stack of whichever thread invoked a Blaze function, for the
/// duration of that one call tree. The runtime resets it before each top-level
/// call and inspects [`CallState::trap`] afterward to turn a resource-limit
/// hit into a defined `Err` rather than undefined behavior.
#[repr(C)]
#[derive(Debug)]
pub struct CallState {
    /// Current Blaze call nesting. Bumped in each function's prologue, dropped
    /// on each return; a prologue that would exceed the depth limit traps
    /// instead of recursing, which is what stops unbounded recursion from
    /// faulting the host. (H2)
    pub depth: u64,
    /// Remaining execution budget, decremented at loop back-edges and calls.
    /// Reserved for fuel metering (H3); set to [`u64::MAX`] to disable.
    pub fuel: u64,
    /// `0` while healthy, else one of the `TRAP_*` codes. Once set, the call's
    /// result is meaningless and the runtime reports the corresponding error.
    pub trap: u64,
}

// Byte offsets of each field, baked into generated loads/stores.
pub const OFF_DEPTH: i32 = 0;
pub const OFF_FUEL: i32 = 8;
pub const OFF_TRAP: i32 = 16;

// Trap codes stored in `CallState::trap`.
pub const TRAP_NONE: i64 = 0;
pub const TRAP_STACK: i64 = 1;
pub const TRAP_FUEL: i64 = 2;

impl CallState {
    /// A fresh state for one top-level call: no nesting, the given fuel budget,
    /// no trap. Pass [`u64::MAX`] as `fuel` to leave fuel metering off.
    #[inline]
    pub fn new(fuel: u64) -> Self {
        CallState { depth: 0, fuel, trap: TRAP_NONE as u64 }
    }
}

/// The largest argument count [`invoke`] can dispatch (excluding the hidden
/// context pointer).
pub const MAX_ARITY: usize = 8;

/// Call finalized Blaze code of `args.len()` arity, threading `state` as the
/// hidden context pointer.
///
/// # Safety
///
/// `code` must point at a finalized function compiled with
/// [`crate::codegen::clif_signature`] for exactly `args.len()` arguments
/// (`args.len() <= MAX_ARITY`), and `state` must be a valid, exclusively-borrowed
/// `CallState` for the duration of the call.
#[inline]
pub unsafe fn invoke(code: *const u8, state: *mut CallState, args: &[i64]) -> i64 {
    type F0 = extern "C" fn(*mut CallState) -> i64;
    type F1 = extern "C" fn(*mut CallState, i64) -> i64;
    type F2 = extern "C" fn(*mut CallState, i64, i64) -> i64;
    type F3 = extern "C" fn(*mut CallState, i64, i64, i64) -> i64;
    type F4 = extern "C" fn(*mut CallState, i64, i64, i64, i64) -> i64;
    type F5 = extern "C" fn(*mut CallState, i64, i64, i64, i64, i64) -> i64;
    type F6 = extern "C" fn(*mut CallState, i64, i64, i64, i64, i64, i64) -> i64;
    type F7 = extern "C" fn(*mut CallState, i64, i64, i64, i64, i64, i64, i64) -> i64;
    type F8 = extern "C" fn(*mut CallState, i64, i64, i64, i64, i64, i64, i64, i64) -> i64;
    match *args {
        [] => t::<*const u8, F0>(code)(state),
        [a] => t::<*const u8, F1>(code)(state, a),
        [a, b] => t::<*const u8, F2>(code)(state, a, b),
        [a, b, c] => t::<*const u8, F3>(code)(state, a, b, c),
        [a, b, c, d] => t::<*const u8, F4>(code)(state, a, b, c, d),
        [a, b, c, d, e] => t::<*const u8, F5>(code)(state, a, b, c, d, e),
        [a, b, c, d, e, f] => t::<*const u8, F6>(code)(state, a, b, c, d, e, f),
        [a, b, c, d, e, f, g] => t::<*const u8, F7>(code)(state, a, b, c, d, e, f, g),
        [a, b, c, d, e, f, g, h] => t::<*const u8, F8>(code)(state, a, b, c, d, e, f, g, h),
        _ => unreachable!("arity checked against MAX_ARITY before dispatch"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_offsets_match_repr_c_layout() {
        let s = CallState::new(0);
        let base = &s as *const _ as usize;
        assert_eq!(&s.depth as *const _ as usize - base, OFF_DEPTH as usize);
        assert_eq!(&s.fuel as *const _ as usize - base, OFF_FUEL as usize);
        assert_eq!(&s.trap as *const _ as usize - base, OFF_TRAP as usize);
    }
}
