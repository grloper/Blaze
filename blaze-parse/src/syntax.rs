//! The syntactic vocabulary of Blaze's C-subset and the `rowan` language
//! binding that turns raw `u16` tags into lossless concrete-syntax-tree nodes.
//!
//! A single [`SyntaxKind`] enum names both *tokens* (produced by the lexer) and
//! *composite nodes* (produced by the parser). This is the rust-analyzer idiom:
//! one flat `#[repr(u16)]` enum keeps the `rowan` <-> Blaze conversions branch
//! free and lets the parser address every grammar construct by a single tag.

/// Every terminal and non-terminal Blaze can emit into a CST.
///
/// The discriminants are stable and dense so the `u16` round-trip through
/// `rowan` is a plain transmute-free cast. New kinds must be appended *before*
/// [`SyntaxKind::__Last`], which exists only to bound the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
#[allow(non_camel_case_types)]
pub enum SyntaxKind {
    // ---- Trivia (retained verbatim for lossless round-tripping) ----
    WHITESPACE = 0,
    LINE_COMMENT,

    // ---- Keywords ----
    INT_KW,    // `int`
    RETURN_KW, // `return`
    IF_KW,     // `if`
    ELSE_KW,   // `else`
    WHILE_KW,  // `while`

    // ---- Literals & identifiers ----
    INT_LITERAL, // `42`
    IDENT,       // `main`, `x`

    // ---- Punctuation ----
    L_PAREN,   // `(`
    R_PAREN,   // `)`
    L_BRACE,   // `{`
    R_BRACE,   // `}`
    COMMA,     // `,`
    SEMICOLON, // `;`
    EQ,        // `=`
    PLUS,      // `+`
    MINUS,     // `-`
    STAR,      // `*`
    SLASH,     // `/`
    LT,        // `<`
    GT,        // `>`
    LT_EQ,     // `<=`
    GT_EQ,     // `>=`
    EQ_EQ,     // `==`
    NOT_EQ,    // `!=`
    AMP,       // `&`  (reserved for address-of syntax)

    // ---- Sentinel ----
    /// Lexer error: a byte span that matches no token rule.
    LEX_ERROR,

    // ---- Composite nodes (parser output) ----
    SOURCE_FILE,  // the whole translation unit
    FN,           // `int name(params) { .. }`
    PARAM_LIST,   // `( int a, int b )`
    PARAM,        // `int a`
    BLOCK,        // `{ .. }`
    LET_STMT,     // `int x = expr;`
    ASSIGN_STMT,  // `x = expr;`
    RETURN_STMT,  // `return expr;`
    IF_STMT,      // `if (cond) { .. } else { .. }`
    WHILE_STMT,   // `while (cond) { .. }`
    EXPR_STMT,    // `expr;`
    BIN_EXPR,     // `a + b`, `a < b`, ...
    PREFIX_EXPR,  // `-a`
    CALL_EXPR,    // `f(a, b)`
    ARG_LIST,     // `( a, b )`
    LITERAL_EXPR, // `42`
    NAME_REF,     // `x`  (a use of an identifier)
    NAME,         // `x`  (a binding occurrence of an identifier)
    PAREN_EXPR,   // `( expr )`
    /// Parser error recovery node wrapping the tokens it could not place.
    ERROR,

    /// Upper bound sentinel; never constructed. Keep last.
    #[doc(hidden)]
    __Last,
}

impl SyntaxKind {
    /// Trivia is syntactically inert: whitespace and comments. The parser skips
    /// it for decision-making but the builder still attaches it to the tree so
    /// nothing about the original source is lost.
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT)
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    #[inline]
    fn from(kind: SyntaxKind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

/// The `rowan` language marker binding Blaze's `SyntaxKind` to the generic tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BlazeLanguage {}

impl rowan::Language for BlazeLanguage {
    type Kind = SyntaxKind;

    #[inline]
    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(raw.0 < SyntaxKind::__Last as u16, "invalid SyntaxKind: {raw:?}");
        // SAFETY-free: the assert bounds `raw` inside the enum's dense,
        // `#[repr(u16)]` discriminant range, so the cast is total.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    #[inline]
    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        kind.into()
    }
}

/// A red (cursor) node over Blaze source. Cheap to clone; walks the green tree.
pub type SyntaxNode = rowan::SyntaxNode<BlazeLanguage>;
/// A red token (leaf) over Blaze source.
pub type SyntaxToken = rowan::SyntaxToken<BlazeLanguage>;
/// Either a node or a token — the unit `rowan` yields when walking children.
pub type SyntaxElement = rowan::SyntaxElement<BlazeLanguage>;
