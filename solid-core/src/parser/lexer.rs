//! Complete lexer for the SOLID alpha.
//!
//! Handles:
//! - Identifiers, keywords (let, true, false, null, env, secret)
//! - Strings (double & single quoted) with escape sequences
//! - Integers (with underscores) and floats
//! - Unit suffixes immediately after numbers (5s, 512MiB, 100/req/s …)
//! - Operators: =  .  ,  ?:  + - * /  == != < <= > >=  && ||  !
//! - Delimiters: [ ] { } ( )
//! - Newlines and full-line comments (# …)
//! - Exact source spans on every token
//!
//! Healing: the lexer detects unterminated strings and records a repair
//! diagnostic (H001) rather than rejecting the input. All other healing
//! phases live in the parser (comma and bracket repairs) and in
//! `crate::heal` (semantic and type healing).

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity, Span};

/// Result of tokenizing: the token stream, hard lexical errors, and any
/// healing diagnostics discovered along the way.
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub repairs: Vec<Diagnostic>,
    pub errors: Vec<Diagnostic>,
}

/// Build the H001 repair for a string that reached EOF without a closing
/// quote: insert the matching quote. Safe repair (confidence ≥ 0.95).
fn unclosed_string_repair(line: u32, col: u32, quote: char) -> Diagnostic {
    Diagnostic {
        severity: Severity::Info, // safe repair: ≥ AUTO_APPLY
        code: "H001".into(),
        message: format!(
            "string literal reached end of input without a closing {}; inserted",
            quote
        ),
        path: None,
        span: Some(Span {
            line,
            column: col,
            end_line: line,
            end_column: col + 1,
        }),
        repair: Some(Repair {
            original: String::new(),
            suggested: quote.to_string(),
            kind: RepairKind::InsertQuote,
            safe: true,
        }),
        confidence: thresholds::AUTO_APPLY + 0.03,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Eof,
    Newline,

    // Literals & identifiers
    Ident,
    String,
    Integer,
    Float,
    True,
    False,
    Null,

    // Keywords
    Let,
    Env,
    Secret,

    // Operators
    Assign,        // =
    Dot,           // .
    Comma,         // ,
    QuestionColon, // ?:
    Plus,          // +
    Minus,         // -
    Star,          // *
    Slash,         // /
    EqEq,          // ==
    NotEq,         // !=
    Lt,            // <
    LtEq,          // <=
    Gt,            // >
    GtEq,          // >=
    AndAnd,        // &&
    OrOr,          // ||
    Bang,          // !

    // Delimiters
    LBracket, // [
    RBracket, // ]
    LBrace,   // {
    RBrace,   // }
    LParen,   // (
    RParen,   // )

    // Unit suffix emitted right after a number when a known unit is present
    Unit(String),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_ahead(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn make_token(&self, kind: TokenKind, text: String, start_line: u32, start_col: u32) -> Token {
        Token {
            kind,
            text,
            line: start_line,
            col: start_col,
            end_line: self.line,
            end_col: self.col,
        }
    }

    pub fn tokenize(&mut self) -> Lexed {
        let mut tokens = Vec::new();
        let mut repairs: Vec<Diagnostic> = Vec::new();
        let mut errors: Vec<Diagnostic> = Vec::new();

        while let Some(c) = self.peek() {
            match c {
                // Horizontal whitespace
                ' ' | '\t' | '\r' => {
                    self.bump();
                }

                // Newline
                '\n' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Newline, "\n".into(), l, col));
                }

                // Full-line comment
                '#' => {
                    while let Some(ch) = self.peek() {
                        if ch == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }

                // Multi-character operators
                '?' if self.peek_ahead(1) == Some(':') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::QuestionColon, "?:".into(), l, col));
                }
                '=' if self.peek_ahead(1) == Some('=') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::EqEq, "==".into(), l, col));
                }
                '!' if self.peek_ahead(1) == Some('=') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::NotEq, "!=".into(), l, col));
                }
                '<' if self.peek_ahead(1) == Some('=') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::LtEq, "<=".into(), l, col));
                }
                '>' if self.peek_ahead(1) == Some('=') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::GtEq, ">=".into(), l, col));
                }
                '&' if self.peek_ahead(1) == Some('&') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::AndAnd, "&&".into(), l, col));
                }
                '|' if self.peek_ahead(1) == Some('|') => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    self.bump();
                    tokens.push(self.make_token(TokenKind::OrOr, "||".into(), l, col));
                }

                // Single-character operators & delimiters
                '=' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Assign, "=".into(), l, col));
                }
                '.' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Dot, ".".into(), l, col));
                }
                ',' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Comma, ",".into(), l, col));
                }
                '+' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Plus, "+".into(), l, col));
                }
                '-' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Minus, "-".into(), l, col));
                }
                '*' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Star, "*".into(), l, col));
                }
                '/' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Slash, "/".into(), l, col));
                }
                '<' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Lt, "<".into(), l, col));
                }
                '>' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Gt, ">".into(), l, col));
                }
                '!' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::Bang, "!".into(), l, col));
                }
                '[' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::LBracket, "[".into(), l, col));
                }
                ']' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::RBracket, "]".into(), l, col));
                }
                '{' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::LBrace, "{".into(), l, col));
                }
                '}' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::RBrace, "}".into(), l, col));
                }
                '(' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::LParen, "(".into(), l, col));
                }
                ')' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    tokens.push(self.make_token(TokenKind::RParen, ")".into(), l, col));
                }

                // Double-quoted strings (escapes + ${…} left intact for the parser)
                '"' => {
                    let (l, col) = (self.line, self.col);
                    self.bump(); // opening "
                    let mut s = String::new();
                    let mut escaped = false;
                    let mut terminated = false;
                    while let Some(ch) = self.peek() {
                        if escaped {
                            match ch {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                'r' => s.push('\r'),
                                '\\' => s.push('\\'),
                                '"' => s.push('"'),
                                '0' => s.push('\0'),
                                other => {
                                    s.push('\\');
                                    s.push(other);
                                }
                            }
                            escaped = false;
                            self.bump();
                        } else if ch == '\\' {
                            escaped = true;
                            self.bump();
                        } else if ch == '"' {
                            self.bump();
                            terminated = true;
                            break;
                        } else {
                            s.push(ch);
                            self.bump();
                        }
                    }
                    if !terminated {
                        // A dangling escape (string ended inside `\`) has
                        // nothing to escape: preserve the backslash so the
                        // content is not silently altered.
                        if escaped {
                            s.push('\\');
                        }
                        // Healing phase 1: insert the missing closing quote.
                        repairs.push(unclosed_string_repair(l, col, '"'));
                    }
                    tokens.push(self.make_token(TokenKind::String, s, l, col));
                }

                // Single-quoted strings (no interpolation semantics)
                '\'' => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    let mut s = String::new();
                    let mut escaped = false;
                    let mut terminated = false;
                    while let Some(ch) = self.peek() {
                        if escaped {
                            match ch {
                                '\\' => s.push('\\'),
                                '\'' => s.push('\''),
                                other => {
                                    s.push('\\');
                                    s.push(other);
                                }
                            }
                            escaped = false;
                            self.bump();
                        } else if ch == '\\' {
                            escaped = true;
                            self.bump();
                        } else if ch == '\'' {
                            self.bump();
                            terminated = true;
                            break;
                        } else {
                            s.push(ch);
                            self.bump();
                        }
                    }
                    if !terminated {
                        // A dangling escape has nothing to escape; preserve
                        // the backslash so the content is not altered.
                        if escaped {
                            s.push('\\');
                        }
                        // Healing phase 1: insert the missing closing quote.
                        repairs.push(unclosed_string_repair(l, col, '\''));
                    }
                    tokens.push(self.make_token(TokenKind::String, s, l, col));
                }

                // Numbers + optional unit
                c if c.is_ascii_digit() => {
                    let (l, col) = (self.line, self.col);
                    let mut s = String::new();
                    let mut is_float = false;

                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_digit() || ch == '_' {
                            s.push(ch);
                            self.bump();
                        } else if ch == '.' && !is_float {
                            if let Some(next) = self.peek_ahead(1) {
                                if next.is_ascii_digit() {
                                    is_float = true;
                                    s.push(ch);
                                    self.bump();
                                    continue;
                                }
                            }
                            break;
                        } else {
                            break;
                        }
                    }

                    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
                    let kind = if is_float {
                        TokenKind::Float
                    } else {
                        TokenKind::Integer
                    };
                    tokens.push(self.make_token(kind, cleaned, l, col));

                    // Unit must be attached with no intervening space
                    if let Some(u) = self.try_consume_unit() {
                        let (ul, uc) = (self.line, self.col);
                        tokens.push(self.make_token(TokenKind::Unit(u.clone()), u, ul, uc));
                    }
                }

                // Identifiers & keywords
                c if c.is_ascii_alphabetic() || c == '_' => {
                    let (l, col) = (self.line, self.col);
                    let mut s = String::new();
                    while let Some(ch) = self.peek() {
                        if ch.is_ascii_alphanumeric() || ch == '_' {
                            s.push(ch);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    let kind = match s.as_str() {
                        "let" => TokenKind::Let,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        "null" => TokenKind::Null,
                        "env" => TokenKind::Env,
                        "secret" => TokenKind::Secret,
                        _ => TokenKind::Ident,
                    };
                    tokens.push(self.make_token(kind, s, l, col));
                }

                // Unknown character — a hard error in every mode: no rule
                // accepts it, and skipping it would silently change the
                // configuration. Reported with a precise span.
                _ => {
                    let (l, col) = (self.line, self.col);
                    self.bump();
                    errors.push(Diagnostic {
                        severity: Severity::Error,
                        code: "E005".into(),
                        message: format!("unexpected character {:?}", c),
                        path: None,
                        span: Some(Span {
                            line: l,
                            column: col,
                            end_line: l,
                            end_column: col + 1,
                        }),
                        repair: None,
                        confidence: 0.0,
                    });
                }
            }
        }

        tokens.push(self.make_token(TokenKind::Eof, String::new(), self.line, self.col));
        Lexed {
            tokens,
            repairs,
            errors,
        }
    }

    /// Consume a known unit suffix if present.
    fn try_consume_unit(&mut self) -> Option<String> {
        let start_pos = self.pos;
        let start_line = self.line;
        let start_col = self.col;

        let mut s = String::new();
        while let Some(ch) = self.peek() {
            // Unicode-aware so the documented `µs` microsecond suffix is
            // collectable (the whitelist carries it).
            if ch.is_alphanumeric() || ch == '/' || ch == '_' {
                s.push(ch);
                self.bump();
            } else {
                break;
            }
        }

        if s.is_empty() {
            return None;
        }

        const KNOWN: &[&str] = &[
            "ns", "us", "µs", "ms", "s", "m", "h", "d", "B", "KiB", "MiB", "GiB", "TiB", "KB",
            "MB", "GB", "TB", "/s", "/m", "/h", "per_s", "per_m", "per_h", "req/s", "req_s",
            "/req/s", "/req/m", "/req/h", "req/m", "req_m", "req/h", "req_h",
        ];

        if KNOWN.contains(&s.as_str()) {
            Some(s)
        } else {
            // rollback
            self.pos = start_pos;
            self.line = start_line;
            self.col = start_col;
            None
        }
    }
}
