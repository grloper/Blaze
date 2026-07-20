//! The **DevIR**: Blaze's SSA-style intermediate representation.
//!
//! DevIR is intentionally tiny — four opcodes over `i64` virtual registers —
//! because its job is not to be a rich optimizer IR but to be a *stable,
//! content-addressable unit of incremental work*. Each function lowers to one
//! [`FunctionNode`]; the surrounding `salsa` graph memoizes those nodes and
//! propagates invalidation along the `dependencies` edges.
//!
//! Every type here is `'static` (no database lifetime), so nodes can be handed
//! to the JIT backend and stored in `Arc`s that outlive any single query.

use std::collections::BTreeSet;

/// A stable, program-wide identifier for a function.
///
/// Derived from the function's name via FNV-1a (see [`function_id_of`]) so it is
/// independent of definition order: editing one function's body never renumbers
/// another. A production frontend would source these from an interned symbol
/// table; the hash is sufficient and collision-free for the C-subset.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct FunctionId(pub u32);

/// FNV-1a (32-bit) of `name`. `const` so ids can be computed in const contexts.
pub const fn function_id_of(name: &str) -> FunctionId {
    let bytes = name.as_bytes();
    let mut hash: u32 = 0x811c_9dc5; // FNV offset basis
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u32;
        hash = hash.wrapping_mul(0x0100_0193); // FNV prime
        i += 1;
    }
    FunctionId(hash)
}

/// The Blaze type lattice. Only `int` (a machine `i64`) exists today; the enum
/// is the extension point for the pointer and aggregate types the grammar
/// already reserves tokens for.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub enum Type {
    Int,
}

/// A function's ABI: parameter types (in order) and return type.
///
/// The signature is the *firewall boundary* of the incremental graph. As long
/// as it compares equal across an edit, `salsa` backdates it and callers are not
/// re-evaluated — the mathematical invariant from the architecture spec.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct Signature {
    pub params: Vec<Type>,
    pub ret: Type,
}

impl Signature {
    /// Number of declared parameters.
    #[inline]
    pub fn arity(&self) -> usize {
        self.params.len()
    }
}

impl Default for Signature {
    /// The signature attributed to an undefined function: `() -> int`.
    fn default() -> Self {
        Signature { params: Vec::new(), ret: Type::Int }
    }
}

/// A single SSA operation. Registers are dense `u32` value numbers, unique
/// within a [`FunctionNode`]; each op defines at most one.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum IrOp {
    /// `dst = <imm>`
    LoadInt(u32, i64),
    /// `dst = lhs + rhs`
    Add(u32, u32, u32),
    /// `dst = callee(args...)`
    Call(u32, FunctionId, Vec<u32>),
    /// `return value`
    Return(u32),
}

/// The lowered form of one source function: its ABI, its SSA body, and the set
/// of callees it depends on (used to route invalidation and, later, JIT linking).
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct FunctionNode {
    pub id: FunctionId,
    pub signature: Signature,
    pub body: Vec<IrOp>,
    /// Callees referenced by this body, de-duplicated and sorted for a stable
    /// hash. An empty vector means the function is a leaf.
    pub dependencies: Vec<FunctionId>,
}

impl FunctionNode {
    /// A well-formed but empty node for a function that failed to parse. Keeps
    /// downstream queries total rather than optional.
    pub fn empty(id: FunctionId) -> Self {
        FunctionNode { id, signature: Signature::default(), body: Vec::new(), dependencies: Vec::new() }
    }

    /// The number of distinct SSA registers the body defines or references.
    pub fn register_count(&self) -> usize {
        let mut max = None::<u32>;
        let mut note = |r: u32| max = Some(max.map_or(r, |m| m.max(r)));
        for op in &self.body {
            match op {
                IrOp::LoadInt(d, _) => note(*d),
                IrOp::Add(d, a, b) => {
                    note(*d);
                    note(*a);
                    note(*b);
                }
                IrOp::Call(d, _, args) => {
                    note(*d);
                    args.iter().for_each(|a| note(*a));
                }
                IrOp::Return(v) => note(*v),
            }
        }
        max.map_or(0, |m| m as usize + 1)
    }
}

/// Accumulates SSA operations while lowering an AST body. Kept in this module so
/// the register-allocation discipline lives next to the IR it produces.
#[derive(Debug, Default)]
pub struct IrBuilder {
    next_reg: u32,
    ops: Vec<IrOp>,
    deps: BTreeSet<FunctionId>,
}

impl IrBuilder {
    /// Allocate a fresh, never-before-used SSA register.
    #[inline]
    pub fn fresh(&mut self) -> u32 {
        let r = self.next_reg;
        self.next_reg += 1;
        r
    }

    pub fn load_int(&mut self, value: i64) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::LoadInt(dst, value));
        dst
    }

    pub fn add(&mut self, lhs: u32, rhs: u32) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::Add(dst, lhs, rhs));
        dst
    }

    pub fn call(&mut self, callee: FunctionId, args: Vec<u32>) -> u32 {
        let dst = self.fresh();
        self.deps.insert(callee);
        self.ops.push(IrOp::Call(dst, callee, args));
        dst
    }

    pub fn ret(&mut self, value: u32) {
        self.ops.push(IrOp::Return(value));
    }

    /// Finish building, sealing the ops and dependency set into a node.
    pub fn finish(self, id: FunctionId, signature: Signature) -> FunctionNode {
        FunctionNode {
            id,
            signature,
            body: self.ops,
            dependencies: self.deps.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_ids_are_name_stable_and_distinct() {
        assert_eq!(function_id_of("main"), function_id_of("main"));
        assert_ne!(function_id_of("main"), function_id_of("add"));
    }

    #[test]
    fn builder_numbers_registers_densely() {
        let mut b = IrBuilder::default();
        let a = b.load_int(1);
        let c = b.load_int(2);
        let s = b.add(a, c);
        b.ret(s);
        let node = b.finish(function_id_of("f"), Signature::default());
        assert_eq!(node.register_count(), 3);
        assert_eq!(node.body.last(), Some(&IrOp::Return(2)));
    }
}
