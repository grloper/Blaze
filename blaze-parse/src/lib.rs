//! `blaze-parse` — the lexical and syntactic frontend of the Blaze incremental
//! compiler.
//!
//! Pipeline: [`lexer::tokenize`] (via `logos`) → [`parser::parse`] (hand-written
//! recursive descent) → a lossless `rowan` CST → the typed [`ast`] view that the
//! IR layer lowers from.
//!
//! The crate deliberately holds *no* incremental state of its own. Parsing is a
//! pure function of the source string; `blaze-ir` is what wires it into a
//! `salsa` query graph so re-parses are memoized at the function granularity.

pub mod ast;
pub mod lexer;
pub mod parser;
pub mod syntax;

pub use ast::{AstNode, BinOp, Block, ElseArm, Expr, Function, Lit, SourceFile, Stmt, Ty};
pub use parser::{parse, Parse, SyntaxError};
pub use syntax::{BlazeLanguage, SyntaxKind, SyntaxNode, SyntaxToken};
