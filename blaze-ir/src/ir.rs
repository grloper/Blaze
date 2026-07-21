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
/// Sourced from `salsa`'s **interner**, not a name hash — see
/// [`crate::db::function_id`]. Interning is an injective map, so two distinct
/// function names can *never* share an id within a database; the value is
/// consistent for the life of a runtime, which is the only scope in which ids
/// are compared. (The earlier FNV-1a hash could, in principle, alias two names
/// to one id and route a call to the wrong function — an id is now real
/// identity, never a probabilistic guess.)
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct FunctionId(pub u32);

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

/// An integer comparison, producing `1` (true) or `0` (false).
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub enum CmpKind {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

/// A single DevIR operation over dense `u32` virtual registers.
///
/// Registers are *mutable* virtual registers, not strict SSA values: parameters
/// occupy `0..arity`, each named local owns one dedicated register (so `Copy`
/// implements assignment across control-flow joins), and expression temporaries
/// are freshly numbered. The JIT backend maps registers onto
/// `cranelift-frontend` variables, which reconstructs SSA form (including phis)
/// mechanically.
///
/// Control flow is label-structured: [`IrOp::Label`] opens a basic block and
/// lowering guarantees every block is closed by exactly one `Jump`, `Branch`,
/// or `Return` before the next `Label` (or end of body). Backends may rely on
/// that invariant.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum IrOp {
    /// `dst = <imm>`
    LoadInt(u32, i64),
    /// `dst = lhs + rhs`
    Add(u32, u32, u32),
    /// `dst = lhs - rhs`
    Sub(u32, u32, u32),
    /// `dst = lhs * rhs`
    Mul(u32, u32, u32),
    /// `dst = lhs / rhs` — **guarded**: `x / 0 == 0` and `INT_MIN / -1 ==
    /// INT_MIN`, so no live-edited program can fault the host process on a
    /// division. The backend emits the guards.
    Div(u32, u32, u32),
    /// `dst = (lhs <op> rhs) ? 1 : 0`
    Cmp(CmpKind, u32, u32, u32),
    /// `dst = src` (assignment into a variable's dedicated register)
    Copy(u32, u32),
    /// `dst = callee(args...)`
    Call(u32, FunctionId, Vec<u32>),
    /// `return value`
    Return(u32),
    /// Unconditional jump to a label.
    Jump(u32),
    /// `if cond != 0 goto then_label else goto else_label`
    Branch(u32, u32, u32),
    /// Opens the basic block named by this label id.
    Label(u32),
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

    /// The number of distinct virtual registers the body defines or references.
    pub fn register_count(&self) -> usize {
        let mut max = None::<u32>;
        let mut note = |r: u32| max = Some(max.map_or(r, |m| m.max(r)));
        for op in &self.body {
            match op {
                IrOp::LoadInt(d, _) => note(*d),
                IrOp::Add(d, a, b)
                | IrOp::Sub(d, a, b)
                | IrOp::Mul(d, a, b)
                | IrOp::Div(d, a, b)
                | IrOp::Cmp(_, d, a, b) => {
                    note(*d);
                    note(*a);
                    note(*b);
                }
                IrOp::Copy(d, s) => {
                    note(*d);
                    note(*s);
                }
                IrOp::Call(d, _, args) => {
                    note(*d);
                    args.iter().for_each(|a| note(*a));
                }
                IrOp::Return(v) => note(*v),
                IrOp::Branch(c, _, _) => note(*c),
                IrOp::Jump(_) | IrOp::Label(_) => {}
            }
        }
        max.map_or(0, |m| m as usize + 1)
    }

    /// The number of labels (basic-block openers) in the body.
    pub fn label_count(&self) -> usize {
        let mut max = None::<u32>;
        let mut note = |l: u32| max = Some(max.map_or(l, |m: u32| m.max(l)));
        for op in &self.body {
            match op {
                IrOp::Label(l) | IrOp::Jump(l) => note(*l),
                IrOp::Branch(_, t, e) => {
                    note(*t);
                    note(*e);
                }
                _ => {}
            }
        }
        max.map_or(0, |m| m as usize + 1)
    }
}

/// Accumulates DevIR operations while lowering an AST body. Kept in this module
/// so the register-allocation discipline lives next to the IR it produces.
#[derive(Debug, Default)]
pub struct IrBuilder {
    next_reg: u32,
    next_label: u32,
    ops: Vec<IrOp>,
    deps: BTreeSet<FunctionId>,
}

impl IrBuilder {
    /// Allocate a fresh, never-before-used virtual register.
    #[inline]
    pub fn fresh(&mut self) -> u32 {
        let r = self.next_reg;
        self.next_reg += 1;
        r
    }

    /// Allocate a fresh label. Bind it later with [`Self::bind_label`].
    #[inline]
    pub fn fresh_label(&mut self) -> u32 {
        let l = self.next_label;
        self.next_label += 1;
        l
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

    pub fn sub(&mut self, lhs: u32, rhs: u32) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::Sub(dst, lhs, rhs));
        dst
    }

    pub fn mul(&mut self, lhs: u32, rhs: u32) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::Mul(dst, lhs, rhs));
        dst
    }

    pub fn div(&mut self, lhs: u32, rhs: u32) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::Div(dst, lhs, rhs));
        dst
    }

    pub fn cmp(&mut self, kind: CmpKind, lhs: u32, rhs: u32) -> u32 {
        let dst = self.fresh();
        self.ops.push(IrOp::Cmp(kind, dst, lhs, rhs));
        dst
    }

    /// Assignment into an existing register (a variable's dedicated slot).
    pub fn copy(&mut self, dst: u32, src: u32) {
        self.ops.push(IrOp::Copy(dst, src));
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

    pub fn jump(&mut self, label: u32) {
        self.ops.push(IrOp::Jump(label));
    }

    pub fn branch(&mut self, cond: u32, then_label: u32, else_label: u32) {
        self.ops.push(IrOp::Branch(cond, then_label, else_label));
    }

    /// Open the basic block named `label` at the current position.
    pub fn bind_label(&mut self, label: u32) {
        self.ops.push(IrOp::Label(label));
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
    fn builder_numbers_registers_densely() {
        let mut b = IrBuilder::default();
        let a = b.load_int(1);
        let c = b.load_int(2);
        let s = b.add(a, c);
        b.ret(s);
        let node = b.finish(FunctionId(0), Signature::default());
        assert_eq!(node.register_count(), 3);
        assert_eq!(node.body.last(), Some(&IrOp::Return(2)));
    }
}
