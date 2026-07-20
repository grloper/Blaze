//! A thin *typed* view over the untyped `rowan` CST.
//!
//! `rowan` stores everything as `SyntaxNode`/`SyntaxToken` tagged by
//! [`SyntaxKind`]. That is perfect for losslessness but awkward to consume, so
//! this module layers ergonomic, allocation-light accessors on top — the same
//! "AST = typed wrapper around a red node" pattern rust-analyzer uses. Wrappers
//! are `Copy`-cheap handles: they borrow nothing and cloning just bumps a green
//! `Arc`.

use crate::syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

/// A typed handle around a single CST node of a known [`SyntaxKind`].
pub trait AstNode: Sized {
    /// Attempt to reinterpret `node` as `Self`; returns `None` on kind mismatch.
    fn cast(node: SyntaxNode) -> Option<Self>;
    /// The underlying untyped node.
    fn syntax(&self) -> &SyntaxNode;
}

/// First child node that casts to `N`.
fn child<N: AstNode>(node: &SyntaxNode) -> Option<N> {
    node.children().find_map(N::cast)
}

/// All child nodes that cast to `N`, in source order.
fn children<'a, N: AstNode + 'a>(node: &'a SyntaxNode) -> impl Iterator<Item = N> + 'a {
    node.children().filter_map(N::cast)
}

/// Text of the first direct child *token* of `kind`.
fn token_text(node: &SyntaxNode, kind: SyntaxKind) -> Option<String> {
    node.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == kind)
        .map(|t: SyntaxToken| t.text().to_string())
}

macro_rules! ast_node {
    ($name:ident, $kind:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);
        impl AstNode for $name {
            fn cast(node: SyntaxNode) -> Option<Self> {
                if node.kind() == $kind {
                    Some(Self(node))
                } else {
                    None
                }
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

ast_node!(SourceFile, SyntaxKind::SOURCE_FILE);
ast_node!(Function, SyntaxKind::FN);
ast_node!(ParamList, SyntaxKind::PARAM_LIST);
ast_node!(Param, SyntaxKind::PARAM);
ast_node!(Name, SyntaxKind::NAME);
ast_node!(Block, SyntaxKind::BLOCK);
ast_node!(LetStmt, SyntaxKind::LET_STMT);
ast_node!(AssignStmt, SyntaxKind::ASSIGN_STMT);
ast_node!(ReturnStmt, SyntaxKind::RETURN_STMT);
ast_node!(IfStmt, SyntaxKind::IF_STMT);
ast_node!(WhileStmt, SyntaxKind::WHILE_STMT);
ast_node!(ExprStmt, SyntaxKind::EXPR_STMT);

impl SourceFile {
    /// Parse `source` and return the typed root (never fails; malformed input
    /// simply yields fewer well-formed children).
    pub fn parse(source: &str) -> Self {
        let parse = crate::parser::parse(source);
        SourceFile::cast(parse.syntax()).expect("root is always SOURCE_FILE")
    }

    /// Top-level function definitions, in source order.
    pub fn functions(&self) -> impl Iterator<Item = Function> + '_ {
        children(&self.0)
    }
}

impl Name {
    /// The identifier text of this binding occurrence.
    pub fn text(&self) -> Option<String> {
        token_text(&self.0, SyntaxKind::IDENT)
    }
}

impl Function {
    /// The declared function name (`None` if the identifier was missing).
    pub fn name(&self) -> Option<String> {
        child::<Name>(&self.0).and_then(|n| n.text())
    }

    /// The parameter names, in declaration order.
    pub fn params(&self) -> Vec<String> {
        child::<ParamList>(&self.0)
            .map(|pl| {
                children::<Param>(pl.syntax())
                    .filter_map(|p| p.name())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The function body block, if present.
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }

    /// The exact source text of this function, trivia included. Used by the
    /// incremental layer as the per-function invalidation unit.
    pub fn source_text(&self) -> String {
        self.0.text().to_string()
    }
}

impl Param {
    pub fn name(&self) -> Option<String> {
        child::<Name>(&self.0).and_then(|n| n.text())
    }
}

impl Block {
    /// Statements in the block, in source order.
    pub fn statements(&self) -> impl Iterator<Item = Stmt> + '_ {
        self.0.children().filter_map(Stmt::cast)
    }
}

/// A statement of the C-subset.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    Return(ReturnStmt),
    If(IfStmt),
    While(WhileStmt),
    Expr(ExprStmt),
}

impl Stmt {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::LET_STMT => LetStmt::cast(node).map(Stmt::Let),
            SyntaxKind::ASSIGN_STMT => AssignStmt::cast(node).map(Stmt::Assign),
            SyntaxKind::RETURN_STMT => ReturnStmt::cast(node).map(Stmt::Return),
            SyntaxKind::IF_STMT => IfStmt::cast(node).map(Stmt::If),
            SyntaxKind::WHILE_STMT => WhileStmt::cast(node).map(Stmt::While),
            SyntaxKind::EXPR_STMT => ExprStmt::cast(node).map(Stmt::Expr),
            _ => None,
        }
    }
}

impl LetStmt {
    /// The bound variable name.
    pub fn name(&self) -> Option<String> {
        child::<Name>(&self.0).and_then(|n| n.text())
    }
    /// The initializer expression.
    pub fn value(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl AssignStmt {
    /// The variable being assigned.
    pub fn name(&self) -> Option<String> {
        child::<Name>(&self.0).and_then(|n| n.text())
    }
    /// The right-hand-side expression.
    pub fn value(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl ReturnStmt {
    /// The returned expression.
    pub fn value(&self) -> Option<Expr> {
        child(&self.0)
    }
}

/// The `else` arm of an `if`: either a plain block or a chained `else if`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ElseArm {
    Block(Block),
    If(IfStmt),
}

impl IfStmt {
    /// The condition expression inside the parentheses.
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
    /// The `then` block (the first block child).
    pub fn then_block(&self) -> Option<Block> {
        child(&self.0)
    }
    /// The `else` arm, if present: the second block child or a nested if.
    pub fn else_arm(&self) -> Option<ElseArm> {
        if let Some(nested) = child::<IfStmt>(&self.0) {
            return Some(ElseArm::If(nested));
        }
        let mut blocks = children::<Block>(&self.0);
        let _then = blocks.next();
        blocks.next().map(ElseArm::Block)
    }
}

impl WhileStmt {
    /// The loop condition inside the parentheses.
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
    /// The loop body.
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl ExprStmt {
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

ast_node!(LiteralExpr, SyntaxKind::LITERAL_EXPR);
ast_node!(NameRef, SyntaxKind::NAME_REF);
ast_node!(BinExpr, SyntaxKind::BIN_EXPR);
ast_node!(PrefixExpr, SyntaxKind::PREFIX_EXPR);
ast_node!(CallExpr, SyntaxKind::CALL_EXPR);
ast_node!(ParenExpr, SyntaxKind::PAREN_EXPR);

/// An expression node in the typed tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    Literal(LiteralExpr),
    NameRef(NameRef),
    Bin(BinExpr),
    Prefix(PrefixExpr),
    Call(CallExpr),
    Paren(ParenExpr),
}

impl AstNode for Expr {
    fn cast(node: SyntaxNode) -> Option<Self> {
        match node.kind() {
            SyntaxKind::LITERAL_EXPR => LiteralExpr::cast(node).map(Expr::Literal),
            SyntaxKind::NAME_REF => NameRef::cast(node).map(Expr::NameRef),
            SyntaxKind::BIN_EXPR => BinExpr::cast(node).map(Expr::Bin),
            SyntaxKind::PREFIX_EXPR => PrefixExpr::cast(node).map(Expr::Prefix),
            SyntaxKind::CALL_EXPR => CallExpr::cast(node).map(Expr::Call),
            SyntaxKind::PAREN_EXPR => ParenExpr::cast(node).map(Expr::Paren),
            _ => None,
        }
    }
    fn syntax(&self) -> &SyntaxNode {
        match self {
            Expr::Literal(e) => e.syntax(),
            Expr::NameRef(e) => e.syntax(),
            Expr::Bin(e) => e.syntax(),
            Expr::Prefix(e) => e.syntax(),
            Expr::Call(e) => e.syntax(),
            Expr::Paren(e) => e.syntax(),
        }
    }
}

/// A binary operator of the C-subset, in source terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Gt,
    LtEq,
    GtEq,
    EqEq,
    NotEq,
}

impl BinOp {
    fn from_token(kind: SyntaxKind) -> Option<Self> {
        Some(match kind {
            SyntaxKind::PLUS => BinOp::Add,
            SyntaxKind::MINUS => BinOp::Sub,
            SyntaxKind::STAR => BinOp::Mul,
            SyntaxKind::SLASH => BinOp::Div,
            SyntaxKind::LT => BinOp::Lt,
            SyntaxKind::GT => BinOp::Gt,
            SyntaxKind::LT_EQ => BinOp::LtEq,
            SyntaxKind::GT_EQ => BinOp::GtEq,
            SyntaxKind::EQ_EQ => BinOp::EqEq,
            SyntaxKind::NOT_EQ => BinOp::NotEq,
            _ => return None,
        })
    }
}

impl LiteralExpr {
    /// The integer value, parsed from the literal's text.
    pub fn value(&self) -> Option<i64> {
        token_text(&self.0, SyntaxKind::INT_LITERAL).and_then(|t| t.parse().ok())
    }
}

impl NameRef {
    /// The referenced identifier text.
    pub fn text(&self) -> Option<String> {
        token_text(&self.0, SyntaxKind::IDENT)
    }
}

impl BinExpr {
    /// Left and right operands. Both are `None` only on malformed input; a
    /// well-formed `BIN_EXPR` always has two expression children.
    pub fn operands(&self) -> (Option<Expr>, Option<Expr>) {
        let mut it = children::<Expr>(&self.0);
        (it.next(), it.next())
    }

    /// The operator between the operands.
    pub fn op(&self) -> Option<BinOp> {
        self.0
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find_map(|t| BinOp::from_token(t.kind()))
    }
}

impl PrefixExpr {
    /// The negated operand of a unary `-`.
    pub fn operand(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl CallExpr {
    /// The callee's name.
    pub fn callee(&self) -> Option<String> {
        child::<NameRef>(&self.0).and_then(|n| n.text())
    }
    /// The argument expressions, in order.
    pub fn args(&self) -> Vec<Expr> {
        self.0
            .children()
            .find(|n| n.kind() == SyntaxKind::ARG_LIST)
            .map(|arg_list| arg_list.children().filter_map(Expr::cast).collect())
            .unwrap_or_default()
    }
}

impl ParenExpr {
    /// The parenthesized inner expression.
    pub fn inner(&self) -> Option<Expr> {
        child(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_functions_and_signatures() {
        let file = SourceFile::parse("int add(int a, int b) { return a + b; }\nint main() { return 0; }");
        let fns: Vec<_> = file.functions().collect();
        assert_eq!(fns.len(), 2);
        assert_eq!(fns[0].name().as_deref(), Some("add"));
        assert_eq!(fns[0].params(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(fns[1].name().as_deref(), Some("main"));
        assert!(fns[1].params().is_empty());
    }

    #[test]
    fn walks_control_flow() {
        let file = SourceFile::parse(
            "int f(int n) { int acc = 0; while (n > 0) { if (n == 2) { acc = acc + 10; } else { acc = acc - 1; } n = n - 1; } return acc; }",
        );
        let f = file.functions().next().unwrap();
        let stmts: Vec<_> = f.body().unwrap().statements().collect();
        assert_eq!(stmts.len(), 3); // let, while, return

        let Stmt::While(w) = &stmts[1] else { panic!("expected while") };
        let Some(Expr::Bin(cond)) = w.condition() else { panic!("expected bin cond") };
        assert_eq!(cond.op(), Some(BinOp::Gt));

        let body: Vec<_> = w.body().unwrap().statements().collect();
        assert_eq!(body.len(), 2); // if, assignment
        let Stmt::If(if_stmt) = &body[0] else { panic!("expected if") };
        let Some(Expr::Bin(if_cond)) = if_stmt.condition() else { panic!() };
        assert_eq!(if_cond.op(), Some(BinOp::EqEq));
        assert!(if_stmt.then_block().is_some());
        assert!(matches!(if_stmt.else_arm(), Some(ElseArm::Block(_))));

        let Stmt::Assign(assign) = &body[1] else { panic!("expected assignment") };
        assert_eq!(assign.name().as_deref(), Some("n"));
        let Some(Expr::Bin(rhs)) = assign.value() else { panic!() };
        assert_eq!(rhs.op(), Some(BinOp::Sub));
    }

    #[test]
    fn walks_expression_shapes() {
        let file = SourceFile::parse("int main() { int x = 1 + f(2); return x; }");
        let main = file.functions().next().unwrap();
        let body = main.body().unwrap();
        let stmts: Vec<_> = body.statements().collect();
        assert_eq!(stmts.len(), 2);

        let Stmt::Let(let_stmt) = &stmts[0] else { panic!("expected let") };
        assert_eq!(let_stmt.name().as_deref(), Some("x"));
        let Some(Expr::Bin(bin)) = let_stmt.value() else { panic!("expected bin expr") };
        let (lhs, rhs) = bin.operands();
        assert!(matches!(lhs, Some(Expr::Literal(_))));
        let Some(Expr::Call(call)) = rhs else { panic!("rhs should be a call") };
        assert_eq!(call.callee().as_deref(), Some("f"));
        assert_eq!(call.args().len(), 1);
    }
}
