//! A hand-written recursive-descent parser that turns a lexeme stream into a
//! lossless `rowan` concrete syntax tree.
//!
//! Design notes:
//!  * **Losslessness.** Every byte of the source — including trivia and the
//!    spans of malformed tokens — is threaded into the green tree. Trivia is
//!    flushed into the *enclosing* node just before the next significant token,
//!    mirroring rust-analyzer's attachment so spans stay intuitive.
//!  * **Error resilience.** The parser never aborts. Unexpected tokens are
//!    wrapped in [`SyntaxKind::ERROR`] nodes and every rule guarantees forward
//!    progress, so a single syntax error can never wedge the loop.
//!
//! Grammar (EBNF) for the strongly-typed `int`-only C-subset:
//! ```text
//! source_file := fn*
//! fn          := 'int' NAME param_list block
//! param_list  := '(' (param (',' param)*)? ')'
//! param       := 'int' NAME
//! block       := '{' stmt* '}'
//! stmt        := let_stmt | assign_stmt | return_stmt | if_stmt | while_stmt | expr_stmt
//! let_stmt    := 'int' NAME '=' expr ';'
//! assign_stmt := NAME '=' expr ';'
//! return_stmt := 'return' expr ';'
//! if_stmt     := 'if' '(' expr ')' block ('else' (if_stmt | block))?
//! while_stmt  := 'while' '(' expr ')' block
//! expr_stmt   := expr ';'
//!
//! // Expressions, lowest to highest precedence (all binaries left-associative):
//! expr     := equality
//! equality := relation (('==' | '!=') relation)*
//! relation := additive (('<' | '>' | '<=' | '>=') additive)*
//! additive := multipl (('+' | '-') multipl)*
//! multipl  := unary (('*' | '/') unary)*
//! unary    := '-' unary | primary
//! primary  := INT_LITERAL | NAME_REF | call | '(' expr ')'
//! call     := NAME_REF arg_list
//! arg_list := '(' (expr (',' expr)*)? ')'
//! ```

use rowan::{GreenNode, GreenNodeBuilder};

use crate::lexer::{tokenize, Lexeme};
use crate::syntax::{SyntaxKind, SyntaxNode};

/// A diagnostic produced during parsing: a message anchored at a byte offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    /// Byte offset into the source where the problem was detected.
    pub offset: usize,
}

/// The result of parsing: an immutable green tree plus any diagnostics.
///
/// Cloning is cheap — the green node is reference-counted — which is what lets
/// the incremental layer stash parses in a query database.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
}

impl Parse {
    /// A fresh red-tree cursor rooted at [`SyntaxKind::SOURCE_FILE`].
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// Diagnostics gathered while parsing (empty on well-formed input).
    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }
}

/// Parse a complete translation unit. Never fails; inspect [`Parse::errors`].
pub fn parse(source: &str) -> Parse {
    let tokens = tokenize(source);
    Parser::new(source, tokens).parse_source_file()
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Lexeme>,
    /// Index into `tokens` of the next lexeme to consider.
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: Vec<Lexeme>) -> Self {
        Parser { source, tokens, pos: 0, builder: GreenNodeBuilder::new(), errors: Vec::new() }
    }

    // ---- Token cursor primitives -------------------------------------------

    /// Emit all trivia at the cursor into the currently open node and advance
    /// past it, so decision-making only ever sees significant tokens.
    fn flush_trivia(&mut self) {
        while let Some(lexeme) = self.tokens.get(self.pos) {
            if !lexeme.kind.is_trivia() {
                break;
            }
            self.builder.token(lexeme.kind.into(), lexeme.text(self.source));
            self.pos += 1;
        }
    }

    /// Kind of the next significant token, or `None` at end of input.
    fn current(&mut self) -> Option<SyntaxKind> {
        self.flush_trivia();
        self.tokens.get(self.pos).map(|l| l.kind)
    }

    /// Non-mutating lookahead: the kind of the `n`-th significant token from the
    /// cursor (`n == 0` is the current token), skipping trivia.
    fn nth(&self, n: usize) -> Option<SyntaxKind> {
        let mut seen = 0;
        for lexeme in &self.tokens[self.pos..] {
            if lexeme.kind.is_trivia() {
                continue;
            }
            if seen == n {
                return Some(lexeme.kind);
            }
            seen += 1;
        }
        None
    }

    fn at(&mut self, kind: SyntaxKind) -> bool {
        self.current() == Some(kind)
    }

    /// Byte offset of the current significant token (or EOF).
    fn offset(&self) -> usize {
        self.tokens
            .get(self.pos)
            .map(|l| l.start)
            .unwrap_or_else(|| self.source.len())
    }

    /// Open a composite node, first flushing leading trivia into the parent so
    /// it attaches outside the new node.
    fn start(&mut self, kind: SyntaxKind) {
        self.flush_trivia();
        self.builder.start_node(kind.into());
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }

    /// Consume the current significant token as a leaf. Caller must ensure one
    /// exists (via [`Self::at`]/[`Self::current`]).
    fn bump(&mut self) {
        self.flush_trivia();
        let lexeme = self.tokens[self.pos];
        self.builder.token(lexeme.kind.into(), lexeme.text(self.source));
        self.pos += 1;
    }

    /// Consume `kind` if present; otherwise record an error *without* advancing.
    /// Forward progress is the responsibility of the surrounding loop, which
    /// always has an [`Self::error_bump`] fallback.
    fn expect(&mut self, kind: SyntaxKind) {
        if self.at(kind) {
            self.bump();
        } else {
            self.error(format!("expected {kind:?}"));
        }
    }

    fn error(&mut self, message: String) {
        self.errors.push(SyntaxError { message, offset: self.offset() });
    }

    /// Record an error and consume one token into an ERROR node, guaranteeing
    /// progress. At EOF only the diagnostic is recorded.
    fn error_bump(&mut self, message: String) {
        self.error(message);
        if self.current().is_some() {
            self.start(SyntaxKind::ERROR);
            self.bump();
            self.finish();
        }
    }

    // ---- Grammar rules ------------------------------------------------------

    fn parse_source_file(mut self) -> Parse {
        self.builder.start_node(SyntaxKind::SOURCE_FILE.into());
        while let Some(kind) = self.current() {
            match kind {
                SyntaxKind::INT_KW => self.function(),
                _ => self.error_bump("expected `int` to begin a function".into()),
            }
        }
        // Trailing trivia (e.g. a final newline) belongs to the file node.
        self.flush_trivia();
        self.builder.finish_node();

        Parse { green: self.builder.finish(), errors: self.errors }
    }

    fn function(&mut self) {
        self.start(SyntaxKind::FN);
        self.expect(SyntaxKind::INT_KW); // return type
        self.name();
        self.param_list();
        self.block();
        self.finish();
    }

    /// A binding occurrence of an identifier, e.g. the `x` in `int x = 1;`.
    fn name(&mut self) {
        self.start(SyntaxKind::NAME);
        self.expect(SyntaxKind::IDENT);
        self.finish();
    }

    fn param_list(&mut self) {
        self.start(SyntaxKind::PARAM_LIST);
        self.expect(SyntaxKind::L_PAREN);
        if !self.at(SyntaxKind::R_PAREN) && self.current().is_some() {
            self.param();
            while self.at(SyntaxKind::COMMA) {
                self.bump();
                self.param();
            }
        }
        self.expect(SyntaxKind::R_PAREN);
        self.finish();
    }

    fn param(&mut self) {
        self.start(SyntaxKind::PARAM);
        self.expect(SyntaxKind::INT_KW);
        self.name();
        self.finish();
    }

    fn block(&mut self) {
        self.start(SyntaxKind::BLOCK);
        self.expect(SyntaxKind::L_BRACE);
        while let Some(kind) = self.current() {
            if kind == SyntaxKind::R_BRACE {
                break;
            }
            self.statement();
        }
        self.expect(SyntaxKind::R_BRACE);
        self.finish();
    }

    fn statement(&mut self) {
        match self.current() {
            Some(SyntaxKind::INT_KW) => self.let_stmt(),
            Some(SyntaxKind::RETURN_KW) => self.return_stmt(),
            Some(SyntaxKind::IF_KW) => self.if_stmt(),
            Some(SyntaxKind::WHILE_KW) => self.while_stmt(),
            // `x = ...;` needs one token of lookahead to distinguish an
            // assignment from an expression statement starting with a name.
            Some(SyntaxKind::IDENT) if self.nth(1) == Some(SyntaxKind::EQ) => self.assign_stmt(),
            Some(_) => self.expr_stmt(),
            None => {}
        }
    }

    fn let_stmt(&mut self) {
        self.start(SyntaxKind::LET_STMT);
        self.expect(SyntaxKind::INT_KW);
        self.name();
        self.expect(SyntaxKind::EQ);
        self.expr();
        self.expect(SyntaxKind::SEMICOLON);
        self.finish();
    }

    fn assign_stmt(&mut self) {
        self.start(SyntaxKind::ASSIGN_STMT);
        self.name();
        self.expect(SyntaxKind::EQ);
        self.expr();
        self.expect(SyntaxKind::SEMICOLON);
        self.finish();
    }

    fn return_stmt(&mut self) {
        self.start(SyntaxKind::RETURN_STMT);
        self.expect(SyntaxKind::RETURN_KW);
        self.expr();
        self.expect(SyntaxKind::SEMICOLON);
        self.finish();
    }

    fn if_stmt(&mut self) {
        self.start(SyntaxKind::IF_STMT);
        self.expect(SyntaxKind::IF_KW);
        self.expect(SyntaxKind::L_PAREN);
        self.expr();
        self.expect(SyntaxKind::R_PAREN);
        self.block();
        if self.at(SyntaxKind::ELSE_KW) {
            self.bump(); // `else`
            if self.at(SyntaxKind::IF_KW) {
                // `else if` chains as a nested IF_STMT child.
                self.if_stmt();
            } else {
                self.block();
            }
        }
        self.finish();
    }

    fn while_stmt(&mut self) {
        self.start(SyntaxKind::WHILE_STMT);
        self.expect(SyntaxKind::WHILE_KW);
        self.expect(SyntaxKind::L_PAREN);
        self.expr();
        self.expect(SyntaxKind::R_PAREN);
        self.block();
        self.finish();
    }

    fn expr_stmt(&mut self) {
        self.start(SyntaxKind::EXPR_STMT);
        self.expr();
        self.expect(SyntaxKind::SEMICOLON);
        self.finish();
    }

    /// Binary operators, grouped by precedence level from loosest to tightest.
    /// Each level folds left-associatively via a checkpoint, so `a - b - c`
    /// becomes `(a - b) - c`.
    const BIN_LEVELS: &'static [&'static [SyntaxKind]] = &[
        &[SyntaxKind::EQ_EQ, SyntaxKind::NOT_EQ],
        &[SyntaxKind::LT, SyntaxKind::GT, SyntaxKind::LT_EQ, SyntaxKind::GT_EQ],
        &[SyntaxKind::PLUS, SyntaxKind::MINUS],
        &[SyntaxKind::STAR, SyntaxKind::SLASH],
    ];

    fn expr(&mut self) {
        self.expr_at_level(0);
    }

    fn expr_at_level(&mut self, level: usize) {
        let Some(ops) = Self::BIN_LEVELS.get(level) else {
            return self.unary();
        };
        self.flush_trivia();
        let checkpoint = self.builder.checkpoint();
        self.expr_at_level(level + 1);
        while self.current().is_some_and(|k| ops.contains(&k)) {
            self.builder.start_node_at(checkpoint, SyntaxKind::BIN_EXPR.into());
            self.bump(); // the operator
            self.expr_at_level(level + 1);
            self.finish();
        }
    }

    /// `unary := '-' unary | primary`
    fn unary(&mut self) {
        if self.at(SyntaxKind::MINUS) {
            self.start(SyntaxKind::PREFIX_EXPR);
            self.bump(); // '-'
            self.unary();
            self.finish();
        } else {
            self.primary();
        }
    }

    fn primary(&mut self) {
        match self.current() {
            Some(SyntaxKind::INT_LITERAL) => {
                self.start(SyntaxKind::LITERAL_EXPR);
                self.bump();
                self.finish();
            }
            Some(SyntaxKind::IDENT) => {
                if self.nth(1) == Some(SyntaxKind::L_PAREN) {
                    self.call_expr();
                } else {
                    self.start(SyntaxKind::NAME_REF);
                    self.bump();
                    self.finish();
                }
            }
            Some(SyntaxKind::L_PAREN) => {
                self.start(SyntaxKind::PAREN_EXPR);
                self.bump(); // '('
                self.expr();
                self.expect(SyntaxKind::R_PAREN);
                self.finish();
            }
            _ => self.error_bump("expected an expression".into()),
        }
    }

    fn call_expr(&mut self) {
        self.start(SyntaxKind::CALL_EXPR);
        // The callee name.
        self.start(SyntaxKind::NAME_REF);
        self.expect(SyntaxKind::IDENT);
        self.finish();
        self.arg_list();
        self.finish();
    }

    fn arg_list(&mut self) {
        self.start(SyntaxKind::ARG_LIST);
        self.expect(SyntaxKind::L_PAREN);
        if !self.at(SyntaxKind::R_PAREN) && self.current().is_some() {
            self.expr();
            while self.at(SyntaxKind::COMMA) {
                self.bump();
                self.expr();
            }
        }
        self.expect(SyntaxKind::R_PAREN);
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_program_without_errors() {
        let parse = parse("int add(int a, int b) {\n  return a + b;\n}\n");
        assert!(parse.errors().is_empty(), "unexpected errors: {:?}", parse.errors());
        assert_eq!(parse.syntax().kind(), SyntaxKind::SOURCE_FILE);
    }

    #[test]
    fn round_trips_source_text() {
        let src = "int main() {\n  int x = 1 + 2;\n  return add(x, 3);\n}\n";
        let parse = parse(src);
        // The red tree's text must equal the original source byte-for-byte.
        assert_eq!(parse.syntax().text().to_string(), src);
    }

    #[test]
    fn parses_control_flow_and_operators() {
        let src = "\
int abs_diff(int a, int b) {
    int d = a - b;
    if (d < 0) {
        d = 0 - d;
    }
    return d;
}

int count(int n) {
    int i = 0;
    int acc = 0;
    while (i < n) {
        acc = acc + i * 2 / 1;
        i = i + 1;
    }
    return acc;
}
";
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "unexpected errors: {:?}", parse.errors());
        assert_eq!(parse.syntax().text().to_string(), src, "must stay lossless");
    }

    #[test]
    fn parses_else_if_chains_and_unary_minus() {
        let src = "int f(int x) { if (x == 0) { return -1; } else if (x != 1) { return x; } else { return 0; } }";
        let parse = parse(src);
        assert!(parse.errors().is_empty(), "unexpected errors: {:?}", parse.errors());
        assert_eq!(parse.syntax().text().to_string(), src);
    }

    #[test]
    fn recovers_from_a_missing_semicolon() {
        let parse = parse("int main() {\n  return 1\n}\n");
        assert!(!parse.errors().is_empty(), "missing `;` should be reported");
        // Even on error the tree stays lossless.
        assert_eq!(
            parse.syntax().text().to_string(),
            "int main() {\n  return 1\n}\n"
        );
    }
}
