//! Zero-copy lexical analysis via `logos`.
//!
//! `logos` compiles the token rules below into a single DFA, so scanning is a
//! branch-lean pass over the input with no per-token allocation. Trivia
//! (whitespace, line comments) is *not* skipped — it is emitted as ordinary
//! tokens so the parser can build a lossless CST.

use logos::Logos;

use crate::syntax::SyntaxKind;

/// The raw token alphabet. Each variant carries no data; the byte slice it
/// spans is recovered from the `logos` lexer's `slice()`.
///
/// Kept private to the crate: callers consume [`tokenize`], which already maps
/// tokens into the unified [`SyntaxKind`] space the parser and `rowan` share.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
enum RawToken {
    #[regex(r"[ \t\r\n\f]+")]
    Whitespace,
    #[regex(r"//[^\n]*", allow_greedy = true)]
    LineComment,

    #[token("int")]
    IntKw,
    #[token("float")]
    FloatKw,
    #[token("return")]
    ReturnKw,
    #[token("if")]
    IfKw,
    #[token("else")]
    ElseKw,
    #[token("while")]
    WhileKw,

    // A decimal float literal (`3.14`). Declared before the integer rule but
    // resolved by longest-match: `3.14` prefers this over `3` + `.` + `14`.
    #[regex(r"[0-9]+\.[0-9]+")]
    FloatLiteral,
    // A decimal integer literal. Values are parsed later during lowering; the
    // lexer only recognizes the shape.
    #[regex(r"[0-9]+")]
    IntLiteral,
    // C-style identifiers.
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,

    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token(",")]
    Comma,
    #[token(";")]
    Semicolon,
    // Two-character operators are declared alongside their one-character
    // prefixes; `logos` resolves the overlap by longest match, so `<=` never
    // lexes as `<` `=`.
    #[token("==")]
    EqEq,
    #[token("!=")]
    NotEq,
    #[token("<=")]
    LtEq,
    #[token(">=")]
    GtEq,
    #[token("<")]
    Lt,
    #[token(">")]
    Gt,
    #[token("=")]
    Eq,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("&")]
    Amp,
}

impl RawToken {
    #[inline]
    fn to_syntax(self) -> SyntaxKind {
        match self {
            RawToken::Whitespace => SyntaxKind::WHITESPACE,
            RawToken::LineComment => SyntaxKind::LINE_COMMENT,
            RawToken::IntKw => SyntaxKind::INT_KW,
            RawToken::FloatKw => SyntaxKind::FLOAT_KW,
            RawToken::ReturnKw => SyntaxKind::RETURN_KW,
            RawToken::IfKw => SyntaxKind::IF_KW,
            RawToken::ElseKw => SyntaxKind::ELSE_KW,
            RawToken::WhileKw => SyntaxKind::WHILE_KW,
            RawToken::FloatLiteral => SyntaxKind::FLOAT_LITERAL,
            RawToken::IntLiteral => SyntaxKind::INT_LITERAL,
            RawToken::Ident => SyntaxKind::IDENT,
            RawToken::LParen => SyntaxKind::L_PAREN,
            RawToken::RParen => SyntaxKind::R_PAREN,
            RawToken::LBrace => SyntaxKind::L_BRACE,
            RawToken::RBrace => SyntaxKind::R_BRACE,
            RawToken::Comma => SyntaxKind::COMMA,
            RawToken::Semicolon => SyntaxKind::SEMICOLON,
            RawToken::EqEq => SyntaxKind::EQ_EQ,
            RawToken::NotEq => SyntaxKind::NOT_EQ,
            RawToken::LtEq => SyntaxKind::LT_EQ,
            RawToken::GtEq => SyntaxKind::GT_EQ,
            RawToken::Lt => SyntaxKind::LT,
            RawToken::Gt => SyntaxKind::GT,
            RawToken::Eq => SyntaxKind::EQ,
            RawToken::Plus => SyntaxKind::PLUS,
            RawToken::Minus => SyntaxKind::MINUS,
            RawToken::Star => SyntaxKind::STAR,
            RawToken::Slash => SyntaxKind::SLASH,
            RawToken::Amp => SyntaxKind::AMP,
        }
    }
}

/// A single lexed token: its kind plus the exact byte range it covers in the
/// source. Storing offsets (not slices) keeps [`Lexeme`] `'static`-friendly and
/// lets the parser re-borrow the original text when building green tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lexeme {
    pub kind: SyntaxKind,
    /// Half-open byte range `[start, end)` into the source string.
    pub start: usize,
    pub end: usize,
}

impl Lexeme {
    #[inline]
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}

/// Scan `source` into a complete, gap-free sequence of lexemes.
///
/// Invariants upheld for downstream consumers:
///  * Concatenating every lexeme's text reproduces `source` byte-for-byte
///    (losslessness). Unrecognized bytes surface as [`SyntaxKind::LEX_ERROR`]
///    lexemes rather than being dropped.
///  * Lexemes are contiguous and ordered: `out[i].end == out[i + 1].start`.
pub fn tokenize(source: &str) -> Vec<Lexeme> {
    let mut out = Vec::new();
    let mut lexer = RawToken::lexer(source);

    while let Some(result) = lexer.next() {
        let span = lexer.span();
        let kind = match result {
            Ok(tok) => tok.to_syntax(),
            // `logos` yields `Err` for any byte the DFA cannot start a token
            // with. We preserve the offending span as an explicit error token.
            Err(_) => SyntaxKind::LEX_ERROR,
        };
        out.push(Lexeme { kind, start: span.start, end: span.end });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<SyntaxKind> {
        tokenize(src).into_iter().map(|l| l.kind).collect()
    }

    #[test]
    fn lexes_a_function_header() {
        use SyntaxKind::*;
        assert_eq!(
            kinds("int add(int a)"),
            vec![
                INT_KW, WHITESPACE, IDENT, L_PAREN, INT_KW, WHITESPACE, IDENT, R_PAREN,
            ]
        );
    }

    #[test]
    fn tokenization_is_lossless() {
        let src = "int  main() {\n  return 1 + 2; // done\n}\n";
        let reconstructed: String =
            tokenize(src).iter().map(|l| l.text(src)).collect();
        assert_eq!(reconstructed, src, "concatenated lexemes must equal the source");
    }

    #[test]
    fn two_char_operators_win_over_prefixes() {
        use SyntaxKind::*;
        assert_eq!(kinds("<= >= == != < > = - /"), vec![
            LT_EQ, WHITESPACE, GT_EQ, WHITESPACE, EQ_EQ, WHITESPACE, NOT_EQ, WHITESPACE,
            LT, WHITESPACE, GT, WHITESPACE, EQ, WHITESPACE, MINUS, WHITESPACE, SLASH,
        ]);
    }

    #[test]
    fn floats_lex_distinctly_from_ints() {
        use SyntaxKind::*;
        assert_eq!(kinds("float x 3.14 42"), vec![
            FLOAT_KW, WHITESPACE, IDENT, WHITESPACE, FLOAT_LITERAL, WHITESPACE, INT_LITERAL,
        ]);
    }

    #[test]
    fn control_flow_keywords_lex() {
        use SyntaxKind::*;
        assert_eq!(kinds("if else while iffy"), vec![
            IF_KW, WHITESPACE, ELSE_KW, WHITESPACE, WHILE_KW, WHITESPACE, IDENT,
        ]);
    }

    #[test]
    fn unknown_bytes_become_error_tokens() {
        let lexemes = tokenize("int x = @;");
        assert!(
            lexemes.iter().any(|l| l.kind == SyntaxKind::LEX_ERROR),
            "the stray `@` must surface as a LEX_ERROR, not vanish"
        );
        // Losslessness must survive lexing errors too.
        let reconstructed: String =
            lexemes.iter().map(|l| l.text("int x = @;")).collect();
        assert_eq!(reconstructed, "int x = @;");
    }
}
