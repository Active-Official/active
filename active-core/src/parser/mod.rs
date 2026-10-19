//! Full recursive-descent parser for the Active alpha.
//!
//! Supported surface language:
//! - `let name = expr`
//! - `[table]` blocks with `key = expr`
//! - top-level and nested `key = expr`
//! - `env.NAME ?: default`
//! - `secret("…")` / `secret(env.NAME)`
//! - integers, floats, bools, null, strings (single & double quoted)
//! - unit suffixes on numbers (`5s`, `512MiB`, `100/req/s` …) → Quantity nodes
//! - full string interpolation `"…${ident}…"` (and nested simple expressions)
//! - arrays `[ … ]`
//! - inline tables `{ k = v, … }`
//! - binary operators with correct precedence:
//!   `||` `&&` `==` `!=` `<` `<=` `>` `>=` `+` `-` `*` `/`
//! - unary `-` and `!`
//! - parentheses for grouping
//!
//! After parsing, a resolution pass turns Ident nodes that match `let`
//! bindings into Ref nodes and builds the dependency graph.

mod lexer;

use lexer::ESCAPED_DOLLAR;
use lexer::SENTINEL_POS;
pub use lexer::{Lexed, Lexer, Token, TokenKind};

/// Extract the span of a token.
fn span_of(tok: &Token) -> Span {
    Span {
        line: tok.line,
        column: tok.col,
        end_line: tok.end_line,
        end_column: tok.end_col,
    }
}

/// Parse an `@intent` payload as one string literal, mirroring the
/// language's escape rules (double-quoted: `\n \t \r \\ \" \0 \$`;
/// single-quoted: `\\ \'`; anything else kept verbatim). Unlike lexer
/// strings there is no healing inside comments: an unterminated
/// literal or trailing text is an error for E021.
fn parse_annotation_string(payload: &str) -> Result<String, String> {
    let mut chars = payload.chars().peekable();
    let quote = chars.next().ok_or("expected a string literal")?;
    if quote != '"' && quote != '\'' {
        return Err("expected a string literal".to_string());
    }
    let mut out = String::new();
    let mut escaped = false;
    for c in chars.by_ref() {
        if escaped {
            match (quote, c) {
                ('"', 'n') => out.push('\n'),
                ('"', 't') => out.push('\t'),
                ('"', 'r') => out.push('\r'),
                ('"', '0') => out.push('\0'),
                ('"', '$') => out.push('$'),
                (_, '\\') => out.push('\\'),
                ('"', '"') => out.push('"'),
                ('\'', '\'') => out.push('\''),
                _ => {
                    out.push('\\');
                    out.push(c);
                }
            }
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            // Closing quote: only trailing whitespace may follow.
            for rest in chars.by_ref() {
                if !rest.is_whitespace() {
                    return Err("unexpected text after the string literal".to_string());
                }
            }
            return Ok(out);
        } else {
            out.push(c);
        }
    }
    Err("unterminated string literal".to_string())
}

/// Maximum expression nesting depth (parenthesized expressions, unary
/// operator chains, nested arrays, nested inline tables). A hard limit
/// that prevents stack exhaustion from pathological input — the
/// parser-side half of the execution budget (see C-037).
const MAX_EXPR_DEPTH: usize = 64;

/// Maximum structural (table/block) nesting depth. Bounds parser recursion
/// on pathological input; exceeding it is E011 (see C-037).
const MAX_BLOCK_DEPTH: usize = 64;

/// Maximum schema-type nesting depth (`Array<Array<…>>`,
/// `Table { … Table { … } … }`). Bounds parser recursion on pathological
/// input; exceeding it is E004 (see C-037).
const MAX_SCHEMA_DEPTH: usize = 64;

use crate::diag::{thresholds, Diagnostic, Repair, RepairKind, Severity, Span};
use crate::ir::{NodeId, SchemaField, SemanticTag, *};
use crate::types::{ActiveType, Unit};
use indexmap::IndexMap;

/// Host-provided include source resolver (F-016): maps a canonical path
/// string to file content. Replaces ONLY the filesystem read; path
/// canonicalization, parent-escape policy, and trust verification around
/// the read are unchanged. `Err` surfaces exactly like a read failure.
///
/// The `arc_with_non_send_sync` lint is allowed here with justification:
/// the parser is single-threaded by construction and the FFI contract
/// confines modules to the calling thread, so this Arc is intra-thread
/// shared ownership (cheap cloning into sub-parsers), never cross-thread
/// sharing.
#[allow(clippy::arc_with_non_send_sync)]
pub type IncludeLoader = std::sync::Arc<dyn Fn(&str) -> Result<String, String>>;

/// Options controlling parsing and healing behavior.
#[derive(Clone)]
pub struct ParseOptions {
    /// When true, medium-confidence repairs (0.70–0.94) are applied and
    /// reported as warnings. When false (strict, the default), they are
    /// declined and the underlying syntax diagnostic stands.
    pub resilient: bool,
    /// When true, `include` statements are loaded from the filesystem
    /// (spec §2.7). False by default: string-based parsing stays hermetic
    /// (the fuzz harness depends on this). `parse_file` enables it.
    pub enable_includes: bool,
    /// When true (default), recorded includes are resolved and merged.
    /// `active-fmt` disables it so formatting preserves include statements
    /// alongside local keys instead of a composed tree.
    pub resolve_includes: bool,
    /// When true, include paths may be absolute or escape the including
    /// directory via `..`. False by default (E015 otherwise).
    pub allow_parent_escapes: bool,
    /// Host-provided source resolver consulted INSTEAD of the filesystem
    /// read when set (F-016). `None` preserves all legacy behavior.
    pub include_loader: Option<IncludeLoader>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            resilient: false,
            enable_includes: false,
            resolve_includes: true,
            allow_parent_escapes: false,
            include_loader: None,
        }
    }
}

impl ParseOptions {
    /// Strict defaults (const-compatible for test constants).
    pub const fn strict() -> Self {
        Self {
            resilient: false,
            enable_includes: false,
            resolve_includes: true,
            allow_parent_escapes: false,
            include_loader: None,
        }
    }

    /// Resilient-mode defaults (const-compatible for test constants).
    pub const fn resilient_mode() -> Self {
        Self {
            resilient: true,
            enable_includes: false,
            resolve_includes: true,
            allow_parent_escapes: false,
            include_loader: None,
        }
    }
}

pub fn parse(source: &str, name: &str) -> Result<Module, Vec<Diagnostic>> {
    parse_with(source, name, ParseOptions::default())
}

pub fn parse_with(
    source: &str,
    name: &str,
    options: ParseOptions,
) -> Result<Module, Vec<Diagnostic>> {
    let (module, errors) = parse_recovering(source, name, options);
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(errors)
    }
}

/// Parse buffer text as if it were the file at `path`: `include`
/// statements resolve against the file's directory (spec §2.7), and
/// diagnostics carry the file's identity. The language server's
/// dirty-buffer path (C-043): unsaved edits analyze with the saved
/// file's include context. Includes are enabled regardless of the
/// passed options' `enable_includes` flag; every other option is
/// honored as given.
pub fn parse_text_with_file(
    source: &str,
    name: &str,
    options: ParseOptions,
    path: &std::path::Path,
) -> Result<Module, Vec<Diagnostic>> {
    let mut options = options;
    options.enable_includes = true;
    let (module, errors) = parse_recovering_impl(source, name, options, Some(path), None);
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(errors)
    }
}

/// Parse buffer text as above but always return the module, even
/// when diagnostics were recorded. Tooling (editor diagnostics on
/// broken files) needs the partial tree; crucially, the file context
/// is preserved so `include` statements never fall back to resolving
/// from the process working directory.
pub fn parse_text_recovering_with_file(
    source: &str,
    name: &str,
    options: ParseOptions,
    path: &std::path::Path,
) -> (Module, Vec<Diagnostic>) {
    let mut options = options;
    options.enable_includes = true;
    parse_recovering_impl(source, name, options, Some(path), None)
}

/// Parse a file from the filesystem, resolving `include` statements
/// against the file's directory (spec §2.7). Includes are enabled
/// regardless of the passed options' `enable_includes` flag; every other
/// option is honored as given.
pub fn parse_file(
    path: &std::path::Path,
    options: ParseOptions,
) -> Result<Module, Vec<Diagnostic>> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        vec![Diagnostic {
            severity: Severity::Error,
            code: "E017".into(),
            message: format!("cannot read file {}: {e}", path.display()),
            path: None,
            span: None,
            repair: None,
            confidence: 1.0,
            file: Some(path.to_string_lossy().into_owned()),
        }]
    })?;
    let mut options = options;
    options.enable_includes = true;
    let (module, errors) =
        parse_recovering_impl(&src, &path.to_string_lossy(), options, Some(path), None);
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(errors)
    }
}

/// Parse a file with overlay enforcement (C-052): identical to
/// `parse_file`, but every `include` must carry a valid sibling
/// signature from `trusted` (E032 otherwise). Use with
/// `--trust-keys`; without it, `parse_file` preserves legacy
/// behavior exactly.
pub fn parse_file_with_trust(
    path: &std::path::Path,
    options: ParseOptions,
    trusted: &crate::sign::TrustedKeys,
) -> Result<Module, Vec<Diagnostic>> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        vec![Diagnostic {
            severity: Severity::Error,
            code: "E017".into(),
            message: format!("cannot read file {}: {e}", path.display()),
            path: None,
            span: None,
            repair: None,
            confidence: 1.0,
            file: Some(path.to_string_lossy().into_owned()),
        }]
    })?;
    let mut options = options;
    options.enable_includes = true;
    let (module, errors) = parse_recovering_impl(
        &src,
        &path.to_string_lossy(),
        options,
        Some(path),
        Some(trusted.clone()),
    );
    if errors.is_empty() {
        Ok(module)
    } else {
        Err(errors)
    }
}

/// Parse and return the module even when diagnostics were recorded.
///
/// Tooling (editor diagnostics on broken files) and recovery tests need the
/// partial tree; `parse` stays the strict entry point that fails on any
/// error. Trivia and spans remain accurate for the parts that parsed.
pub fn parse_recovering(
    source: &str,
    name: &str,
    options: ParseOptions,
) -> (Module, Vec<Diagnostic>) {
    parse_recovering_impl(source, name, options, None, None)
}

fn parse_recovering_impl(
    source: &str,
    name: &str,
    options: ParseOptions,
    root_file: Option<&std::path::Path>,
    trust: Option<crate::sign::TrustedKeys>,
) -> (Module, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(source);
    let mut lexed = lexer.tokenize();
    let n_comments = lexed.comments.len();
    // Root comments stamp with the session file (None for string
    // input): annotation block scans match on (line, file), so the
    // stamp must equal `current_file_name()` during the root parse.
    let root_stamp = root_file.map(|p| p.to_string_lossy().into_owned());
    let mut parser = Parser {
        tokens: lexed.tokens,
        pos: 0,
        module: Module::new(name),
        errors: Vec::new(),
        ident_scopes: Vec::new(),
        depth: 0,
        options,
        block_depth: 0,
        schema_depth: 0,
        comments: lexed.comments,
        comment_files: vec![root_stamp; n_comments],
        statements: Vec::new(),
        blocks: Vec::new(),
        current_path: Vec::new(),
        brace_tables: 0,
        indent_tables: 0,
        current_file: root_file.map(|p| p.to_path_buf()),
        include_stack: Vec::new(),
        in_include: false,
        in_profile: false,
        array_depth: 0,
        binding_spliced: std::collections::HashSet::new(),
        pending_profiles: Vec::new(),
        trust,
    };
    // Lexer diagnostics carry no file; stamp the session file (includes
    // attribute their own file when their sub-parse runs).
    let lex_file = parser.current_file_name();
    for d in lexed.errors.iter_mut().chain(lexed.repairs.iter_mut()) {
        if d.file.is_none() {
            d.file = lex_file.clone();
        }
    }
    parser.module.healing.extend(lexed.repairs);
    parser.errors.extend(lexed.errors);
    parser.parse_document();
    // Schema extraction before graph build: `schema` tables leave value
    // fields for the schema tree (spec §2.10), so neither sibling
    // resolution nor evaluation ever sees them as data.
    parser.extract_schemas();
    // Dependency metadata leaves value fields the same way (spec §2.11).
    parser.extract_depends_on();
    parser.attach_trivia();
    parser.module.source_style = if parser.indent_tables > parser.brace_tables {
        Style::Indent
    } else {
        Style::Brace
    };
    parser.build_graph();
    (parser.module, parser.errors)
}

/// Position-vector read with sentinel substitution: marked entries inherit
/// the nearest preceding real position, because marks must never reach
/// line/column arithmetic. `fallback` covers missing entries, as before.
fn pos_entry(pos: &[(u32, u32)], k: usize, fallback: (u32, u32)) -> (u32, u32) {
    let mut j = k;
    loop {
        match pos.get(j) {
            Some(&e) if e != SENTINEL_POS => return e,
            Some(_) => {
                if j == 0 {
                    return fallback;
                }
                j -= 1;
            }
            None => return fallback,
        }
    }
}

/// Compute each comment's normative attachment (spec/dual-syntax.md §7.2)
/// and store it in the module's trivia list.
impl Parser {
    fn attach_trivia(&mut self) {
        let comments = std::mem::take(&mut self.comments);
        // Kept aligned with `comments` (taken together): files stamp
        // every comment so attachment matches (line, file).
        let comment_files = std::mem::take(&mut self.comment_files);
        let statements = std::mem::take(&mut self.statements);
        let blocks = std::mem::take(&mut self.blocks);

        let mut trivia = Vec::with_capacity(comments.len());
        for ((text, span), file) in comments.into_iter().zip(comment_files) {
            let attachment = self.attach_one(&span, &file, &statements, &blocks);
            trivia.push(TriviaComment {
                text,
                span,
                attachment,
                file,
            });
        }
        self.module.trivia = trivia;
    }

    fn attach_one(
        &self,
        span: &Span,
        file: &Option<String>,
        stmts: &[StmtInfo],
        blocks: &[BlockInfo],
    ) -> Attachment {
        let line = span.line;
        let level = span.column.saturating_sub(1) as usize;

        // Rule 1: after code on the same line as any statement — either the
        // line the statement begins on (a table header, `[app] # note`) or
        // the line it ends on (a multi-line value). Same file only:
        // line numbers repeat across includes.
        if let Some(stmt) = stmts
            .iter()
            .filter(|s| &s.file == file && (s.start_line == line || s.end_line == line))
            .max_by_key(|s| s.start_line)
        {
            return Attachment::Trailing(qualified(&stmt.path, &stmt.key));
        }

        // Determine the comment's block (spec §7.1): the innermost block
        // that still owns the comment's line, preferring one whose body is
        // at exactly the comment's own indentation level.
        let owner = self.owning_block(line, level, file, stmts, blocks);
        if let Some(b) = &owner {
            // Rule 2: any statement (including a nested table header)
            // follows within the same block.
            if let Some(stmt) = stmts
                .iter()
                .filter(|s| &s.file == file && s.start_line > line && s.path == b.path)
                .min_by_key(|s| s.start_line)
            {
                return Attachment::Leading(qualified(&stmt.path, &stmt.key));
            }
            // Rule 3: trailing comment of that block.
            return Attachment::TrailingBlock(b.path.clone());
        }

        // Rule 2 at document level, then rule 4.
        if let Some(stmt) = stmts
            .iter()
            .filter(|s| &s.file == file && s.start_line > line && s.path.is_empty())
            .min_by_key(|s| s.start_line)
        {
            return Attachment::Leading(qualified(&stmt.path, &stmt.key));
        }
        Attachment::TrailingDocument
    }

    /// The innermost block owning `line` at indentation `level`, per §7.1.
    fn owning_block(
        &self,
        line: u32,
        level: usize,
        file: &Option<String>,
        stmts: &[StmtInfo],
        blocks: &[BlockInfo],
    ) -> Option<BlockInfo> {
        // Score prefers an exact indentation-level match, then a deeper
        // block, then a later start.
        fn score(b: &BlockInfo, level: usize) -> (u8, usize, u32) {
            (if b.level == level { 1 } else { 0 }, b.level, b.start_line)
        }
        let mut best: Option<&BlockInfo> = None;
        for b in blocks {
            if &b.file != file || b.start_line > line {
                continue;
            }
            // The block owns lines up to the next statement at or above its
            // own level *after this comment*.
            let next_sibling = stmts
                .iter()
                .filter(|s| &s.file == file && s.start_line > line && s.level <= b.level)
                .min_by_key(|s| s.start_line)
                .map(|s| s.start_line)
                .unwrap_or(u32::MAX);
            if line >= next_sibling {
                continue;
            }
            let better = match best {
                None => true,
                Some(prev) => score(b, level) >= score(prev, level),
            };
            if better {
                best = Some(b);
            }
        }
        best.cloned()
    }
}

/// The fully qualified key of a statement (`table.key`, or just `key` at
/// document level) — the identity comments attach to.
fn qualified(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// Validate and split a patch path (`a.b[0].c`) into segments.
/// Keys follow identifier syntax (ASCII letter or `_` first, then ASCII
/// alphanumerics or `_`); an index suffix `[n]` may repeat (`a[0][1]`
/// walks nested arrays). Empty paths/segments, malformed brackets, and
/// non-integer indices are E019 at parse time; missing targets are W004
/// at apply.
pub fn validate_patch_path(path: &str) -> Result<Vec<PathSeg>, String> {
    fn is_key(s: &str) -> bool {
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }
    if path.is_empty() {
        return Err("empty path".into());
    }
    let mut segs = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err("empty segment".into());
        }
        // Split `key`, `key[0]`, `key[0][1]` (multi-index, applied left
        // to right).
        let mut rest = part;
        let key_end = rest.find('[').unwrap_or(rest.len());
        let key = &rest[..key_end];
        if !is_key(key) {
            return Err(format!("invalid key `{key}` in segment `{part}`"));
        }
        segs.push(PathSeg::Key(key.to_string()));
        rest = &rest[key_end..];
        while !rest.is_empty() {
            if !rest.starts_with('[') {
                return Err(format!("unexpected text in segment `{part}`"));
            }
            let close = rest
                .find(']')
                .ok_or_else(|| format!("unclosed bracket in segment `{part}`"))?;
            let idx: usize = rest[1..close]
                .parse()
                .map_err(|_| format!("array index must be a non-negative integer in `{part}`"))?;
            segs.push(PathSeg::Index(idx));
            rest = &rest[close + 1..];
        }
    }
    Ok(segs)
}

/// One segment of a patch path (spec §2.9): a table key, optionally
/// followed by an array index (`servers[0]` lexes as Key+Index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(usize),
}
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    module: Module,
    errors: Vec<Diagnostic>,
    /// Stack of per-scope (owning key, Ident node id) pairs, used to resolve
    /// sibling-key references (e.g. `can_scale = !is_prod` inside the same
    /// table). The owning key prevents a key's own value from resolving to
    /// itself.
    ident_scopes: Vec<Vec<(String, NodeId)>>,
    /// Current expression nesting depth, guarded by MAX_EXPR_DEPTH.
    depth: usize,
    /// Healing mode for this parse.
    options: ParseOptions,
    /// Current block nesting depth, guarded by MAX_BLOCK_DEPTH.
    block_depth: usize,
    /// Current schema-type nesting depth (`Array<…>` / `Table {…}`),
    /// guarded by MAX_SCHEMA_DEPTH. Schema recursion bypasses both the
    /// expression and block guards, so it needs its own counter.
    schema_depth: usize,
    /// Comments from the lexer, awaiting attachment computation.
    comments: Vec<(String, Span)>,
    /// Source file per comment, parallel to `comments`: `None` for the
    /// root document, `Some` for included files (C-033). Lines repeat
    /// across files, so annotation block scans must match on
    /// (line, file) — never bare lines.
    comment_files: Vec<Option<String>>,
    /// Statement records used for comment attachment (spec §7).
    statements: Vec<StmtInfo>,
    /// Block (table) records used for comment attachment (spec §7).
    blocks: Vec<BlockInfo>,
    /// Enclosing table path while parsing.
    current_path: Vec<String>,
    /// Counts of brace-style and indentation-style table declarations.
    brace_tables: usize,
    indent_tables: usize,
    /// File currently being parsed (None for string input). Diagnostics
    /// record it; include paths resolve against its directory.
    current_file: Option<std::path::PathBuf>,
    /// Canonicalized files on the include stack (cycle detection, C-025).
    include_stack: Vec<std::path::PathBuf>,
    /// True while parsing an included file: `let` bindings use
    /// first-wins (local) semantics instead of overwriting.
    in_include: bool,
    /// True while parsing a profile body: `schema` tables are E020
    /// there (profiles merge values; the root schema validates).
    in_profile: bool,
    /// Array nesting depth: `schema` blocks are E020 inside array
    /// elements (spec §2.10) — element types use `Array<T>`, so inline
    /// metadata blocks would only evade schema extraction and leak
    /// `"schema": null` into exports.
    array_depth: usize,
    /// Binding names contributed by includes (module-global). A local
    /// `let` clears its mark, so later includes skip it (local-wins)
    /// while earlier-only names cascade.
    binding_spliced: std::collections::HashSet<String>,
    /// Profile scopes awaiting the final resolution pass: their ident
    /// nodes plus a copy of their fields. Resolved after the document
    /// root completes so profiles see root keys (forward-safe).
    pending_profiles: Vec<ProfileScope>,
    /// Trusted overlay keys (C-052): when set, every `include` load
    /// must carry a valid sibling signature from this set (E032
    /// otherwise). `None` preserves all legacy behavior.
    trust: Option<crate::sign::TrustedKeys>,
}

impl Parser {
    /// The dotted path of the enclosing table (empty at document level).
    fn path_str(&self) -> String {
        self.current_path.join(".")
    }

    /// End line of the most recently consumed token.
    fn last_end_line(&self) -> u32 {
        self.tokens
            .get(self.pos.saturating_sub(1))
            .map(|t| t.end_line)
            .unwrap_or(1)
    }

    /// Record a statement for comment attachment.
    fn record_stmt(&mut self, key: &str, start_line: u32, end_line: u32, level: usize) {
        self.statements.push(StmtInfo {
            key: key.to_string(),
            path: self.path_str(),
            start_line,
            end_line,
            level,
            file: self.current_file_name(),
        });
    }
}

/// A statement's source extent, for comment attachment.
#[derive(Debug, Clone)]
struct StmtInfo {
    key: String,
    path: String,
    start_line: u32,
    end_line: u32,
    level: usize,
    /// Source file (includes parse as their own documents, C-025):
    /// line numbers repeat across files, so scans match (line, file).
    file: Option<String>,
}

/// A table/block's source extent, for comment attachment.
#[derive(Debug, Clone)]
struct BlockInfo {
    path: String,
    level: usize,
    start_line: u32,
    /// Source file, like statements: block matching is (line, file).
    file: Option<String>,
}

/// An `include` statement recorded while parsing a body, resolved and
/// merged when the enclosing scope closes (spec §2.7).
#[derive(Debug, Clone)]
struct PendingInclude {
    path: String,
    alias: Option<String>,
    private: bool,
    tok: Token,
    /// Length of `module.patches` when the statement was encountered:
    /// included patches splice in at this position so application order
    /// matches textual source order across files (spec §2.9).
    patch_mark: usize,
}

/// What terminates a schema entry list (spec §2.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaEnd {
    /// `}` closes an inline or brace-form schema table.
    Brace,
    /// A Dedent (or EOF) closes an indentation-form schema body.
    Indent,
}

/// A profile scope stashed for the final resolution pass: ident nodes
/// (with owning keys) plus a copy of the profile's fields.
type ProfileScope = (Vec<(String, NodeId)>, indexmap::IndexMap<String, NodeId>);

/// What terminates a statement body (spec/dual-syntax.md §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyEnd {
    /// Document root: Dedents are the EOF flush and carry no block meaning.
    Document,
    /// An indented block at indentation level `n`: ends at a Dedent whose
    /// target level is below `n`, which it consumes.
    Block(usize),
    /// A brace block (`key { … }`): ends at and consumes its `}`.
    Brace,
    /// The body of a `[header]` written at column `n`: ends (without
    /// consuming) when the next statement belongs to the enclosing scope.
    HeaderBody(usize),
}

/// The indentation level an Indent/Dedent token refers to (its text).
fn indent_level(tok: &Token) -> usize {
    tok.text.parse().unwrap_or(0)
}

impl Parser {
    // ------------------------------------------------------------------
    // Low-level token helpers
    // ------------------------------------------------------------------

    fn peek(&self) -> &Token {
        // The lexer always appends a trailing Eof token and `bump` never
        // advances past it, so the fallback borrows from the token stream
        // itself instead of a function-local temporary (fixes E0515).
        self.tokens.get(self.pos).unwrap_or_else(|| {
            self.tokens
                .last()
                .expect("token stream is never empty; the lexer appends Eof")
        })
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        // Compare by discriminant so Unit(String) still matches Unit(_)
        if std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(&kind) {
            Some(self.bump())
        } else {
            self.error(
                "E001",
                format!("expected {:?}, found {:?}", kind, self.peek_kind()),
            );
            None
        }
    }

    fn error(&mut self, code: &str, message: impl Into<String>) {
        let t = self.peek();
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            span: Some(Span {
                line: t.line,
                column: t.col,
                end_line: t.end_line,
                end_column: t.end_col,
            }),
            repair: None,
            confidence: 1.0,
            file: self.current_file_name(),
        });
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.bump();
        }
    }

    /// Token kind `n` positions ahead (0 = current). Used for the two-token
    /// lookahead that distinguishes `key =`, `key:`, and `key {`.
    fn peek_kind_at(&self, n: usize) -> TokenKind {
        self.peek_at(n)
            .map(|t| t.kind.clone())
            .unwrap_or(TokenKind::Eof)
    }

    fn peek_at(&self, n: usize) -> Option<Token> {
        self.tokens.get(self.pos + n).cloned()
    }

    /// Parse statements into `fields` until the body terminator
    /// (spec/dual-syntax.md §2, §5). Structural nesting is bounded by
    /// MAX_BLOCK_DEPTH so pathological input cannot exhaust the stack.
    /// `pending` collects `include` statements for this scope; the caller
    /// splices them when the scope closes (spec §2.7).
    fn parse_body(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        end: BodyEnd,
        pending: &mut Vec<PendingInclude>,
    ) {
        self.block_depth += 1;
        if self.block_depth > MAX_BLOCK_DEPTH {
            self.error(
                "E011",
                format!("maximum block nesting depth ({MAX_BLOCK_DEPTH}) exceeded"),
            );
            self.block_depth -= 1;
            self.recover_block(end);
            return;
        }
        self.parse_body_inner(fields, end, pending);
        self.block_depth -= 1;
    }

    /// Skip tokens until the terminator of an over-deep body, so recovery
    /// makes progress without recursing further. Terminator- and
    /// level-aware: a Dedent that closes a deeper child must not stop the
    /// skip early, or later statements would leak into an outer scope.
    fn recover_block(&mut self, end: BodyEnd) {
        let mut open_braces = 0usize;
        loop {
            match self.peek_kind() {
                TokenKind::Eof => return,
                TokenKind::LBrace => {
                    open_braces += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    if open_braces > 0 {
                        open_braces -= 1;
                        self.bump();
                    } else if end == BodyEnd::Brace {
                        self.bump();
                        return;
                    } else {
                        self.bump();
                    }
                }
                TokenKind::Dedent => {
                    let tok = self.peek().clone();
                    let target = indent_level(&tok);
                    match end {
                        BodyEnd::Block(level) | BodyEnd::HeaderBody(level) => {
                            self.bump();
                            if target < level {
                                return;
                            }
                        }
                        _ => {
                            self.bump();
                        }
                    }
                }
                TokenKind::LBracket | TokenKind::Let if matches!(end, BodyEnd::HeaderBody(_)) => {
                    return
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    fn parse_body_inner(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        end: BodyEnd,
        pending: &mut Vec<PendingInclude>,
    ) {
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                TokenKind::Eof => {
                    if end == BodyEnd::Brace {
                        self.error("E001", "expected `}`, found end of input");
                    }
                    return;
                }
                TokenKind::Dedent => {
                    let tok = self.peek().clone();
                    let target = indent_level(&tok);
                    match end {
                        // The EOF flush emits Dedent tokens; they carry no
                        // block meaning at document level.
                        BodyEnd::Document | BodyEnd::Brace => {
                            self.bump();
                        }
                        BodyEnd::Block(level) => {
                            self.bump();
                            if target >= level {
                                // Closes a nested block that was recovered
                                // under E010 (§6), not this block.
                                continue;
                            }
                            return;
                        }
                        BodyEnd::HeaderBody(level) => {
                            if target >= level {
                                self.bump();
                                continue;
                            }
                            return;
                        }
                    }
                }
                TokenKind::RBrace => match end {
                    BodyEnd::Brace => {
                        self.bump();
                        return;
                    }
                    // §5.3: a brace cannot close an indentation block.
                    BodyEnd::Block(_) => {
                        self.error("E001", "expected Dedent to close this block, found `}`");
                        self.bump();
                        return;
                    }
                    BodyEnd::HeaderBody(_) => return,
                    BodyEnd::Document => {
                        self.error("E002", "unexpected `}`");
                        self.bump();
                    }
                },
                // A header body ends (without consuming) when the next
                // statement belongs to the enclosing scope: a new header,
                // a `let`, or a document-level `profile`/`include`
                // statement (spec §2.7, §2.8). Only column-0 (header-level)
                // statements terminate: indented content stays nested, so
                // a nested `profile` still earns E018 and a nested
                // `include` merges into its true scope.
                TokenKind::LBracket | TokenKind::Let if matches!(end, BodyEnd::HeaderBody(_)) => {
                    return
                }
                TokenKind::Ident => {
                    let t = self.peek().clone();
                    if let BodyEnd::HeaderBody(level) = end {
                        if (t.text == "profile" || t.text == "include")
                            && t.col.saturating_sub(1) as usize <= level
                            && !matches!(
                                self.peek_kind_at(1),
                                TokenKind::Assign | TokenKind::Colon | TokenKind::LBrace
                            )
                        {
                            return;
                        }
                    }
                    self.parse_ident_statement(fields, pending);
                }
                TokenKind::Let => self.parse_let(),
                TokenKind::LBracket => self.parse_table_header(fields),
                TokenKind::At => {
                    // `@…` statements: patch directives (`@patch`, `@remove`,
                    // `@rename` with `(`) or bare semantic blocks. Patches
                    // live at document root only (spec §2.9). A
                    // document-level patch directive ends a header body
                    // (like profile/include/let); nested content stays in
                    // its scope.
                    if let BodyEnd::HeaderBody(level) = end {
                        let is_patch = matches!(self.peek_kind_at(1), TokenKind::Ident) && {
                            let tag = self.peek_at(1).map(|t| t.text.clone()).unwrap_or_default();
                            (tag == "patch" || tag == "remove" || tag == "rename")
                                && matches!(self.peek_kind_at(2), TokenKind::LParen)
                        };
                        let t = self.peek().clone();
                        if is_patch && t.col.saturating_sub(1) as usize <= level {
                            return;
                        }
                    }
                    let at_tok = self.peek().clone();
                    if matches!(self.peek_kind_at(1), TokenKind::Ident) {
                        let tag = self.peek_at(1).map(|t| t.text.clone()).unwrap_or_default();
                        if tag == "patch" || tag == "remove" || tag == "rename" {
                            // Reserved directives (spec §2.9): only `(` can
                            // follow. Keys cannot contain `@`, so no
                            // ordinary-key fallback exists — E019.
                            if !matches!(self.peek_kind_at(2), TokenKind::LParen) {
                                let tag_tok = self.peek_at(1).unwrap_or_else(|| at_tok.clone());
                                self.error_at(
                                    "E019",
                                    format!("expected `(` after `@{tag}`"),
                                    &tag_tok,
                                );
                                self.bump();
                                self.bump();
                                continue;
                            }
                            self.parse_patch_statement(&at_tok);
                            continue;
                        }
                    }
                    self.bump();
                    let id = self.parse_semantic_block(&at_tok);
                    if let Some(Node::Semantic { tag, .. }) = self.module.get(id) {
                        let key = tag.as_str().to_string();
                        self.record_stmt(
                            &key,
                            at_tok.line,
                            self.last_end_line(),
                            at_tok.col.saturating_sub(1) as usize,
                        );
                        fields.insert(key, id);
                    }
                }
                TokenKind::Indent => {
                    // E010 unexpected indentation (§6): consume the Indent,
                    // parse the line in the enclosing block, and let the
                    // level-aware Dedent rule above discard its matching
                    // Dedent (the enclosing block's level comparison).
                    self.error("E010", "unexpected indentation");
                    self.bump();
                }
                TokenKind::String(_)
                | TokenKind::SingleString
                | TokenKind::MlString(_)
                | TokenKind::MlSingleString => {
                    // Quoted keys are not part of the grammar (identifiers
                    // only): a string literal cannot start a statement.
                    // Skipping it would silently drop data (research
                    // HIST-YAML-020), so this is a loud E002 instead.
                    let tok = self.peek().clone();
                    self.error_at(
                        "E002",
                        "unexpected string literal at statement position (keys must be identifiers)",
                        &tok,
                    );
                    self.bump();
                }
                _ => {
                    // Alpha resilience: skip an unexpected token.
                    // NOTE (research P1-ST-002): stray value tokens after
                    // a complete statement are silently absorbed here by
                    // explicit design (spec/grammar.md §4.1 quirks:
                    // `x = 5 5` evaluates to `5`). Only string literals
                    // above are loud (no quirk protects them).
                    //
                    // Exception: a VALUE token starting a statement (at
                    // the body's statement column) can never be a quirk
                    // stray — nothing routes value-first tokens — so a
                    // bare `true = 1` or `2 = 1` line would vanish with
                    // zero diagnostics. Loud E002 instead (research
                    // P1-TI-011); same-line strays keep the quirk.
                    let tok = self.peek().clone();
                    let stmt_col = match end {
                        BodyEnd::Block(l) | BodyEnd::HeaderBody(l) => (l + 1) as u32,
                        _ => 1,
                    };
                    match &tok.kind {
                        TokenKind::Integer
                        | TokenKind::Float
                        | TokenKind::True
                        | TokenKind::False
                        | TokenKind::Null
                        | TokenKind::Env
                        | TokenKind::Secret
                        | TokenKind::Minus
                        | TokenKind::Bang
                        | TokenKind::LParen
                            if tok.col == stmt_col =>
                        {
                            self.error_at(
                                "E002",
                                "unexpected value at statement position (values need a key: `name = …`)",
                                &tok,
                            );
                        }
                        _ => {}
                    }
                    self.bump();
                }
            }
        }
    }

    /// Dispatch a statement that begins with an identifier: `key = value`,
    /// `key: value`, `key:` block header, `key { … }` table block, an
    /// `include` statement, or a `profile` block (`include`/`profile` are
    /// contextual: followed by `=`, `:`, or `{` they stay ordinary keys).
    fn parse_ident_statement(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        pending: &mut Vec<PendingInclude>,
    ) {
        let start = self.peek().clone();
        let level = start.col.saturating_sub(1) as usize;
        if start.text == "include"
            && !matches!(
                self.peek_kind_at(1),
                TokenKind::Assign | TokenKind::Colon | TokenKind::LBrace
            )
        {
            self.parse_include(pending);
            return;
        }
        if start.text == "profile"
            && !matches!(
                self.peek_kind_at(1),
                TokenKind::Assign | TokenKind::Colon | TokenKind::LBrace
            )
        {
            self.parse_profile();
            return;
        }
        match self.peek_kind_at(1) {
            TokenKind::Assign => {
                if let Some((k, id)) = self.parse_key_value_sep(TokenKind::Assign) {
                    let end = self.last_end_line();
                    self.record_stmt(&k, start.line, end, level);
                    self.extract_annotations(&k, id, start.line);
                    if self.check_depends_on_statement(&k, id) {
                        self.note_duplicate_key(&k, Some(span_of(&start)), fields.contains_key(&k));
                        fields.insert(k, id);
                    }
                }
            }
            TokenKind::Colon => {
                if matches!(self.peek_kind_at(2), TokenKind::Newline | TokenKind::Eof) {
                    self.parse_block_header(fields);
                } else if let Some((k, id)) = self.parse_key_value_sep(TokenKind::Colon) {
                    let end = self.last_end_line();
                    self.record_stmt(&k, start.line, end, level);
                    self.extract_annotations(&k, id, start.line);
                    if self.check_depends_on_statement(&k, id) {
                        self.note_duplicate_key(&k, Some(span_of(&start)), fields.contains_key(&k));
                        fields.insert(k, id);
                    }
                }
            }
            TokenKind::LBrace => self.parse_table_block(fields),
            _ => {
                // Not a statement form the grammar accepts; keep progress.
                let tok = self.peek().clone();
                self.error_at("E002", "unexpected token after identifier", &tok);
                self.bump();
            }
        }
    }

    // ------------------------------------------------------------------
    // profile "name" { … }  (spec §2.8, C-027)
    // ------------------------------------------------------------------

    /// Parse a profile block. Profiles live at document root only and
    /// merge into the root value at evaluation; they never enter the
    /// enclosing fields. References to document-root keys resolve in a
    /// final pass after the root completes (forward-safe).
    fn parse_profile(&mut self) {
        let kw = self.bump(); // consume `profile`
        if !self.current_path.is_empty() {
            self.error_at(
                "E018",
                "profile blocks may only appear at document root",
                &kw,
            );
            return;
        }
        let name_tok = self.peek().clone();
        let name = match &name_tok.kind {
            TokenKind::String(_)
            | TokenKind::SingleString
            | TokenKind::MlString(_)
            | TokenKind::MlSingleString => {
                self.bump();
                // A profile name is a static label; interpolation in it is
                // meaningless, so require a plain name.
                if name_tok.text.contains("${") {
                    self.error_at(
                        "E018",
                        "profile names must be plain string literals",
                        &name_tok,
                    );
                    return;
                }
                name_tok.text.clone()
            }
            _ => {
                self.error_at("E018", "profile blocks need a string name", &name_tok);
                return;
            }
        };
        // Normalize the name before anything else: it is a static label,
        // never interpolated, so an escaped dollar is a plain dollar here.
        // Normalizing first keeps duplicate detection sound (`"$"` vs
        // `"\$"` collide instead of silently replacing). Only marked
        // sentinels convert; a literal U+E000 keeps its identity.
        let name = Self::render_marked(&name, &Self::sentinel_marks(&name, &name_tok.kind));
        if self.module.profiles.contains_key(&name) {
            self.error_at("E018", format!("duplicate profile \"{name}\""), &name_tok);
            return;
        }
        // Brace form (`profile "n" { … }`) or indentation form
        // (`profile "n":` + indented body, spec §2.8).
        enum ProfileBody {
            Brace,
            Block(usize),
        }
        let body = match self.peek_kind() {
            TokenKind::LBrace => {
                self.bump();
                ProfileBody::Brace
            }
            TokenKind::Colon => {
                let colon = self.bump();
                self.skip_newlines();
                match self.peek_kind() {
                    TokenKind::Indent => {
                        let indent = self.bump();
                        ProfileBody::Block(indent_level(&indent))
                    }
                    _ => {
                        self.error_at("E018", "profile block has no indented body", &colon);
                        return;
                    }
                }
            }
            _ => {
                let tok = self.peek().clone();
                self.error_at("E018", "expected `{` or `:` after the profile name", &tok);
                return;
            }
        };
        self.record_stmt(
            &name,
            kw.line,
            self.last_end_line(),
            kw.col.saturating_sub(1) as usize,
        );
        let profile_stmt = self.statements.len() - 1;
        self.current_path.push(name.clone());
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        let mut pending = Vec::new();
        let saved_in_profile = self.in_profile;
        self.in_profile = true;
        match body {
            ProfileBody::Brace => self.parse_body(&mut fields, BodyEnd::Brace, &mut pending),
            ProfileBody::Block(level) => {
                self.parse_body(&mut fields, BodyEnd::Block(level), &mut pending)
            }
        }
        self.in_profile = saved_in_profile;
        self.splice_includes(&mut fields, pending, true);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        // Sibling resolution against the profile's own fields happens in
        // the final pass (together with the root fallback) so forward
        // references work uniformly.
        self.pending_profiles.push((idents, fields.clone()));
        let table_id = self.module.add_node(Node::Table { fields });
        self.statements[profile_stmt].end_line = self.last_end_line();
        self.current_path.pop();
        self.module.profiles.insert(name, table_id);
    }

    // ------------------------------------------------------------------
    // include "path" [as alias] [private]  (spec §2.7)
    // ------------------------------------------------------------------

    /// Maximum include nesting depth (files within files).
    const MAX_INCLUDE_DEPTH: usize = 32;

    /// Parse an `include` statement. The path must be a string literal;
    /// anything else is E015. The statement is recorded for resolution
    /// when the enclosing scope closes (includes are file-level defaults
    /// merged after all local keys are known).
    fn parse_include(&mut self, pending: &mut Vec<PendingInclude>) {
        let kw = self.bump(); // consume `include`
        let path_tok = self.peek().clone();
        let path = match &path_tok.kind {
            TokenKind::String(_)
            | TokenKind::SingleString
            | TokenKind::MlString(_)
            | TokenKind::MlSingleString => {
                self.bump();
                path_tok.text.clone()
            }
            _ => {
                self.error_at("E015", "include paths must be string literals", &path_tok);
                self.bump(); // guarantee progress
                return;
            }
        };
        if path.contains("${")
            || Self::has_marked(&path, &Self::sentinel_marks(&path, &path_tok.kind))
        {
            self.error_at(
                "E015",
                "interpolation in include paths is a hard error; paths must be static string literals",
                &path_tok,
            );
            return;
        }
        // Optional `as alias` and `private`, in either order.
        let mut alias: Option<String> = None;
        let mut private = false;
        loop {
            let t = self.peek().clone();
            match &t.kind {
                TokenKind::Ident if t.text == "as" && alias.is_none() => {
                    self.bump();
                    match self.expect(TokenKind::Ident) {
                        Some(name) => alias = Some(name.text),
                        None => return,
                    }
                }
                TokenKind::Ident if t.text == "private" && !private => {
                    self.bump();
                    private = true;
                }
                _ => break,
            }
        }
        let key = alias.clone().unwrap_or_else(|| path.clone());
        self.record_stmt(
            &key,
            kw.line,
            self.last_end_line(),
            kw.col.saturating_sub(1) as usize,
        );
        self.module.includes.push(IncludeStmt {
            path: path.clone(),
            alias: alias.clone(),
            private,
            scope_path: self.current_path.clone(),
            line: kw.line,
        });
        pending.push(PendingInclude {
            path,
            alias,
            private,
            tok: path_tok,
            patch_mark: self.module.patches.len(),
        });
    }

    /// Load and sub-parse one include, returning its root table node.
    /// Returns None after recording E015/E016/E017. The sub-parse shares
    /// the arena, bindings, graph, and diagnostics; scope stacks, file
    /// identity, style counters, and the module root are saved and
    /// restored so the included file parses as its own document.
    fn load_include(&mut self, inc: &PendingInclude) -> Option<NodeId> {
        use std::path::{Component, PathBuf};
        let fail = |me: &mut Parser, code: &str, msg: String| {
            me.error_at(code, msg, &inc.tok);
            None
        };
        // Policy: literal relative paths stay inside the tree by default.
        let raw = std::path::Path::new(&inc.path);
        if !self.options.allow_parent_escapes {
            if raw.is_absolute() {
                return fail(
                    self,
                    "E015",
                    format!(
                        "include paths must be relative (got `{}`); pass --allow-parent-escapes to override",
                        inc.path
                    ),
                );
            }
            if raw.components().any(|c| matches!(c, Component::ParentDir)) {
                return fail(
                    self,
                    "E015",
                    format!(
                        "include paths must not escape the including directory via `..` (got `{}`); pass --allow-parent-escapes to override",
                        inc.path
                    ),
                );
            }
        }
        let base_dir: PathBuf = self
            .current_file
            .as_ref()
            .and_then(|f| f.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let joined = base_dir.join(raw);
        // Canonicalization splits by source (F-016): the filesystem path
        // needs existence, so a host loader (virtual FS: browser blobs,
        // archives, test fixtures) takes the lexical branch, which never
        // touches the disk. `..`/absolute were already gated above (or
        // explicitly allowed); symlinks do not resolve on the lexical
        // branch, a documented delta the parent-escape check below covers
        // by comparing normalized stems on both sides.
        fn lexical_join(base: &std::path::Path, raw: &std::path::Path) -> PathBuf {
            let mut out = base.to_path_buf();
            for c in raw.components() {
                match c {
                    Component::CurDir => {}
                    Component::ParentDir => {
                        out.pop();
                    }
                    Component::Normal(s) => out.push(s),
                    Component::RootDir | Component::Prefix(_) => {
                        out = PathBuf::from(c.as_os_str());
                    }
                }
            }
            out
        }
        let has_loader = self.options.include_loader.is_some();
        let canonical = if has_loader {
            lexical_join(&base_dir, raw)
        } else {
            match joined.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    return fail(
                        self,
                        "E017",
                        format!("cannot read include file `{}`: {e}", joined.display()),
                    );
                }
            }
        };
        if !self.options.allow_parent_escapes {
            let contained = if has_loader {
                canonical.starts_with(lexical_join(&base_dir, std::path::Path::new("")))
            } else if let Ok(base_canon) = base_dir.canonicalize() {
                canonical.starts_with(&base_canon)
            } else {
                true
            };
            if !contained {
                return fail(
                        self,
                        "E015",
                        format!(
                            "include target `{}` escapes the including directory; pass --allow-parent-escapes to override",
                            canonical.display()
                        ),
                    );
            }
        }
        if self.include_stack.contains(&canonical) {
            return fail(
                self,
                "E016",
                format!(
                    "circular include detected: `{}` is already being parsed ({})",
                    canonical.display(),
                    self.include_stack
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                ),
            );
        }
        if self.include_stack.len() >= Self::MAX_INCLUDE_DEPTH {
            return fail(
                self,
                "E016",
                format!(
                    "maximum include depth ({}) exceeded",
                    Self::MAX_INCLUDE_DEPTH
                ),
            );
        }
        // Signed overlays (C-052): with a trusted set, the include
        // must carry a valid sibling signature — fail closed before
        // a single byte is parsed. The content below is read once
        // and verified in memory (no check/use gap); only the
        // sidecar itself is read separately, which cannot smuggle
        // unverified content past the check. (Cloned: the failure
        // path needs `self` mutably.)
        let trust = self.trust.clone();
        // Host loader hook (F-016): replaces ONLY the filesystem read.
        // Canonicalization, depth, and trust verification around it are
        // unchanged; a hook failure surfaces exactly like a read failure.
        let src = match self.options.include_loader.clone() {
            Some(loader) => match loader(&canonical.to_string_lossy()) {
                Ok(s) => s,
                Err(e) => {
                    return fail(
                        self,
                        "E017",
                        format!("include loader refused `{}`: {e}", canonical.display()),
                    );
                }
            },
            None => match std::fs::read_to_string(&canonical) {
                Ok(s) => s,
                Err(e) => {
                    return fail(
                        self,
                        "E017",
                        format!("cannot read include file `{}`: {e}", canonical.display()),
                    );
                }
            },
        };
        if let Some(keys) = &trust {
            let label = canonical.to_string_lossy().into_owned();
            match crate::sign::read_sidecar(&canonical, &label) {
                Err(reason) => return fail(self, "E032", reason),
                Ok(sidecar) => {
                    if let Err(reason) =
                        crate::sign::verify_bytes(&label, src.as_bytes(), &sidecar, keys)
                    {
                        return fail(self, "E032", reason);
                    }
                }
            }
        }
        // Save per-file session state; the arena, bindings, graph,
        // diagnostics, and options are intentionally shared.
        let saved_file = self.current_file.clone();
        let saved_root = self.module.root;
        let saved_scopes = std::mem::take(&mut self.ident_scopes);
        let saved_path = self.current_path.clone();
        let saved_brace = self.brace_tables;
        let saved_indent = self.indent_tables;
        let saved_in_include = self.in_include;
        let saved_in_profile = self.in_profile;
        self.current_file = Some(canonical.clone());
        self.include_stack.push(canonical);
        self.in_include = true;
        // An included file is a fresh document: profiles allowed at its
        // root, with resolution deferred to the outer document end.
        self.in_profile = false;
        // Included files lex fresh (their own Indent/Dedent stacks).
        let mut lexer = Lexer::new(&src);
        let lexed = lexer.tokenize();
        let lex_file = self.current_file_name();
        // Included comments join the shared stream with their file
        // stamped, so annotation extraction sees included bindings'
        // contracts (C-033) without cross-file line collisions.
        let n_lex_comments = lexed.comments.len();
        self.comments.extend(lexed.comments);
        self.comment_files
            .extend(std::iter::repeat_with(|| lex_file.clone()).take(n_lex_comments));
        // Aliased includes nest under the alias: scope the sub-parse
        // path so annotation paths match the merged tree (C-033).
        if let Some(alias) = inc.alias.clone() {
            self.current_path.push(alias);
        }
        let mut sub_errors = lexed.errors;
        let mut sub_repairs = lexed.repairs;
        for d in sub_errors.iter_mut().chain(sub_repairs.iter_mut()) {
            if d.file.is_none() {
                d.file = lex_file.clone();
            }
        }
        self.errors.extend(sub_errors);
        self.module.healing.extend(sub_repairs);
        // The outer token stream is NOT exhausted (table bodies end
        // mid-stream), so save and restore it around the sub-parse.
        let saved_tokens = std::mem::take(&mut self.tokens);
        let saved_pos = self.pos;
        self.tokens = lexed.tokens;
        self.pos = 0;
        self.parse_document();
        self.tokens = saved_tokens;
        self.pos = saved_pos;
        // Restore the outer session; keep the shared arena, bindings,
        // graph, diagnostics, includes list, and trivia.
        let sub_root = self.module.root;
        self.module.root = saved_root;
        self.current_file = saved_file;
        self.include_stack.pop();
        self.current_path = saved_path;
        self.brace_tables = saved_brace;
        self.indent_tables = saved_indent;
        self.in_include = saved_in_include;
        self.in_profile = saved_in_profile;
        self.ident_scopes = saved_scopes;
        sub_root
    }

    /// Resolve and merge a scope's recorded includes (spec §2.7).
    /// Included files provide defaults: explicit local keys always win
    /// regardless of position; among several included files the LAST one
    /// wins (cascade). Tables merge recursively with local-wins at every
    /// level (see merge_tables_at). An included file's `let` bindings
    /// merge into the shared bindings without overwriting local ones;
    /// private includes contribute only bindings, never fields.
    /// `profile_scope` is true when the target scope is a profile body:
    /// schemas arriving through those includes are E020 (spec §2.10).
    fn splice_includes(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        mut pending: Vec<PendingInclude>,
        profile_scope: bool,
    ) {
        if pending.is_empty() {
            return;
        }
        if !self.options.enable_includes {
            for inc in &pending {
                self.error_at(
                    "E015",
                    "include statements require file-based parsing (parse_file); string input stays hermetic",
                    &inc.tok,
                );
            }
            return;
        }
        if !self.options.resolve_includes {
            // Formatting mode: keep the statements, skip the merge.
            return;
        }
        // Dotted paths of keys contributed by includes in this scope
        // (spec §2.7). A marked path cascades (later includes win); an
        // unmarked path is local content that never loses. Subtree paths
        // are marked on insert so nested levels decide correctly.
        let mut spliced: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut pi = 0usize;
        while pi < pending.len() {
            let inc = pending[pi].clone();
            let patches_before = self.module.patches.len();
            let loaded = self.load_include(&inc);
            // Included patches belong textually at the include statement:
            // move everything the load appended back to the recorded mark
            // so cross-file application order matches source order.
            // Layout: [head..mark] ++ [mark..old] ++ [new...] becomes
            // [head..mark] ++ [new...] ++ [mark..old]. Marks recorded
            // after this insertion point shift by the inserted count so
            // later includes keep stable source-order positions.
            let mark = inc.patch_mark.min(patches_before);
            let old_tail_len = patches_before - mark;
            let mut inserted = 0usize;
            if self.module.patches.len() > patches_before {
                let mut tail: Vec<NodeId> = self.module.patches.drain(mark..).collect();
                let new_part: Vec<NodeId> = tail.split_off(old_tail_len.min(tail.len()));
                inserted = new_part.len();
                self.module.patches.extend(new_part);
                self.module.patches.extend(tail);
            }
            for later in pending.iter_mut().skip(pi + 1) {
                if later.patch_mark >= mark {
                    later.patch_mark += inserted;
                }
            }
            pi += 1;
            let Some(sub_root) = loaded else {
                continue;
            };
            let sub_fields = match self.module.get(sub_root) {
                Some(Node::Table { fields }) => fields.clone(),
                _ => continue,
            };
            if inc.private {
                match inc.alias {
                    Some(name) => {
                        // The alias itself becomes a referenceable binding;
                        // a local `let` of the same name always wins, and
                        // parse_let clears the mark so later includes skip.
                        if !self.module.bindings.contains_key(&name)
                            || self.binding_spliced.contains(&name)
                        {
                            self.module.bindings.insert(name.clone(), sub_root);
                            self.binding_spliced.insert(name);
                        }
                    }
                    None => {
                        for (k, v) in sub_fields {
                            if !self.module.bindings.contains_key(&k)
                                || self.binding_spliced.contains(&k)
                            {
                                self.module.bindings.insert(k.clone(), v);
                                self.binding_spliced.insert(k);
                            }
                        }
                    }
                }
                continue;
            }
            match inc.alias {
                Some(name) => {
                    // Aliased public includes nest under the alias with
                    // the same defaults rule as bare keys.
                    self.merge_key(fields, name, sub_root, &mut spliced);
                }
                None => {
                    for (k, v) in sub_fields {
                        self.merge_key(fields, k, v, &mut spliced);
                    }
                }
            }
        }
        if profile_scope {
            self.reject_profile_schemas(fields);
        }
    }

    /// Reject schemas that entered a profile through an include (spec
    /// §2.10, E020). Direct schema blocks are already banned inside
    /// profiles (`in_profile`), but included files parse fresh with
    /// `in_profile = false`, so without this check an included schema
    /// would survive as a SchemaTable value, evaluate to Null, and leak
    /// `"schema": null` into profile exports. Every offending schema is
    /// reported at its own span (with its file) and detached so profile
    /// values stay clean; the rest of the included data merges normally.
    /// Ancestor data tables are preserved: only the schema keys leave.
    fn reject_profile_schemas(&mut self, fields: &mut IndexMap<String, NodeId>) {
        let keys: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (key, id) in keys {
            if matches!(self.module.get(id), Some(Node::SchemaTable { .. })) {
                self.report_profile_schema(id);
                fields.shift_remove(&key);
                continue;
            }
            let mut hits = Vec::new();
            self.strip_schema_tables(id, &mut hits);
            for hit in hits {
                self.report_profile_schema(hit);
            }
        }
    }

    /// Report one include-smuggled profile schema at its own span.
    fn report_profile_schema(&mut self, schema_id: NodeId) {
        let span = self.module.node_spans.get(&schema_id).cloned();
        let file = self.module.node_files.get(&schema_id).cloned();
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: "E020".into(),
            message: "schemas may not appear inside profiles, including through includes; move the schema to the document root".into(),
            path: None,
            span,
            repair: None,
            confidence: 1.0,
            file,
        });
    }

    /// Remove SchemaTable nodes under the value table `id`, recursing
    /// into nested tables (an aliased include can nest a schema under
    /// data). Direct schema children leave by key; deeper hits are
    /// removed in place so ancestor data tables survive. Collects every
    /// removed schema node for diagnostics.
    fn strip_schema_tables(&mut self, id: NodeId, out: &mut Vec<NodeId>) {
        let children: Vec<(String, NodeId)> = match self.module.get(id) {
            Some(Node::Table { fields }) => fields.clone().into_iter().collect(),
            // A schema node here means strip was called on one directly;
            // the caller drops its key, so just record it.
            Some(Node::SchemaTable { .. }) => {
                out.push(id);
                return;
            }
            _ => return,
        };
        for (k, v) in &children {
            if matches!(self.module.get(*v), Some(Node::SchemaTable { .. })) {
                if let Some(Node::Table { fields }) = self.module.get_mut(id) {
                    fields.shift_remove(k);
                }
                out.push(*v);
            } else {
                self.strip_schema_tables(*v, out);
            }
        }
    }

    /// Merge one included value under `key` (spec §2.7 defaults rule):
    /// fresh keys are taken (whole subtree marked); conflicts on marked
    /// paths cascade (later file wins, tables recurse); conflicts on
    /// unmarked paths fill gaps only (tables recurse with local-wins,
    /// scalars untouched).
    fn merge_key(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        key: String,
        inc_id: NodeId,
        spliced: &mut std::collections::HashSet<String>,
    ) {
        match fields.get(&key) {
            None => {
                fields.insert(key.clone(), inc_id);
                self.mark_subtree(&key, inc_id, spliced);
            }
            Some(_) if spliced.contains(&key) => {
                self.merge_tables_at(fields, &key, inc_id, spliced, true);
            }
            Some(_) => {
                self.merge_tables_at(fields, &key, inc_id, spliced, false);
            }
        }
    }

    /// Merge table `inc_id` into the table under `key` in place when both
    /// sides are tables; on a scalar conflict overwrite only in cascade
    /// mode. Anything else is a no-op (the defaults rule already
    /// decided). Merging in place keeps edges and references valid;
    /// merged-in fields gain dependency edges to their new parent.
    fn merge_tables_at(
        &mut self,
        fields: &mut indexmap::IndexMap<String, NodeId>,
        key: &str,
        inc_id: NodeId,
        spliced: &mut std::collections::HashSet<String>,
        overwrite: bool,
    ) {
        // SchemaTable × SchemaTable unions field-wise like value tables
        // (spec §2.10): an included schema merges with a local one
        // instead of clobbering it. Schema-vs-data conflicts fall
        // through to the scalar rules below.
        if matches!(self.module.get(inc_id), Some(Node::SchemaTable { .. })) {
            if let Some(&local_id) = fields.get(key) {
                if local_id != inc_id
                    && matches!(self.module.get(local_id), Some(Node::SchemaTable { .. }))
                {
                    if self.merge_schemas_inner(local_id, inc_id, key, spliced, overwrite) {
                        spliced.insert(key.to_string());
                    }
                    return;
                }
            }
        }
        let Some(Node::Table { fields: inc_fields }) = self.module.get(inc_id) else {
            // Scalar-vs-table conflict: replace only when cascading over a
            // fully include-provided table. A table with any local
            // (unmarked) content keeps local-wins and ignores the scalar.
            if overwrite {
                if let Some(&local_id) = fields.get(key) {
                    if !self.has_unmarked(local_id, key, spliced) {
                        fields.insert(key.to_string(), inc_id);
                        self.mark_subtree(key, inc_id, spliced);
                    }
                }
            }
            return;
        };
        let inc_fields = inc_fields.clone();
        let Some(&local_id) = fields.get(key) else {
            return;
        };
        if local_id == inc_id {
            return;
        }
        let Some(Node::Table { .. }) = self.module.get(local_id) else {
            // Table-vs-scalar conflict: a marked (previously included)
            // scalar cascades, a local scalar always wins.
            if overwrite && spliced.contains(key) {
                fields.insert(key.to_string(), inc_id);
                self.mark_subtree(key, inc_id, spliced);
            }
            return;
        };
        let mut merged = match self.module.get(local_id) {
            Some(Node::Table { fields: lf }) => lf.clone(),
            _ => return,
        };
        let mut changed = false;
        for (k, v) in &inc_fields {
            let child = format!("{key}.{k}");
            match merged.get(k) {
                None => {
                    merged.insert(k.clone(), *v);
                    self.module.add_edge(*v, local_id);
                    self.mark_subtree(&child, *v, spliced);
                    changed = true;
                }
                Some(&sub_local) => {
                    if sub_local == *v {
                        continue;
                    }
                    let local_kind = self.module.get(sub_local).cloned();
                    let inc_kind = self.module.get(*v).cloned();
                    // Nested schemas union like their parents (spec §2.10).
                    if matches!(
                        (&local_kind, &inc_kind),
                        (
                            Some(Node::SchemaTable { .. }),
                            Some(Node::SchemaTable { .. })
                        )
                    ) {
                        let sub_overwrite = overwrite && spliced.contains(&child);
                        if self.merge_schemas_inner(sub_local, *v, &child, spliced, sub_overwrite) {
                            changed = true;
                        }
                        continue;
                    }
                    let both_tables = matches!(
                        (&local_kind, &inc_kind),
                        (Some(Node::Table { .. }), Some(Node::Table { .. }))
                    );
                    if !both_tables {
                        if overwrite && spliced.contains(&child) {
                            merged.insert(k.clone(), *v);
                            self.module.add_edge(*v, local_id);
                            changed = true;
                        }
                        continue;
                    }
                    // Recurse: cascade only through included paths, so a
                    // purely local nested table keeps local-wins even
                    // inside a composition.
                    let sub_overwrite = overwrite && spliced.contains(&child);
                    if self.merge_tables_inner(sub_local, *v, &child, spliced, sub_overwrite) {
                        changed = true;
                    }
                }
            }
        }
        if changed {
            if let Some(Node::Table { fields: slot }) = self.module.get_mut(local_id) {
                *slot = merged;
            }
            spliced.insert(key.to_string());
        }
    }

    /// Merge table `inc_id`'s fields into table `local_id` in place.
    /// Shared worker for nested recursion. Returns true when the tree
    /// was actually mutated, so callers mark provenance precisely.
    fn merge_tables_inner(
        &mut self,
        local_id: NodeId,
        inc_id: NodeId,
        path: &str,
        spliced: &mut std::collections::HashSet<String>,
        overwrite: bool,
    ) -> bool {
        let Some(Node::Table { fields: inc_fields }) = self.module.get(inc_id) else {
            return false;
        };
        let inc_fields = inc_fields.clone();
        let mut merged = match self.module.get(local_id) {
            Some(Node::Table { fields: lf }) => lf.clone(),
            _ => return false,
        };
        let mut changed = false;
        for (k, v) in &inc_fields {
            let child = format!("{path}.{k}");
            match merged.get(k) {
                None => {
                    merged.insert(k.clone(), *v);
                    self.module.add_edge(*v, local_id);
                    self.mark_subtree(&child, *v, spliced);
                    changed = true;
                }
                Some(&sub_local) => {
                    if sub_local == *v {
                        continue;
                    }
                    let local_kind = self.module.get(sub_local).cloned();
                    let inc_kind = self.module.get(*v).cloned();
                    // Nested schemas union like their parents (spec §2.10).
                    if matches!(
                        (&local_kind, &inc_kind),
                        (
                            Some(Node::SchemaTable { .. }),
                            Some(Node::SchemaTable { .. })
                        )
                    ) {
                        let sub_overwrite = overwrite && spliced.contains(&child);
                        if self.merge_schemas_inner(sub_local, *v, &child, spliced, sub_overwrite) {
                            changed = true;
                        }
                        continue;
                    }
                    let both_tables = matches!(
                        (&local_kind, &inc_kind),
                        (Some(Node::Table { .. }), Some(Node::Table { .. }))
                    );
                    if !both_tables {
                        if overwrite && spliced.contains(&child) {
                            merged.insert(k.clone(), *v);
                            self.module.add_edge(*v, local_id);
                            changed = true;
                        }
                        continue;
                    }
                    let sub_overwrite = overwrite && spliced.contains(&child);
                    if self.merge_tables_inner(sub_local, *v, &child, spliced, sub_overwrite) {
                        changed = true;
                    }
                }
            }
        }
        if changed {
            if let Some(Node::Table { fields: slot }) = self.module.get_mut(local_id) {
                *slot = merged;
            }
        }
        changed
    }

    /// Union an included schema table into a local one in place (spec
    /// §2.10): missing fields are taken, conflicting leaf fields follow
    /// local-wins (or cascade when `overwrite` over an included path),
    /// nested schema tables recurse, and `closed` follows the winner.
    /// Merging in place keeps edges valid; returns true on mutation.
    fn merge_schemas_inner(
        &mut self,
        local_id: NodeId,
        inc_id: NodeId,
        path: &str,
        spliced: &mut std::collections::HashSet<String>,
        overwrite: bool,
    ) -> bool {
        let (local_fields, local_closed) = match self.module.get(local_id) {
            Some(Node::SchemaTable { fields, closed }) => (fields.clone(), *closed),
            _ => return false,
        };
        let (inc_fields, inc_closed) = match self.module.get(inc_id) {
            Some(Node::SchemaTable { fields, closed }) => (fields.clone(), *closed),
            _ => return false,
        };
        let mut merged = local_fields;
        let mut merged_closed = local_closed;
        let mut changed = false;
        if overwrite && merged_closed != inc_closed {
            merged_closed = inc_closed;
            changed = true;
        }
        for (k, v) in &inc_fields {
            let child = format!("{path}.{k}");
            match merged.get(k) {
                None => {
                    merged.insert(k.clone(), *v);
                    self.module.add_edge(*v, local_id);
                    self.mark_subtree(&child, *v, spliced);
                    changed = true;
                }
                Some(&sub_local) => {
                    if sub_local == *v {
                        continue;
                    }
                    let both_schema = matches!(
                        (self.module.get(sub_local), self.module.get(*v)),
                        (
                            Some(Node::SchemaTable { .. }),
                            Some(Node::SchemaTable { .. })
                        )
                    );
                    if both_schema {
                        let sub_overwrite = overwrite && spliced.contains(&child);
                        if self.merge_schemas_inner(sub_local, *v, &child, spliced, sub_overwrite) {
                            changed = true;
                        }
                        continue;
                    }
                    if overwrite && spliced.contains(&child) {
                        merged.insert(k.clone(), *v);
                        self.module.add_edge(*v, local_id);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            if let Some(Node::SchemaTable {
                fields: slot,
                closed,
            }) = self.module.get_mut(local_id)
            {
                *slot = merged;
                *closed = merged_closed;
            }
        }
        changed
    }

    /// True when a table subtree contains any local (unmarked) content.
    /// Guards whole-table replacement: a later include's scalar never
    /// discards a table that still holds explicit local keys.
    fn has_unmarked(
        &self,
        id: NodeId,
        path: &str,
        spliced: &std::collections::HashSet<String>,
    ) -> bool {
        if !spliced.contains(path) {
            return true;
        }
        match self.module.get(id) {
            Some(Node::Table { fields }) => {
                let fields = fields.clone();
                fields.iter().any(|(k, v)| {
                    let child = format!("{path}.{k}");
                    self.has_unmarked(*v, &child, spliced)
                })
            }
            _ => false,
        }
    }

    /// Mark a key path and every table path below it as include-provided,
    /// so nested cascade decisions see the full provenance. Schema
    /// tables descend like value tables: without this, a later include
    /// could never cascade over an earlier include's schema fields.
    fn mark_subtree(
        &self,
        path: &str,
        id: NodeId,
        spliced: &mut std::collections::HashSet<String>,
    ) {
        spliced.insert(path.to_string());
        let children: Vec<(String, NodeId)> = match self.module.get(id) {
            Some(Node::Table { fields }) | Some(Node::SchemaTable { fields, .. }) => {
                fields.clone().into_iter().collect()
            }
            _ => return,
        };
        for (k, v) in &children {
            let child = format!("{path}.{k}");
            self.mark_subtree(&child, *v, spliced);
        }
    }

    /// `key:` followed by an indented block (indentation style, §2 and §5).
    fn parse_block_header(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let key_tok = self.peek().clone();
        let key = self
            .expect(TokenKind::Ident)
            .map(|t| t.text)
            .unwrap_or_default();
        let colon = self.expect(TokenKind::Colon);
        self.skip_newlines();
        let level = if matches!(self.peek_kind(), TokenKind::Indent) {
            let indent = self.bump();
            indent_level(&indent)
        } else {
            // E008: the header has no indented body (including at EOF).
            let tok = colon.unwrap_or_else(|| self.peek().clone());
            self.error_at(
                "E008",
                format!("block header `{key}:` has no indented body"),
                &tok,
            );
            return;
        };
        // A `schema:` header parses its indented body as a schema
        // (spec §2.10).
        if key == "schema" {
            if self.in_profile {
                self.error_at("E020", "schemas may not appear inside profiles", &key_tok);
                return;
            }
            self.indent_tables += 1;
            self.record_stmt(
                &key,
                key_tok.line,
                self.last_end_line(),
                key_tok.col.saturating_sub(1) as usize,
            );
            let node = self.parse_schema_indent(&key_tok, level);
            parent.insert(key, node);
            return;
        }
        self.indent_tables += 1;
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: key.clone(),
            path: self.path_str(),
            start_line: key_tok.line,
            end_line: key_tok.line,
            level: key_tok.col.saturating_sub(1) as usize,
            file: self.current_file_name(),
        });
        self.current_path.push(key.clone());
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level,
            start_line: key_tok.line,
            file: self.current_file_name(),
        });
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        let mut pending = Vec::new();
        self.parse_body(&mut fields, BodyEnd::Block(level), &mut pending);
        self.splice_includes(&mut fields, pending, false);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        let _ = self.resolve_sibling_refs(idents, &fields);
        // Tables carry their name-token span (and file stamp) like
        // every value node: tooling scopes extents off them (C-043).
        let table_id = self.add_spanned(Node::Table { fields }, &key_tok);
        self.statements[stmt_idx].end_line = self.last_end_line();
        self.current_path.pop();
        self.extract_annotations(&key, table_id, key_tok.line);
        self.note_duplicate_key(&key, Some(span_of(&key_tok)), parent.contains_key(&key));
        parent.insert(key, table_id);
    }

    /// `key { … }` brace-style table block (HCL-style, §2).
    fn parse_table_block(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let key_tok = self.peek().clone();
        let key = self
            .expect(TokenKind::Ident)
            .map(|t| t.text)
            .unwrap_or_default();
        // A `schema` table block parses as a schema (spec §2.10).
        // The `{` is still next: parse_schema_brace consumes it.
        if key == "schema" {
            if self.in_profile {
                self.error_at("E020", "schemas may not appear inside profiles", &key_tok);
                return;
            }
            self.brace_tables += 1;
            self.record_stmt(
                &key,
                key_tok.line,
                self.last_end_line(),
                key_tok.col.saturating_sub(1) as usize,
            );
            let node = self.parse_schema_brace(&key_tok);
            parent.insert(key, node);
            return;
        }
        self.expect(TokenKind::LBrace);
        self.brace_tables += 1;
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: key.clone(),
            path: self.path_str(),
            start_line: key_tok.line,
            end_line: key_tok.line,
            level: key_tok.col.saturating_sub(1) as usize,
            file: self.current_file_name(),
        });
        self.current_path.push(key.clone());
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level: key_tok.col.saturating_sub(1) as usize,
            start_line: key_tok.line,
            file: self.current_file_name(),
        });
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        let mut pending = Vec::new();
        self.parse_body(&mut fields, BodyEnd::Brace, &mut pending);
        self.splice_includes(&mut fields, pending, false);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        let _ = self.resolve_sibling_refs(idents, &fields);
        // Name-token span and file stamp, as above (C-043 scopes).
        let table_id = self.add_spanned(Node::Table { fields }, &key_tok);
        self.statements[stmt_idx].end_line = self.last_end_line();
        self.current_path.pop();
        self.extract_annotations(&key, table_id, key_tok.line);
        self.note_duplicate_key(&key, Some(span_of(&key_tok)), parent.contains_key(&key));
        parent.insert(key, table_id);
    }

    /// Diagnostic at an explicit token's span.
    fn error_at(&mut self, code: &str, message: impl Into<String>, tok: &Token) {
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            span: Some(span_of(tok)),
            repair: None,
            confidence: 1.0,
            file: self.current_file_name(),
        });
    }

    /// Diagnostic at an explicit span (for comment-anchored diagnostics
    /// such as malformed annotations, which have no token).
    fn error_at_span(&mut self, code: &str, message: impl Into<String>, span: &Span) {
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: message.into(),
            path: None,
            span: Some(span.clone()),
            repair: None,
            confidence: 1.0,
            file: self.current_file_name(),
        });
    }

    /// Duplicate-key notice (W011): a same-scope redefinition keeps
    /// last-wins (long-standing behavior everywhere), but silence is not
    /// allowed (research P1-TI-012/HIST-YAML-004). Warning-class, recorded
    /// on module.healing so strict parsing still succeeds and the notice
    /// surfaces through evaluation diagnostics. Call BEFORE overwriting;
    /// pass whether the key already exists. Include-merge paths never call
    /// this (cross-file composition is designed, not accidental).
    fn note_duplicate_key(&mut self, key: &str, span: Option<Span>, existed: bool) {
        if existed {
            self.module.healing.push(Diagnostic {
                severity: Severity::Warning,
                code: "W011".into(),
                message: format!("duplicate key '{key}'; last wins"),
                path: None,
                span,
                repair: None,
                confidence: 1.0,
                file: self.current_file_name(),
            });
        }
    }

    /// Extract `schema` tables from value fields into the schema tree
    /// (spec §2.10, C-030). Runs once per document after the body parses
    /// and includes splice, before the dependency graph builds. A `schema`
    /// key holding a SchemaTable node becomes schema (never data); any
    /// other `schema`-keyed value stays ordinary data. Nested schemas
    /// attach under their parent's schema node, creating open parents
    /// as needed. Runs only at the top-level document; included files
    /// contribute through the shared fields.
    fn extract_schemas(&mut self) {
        if self.in_include {
            return;
        }
        let Some(root) = self.module.root else {
            return;
        };
        if let Some(schema_id) = self.extract_table_schema(root) {
            self.module.schema = Some(schema_id);
        }
    }

    /// Extract schemas from one value table node. Returns the schema
    /// node for this table when it has a direct `schema` child or any
    /// nested schemas, creating an open parent as needed.
    fn extract_table_schema(&mut self, table_id: NodeId) -> Option<NodeId> {
        // Snapshot the fields: extraction mutates the table in place.
        let fields = match self.module.get(table_id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return None,
        };
        // Direct `schema` child?
        let mut schema_id = match fields.get("schema") {
            Some(&id) => match self.module.get(id) {
                Some(Node::SchemaTable { .. }) => {
                    // Detach from value fields so it never exports.
                    if let Some(Node::Table { fields }) = self.module.get_mut(table_id) {
                        fields.shift_remove("schema");
                    }
                    Some(id)
                }
                _ => None,
            },
            None => None,
        };
        // Recurse into value-table children for nested schemas.
        let mut nested: IndexMap<String, NodeId> = IndexMap::new();
        for (k, v) in &fields {
            if *k == "schema" {
                continue;
            }
            if matches!(self.module.get(*v), Some(Node::Table { .. })) {
                if let Some(sub) = self.extract_table_schema(*v) {
                    nested.insert(k.clone(), sub);
                }
            }
        }
        if nested.is_empty() {
            return schema_id;
        }
        // Nested schemas need a parent: reuse the direct one or create
        // an open table.
        let parent = match schema_id {
            Some(id) => id,
            None => {
                let id = self.module.add_node(Node::SchemaTable {
                    fields: IndexMap::new(),
                    closed: false,
                });
                schema_id = Some(id);
                id
            }
        };
        if let Some(Node::SchemaTable { fields, .. }) = self.module.get_mut(parent) {
            for (k, v) in nested {
                fields.insert(k, v);
            }
        }
        schema_id
    }

    /// Extract `depends_on` arrays from value fields into the metadata
    /// side table (spec §2.11, C-042). Runs once per document alongside
    /// schema extraction, before the dependency graph builds. A
    /// `depends_on` key holding an array of string literals detaches
    /// into `Module.depends_on` (never data, never exported); any
    /// other value stays ordinary data. Non-string elements are E022.
    /// Nested tables recurse; included files contribute through the
    /// shared fields.
    fn extract_depends_on(&mut self) {
        if self.in_include {
            return;
        }
        let Some(root) = self.module.root else {
            return;
        };
        self.extract_table_depends_on(root);
    }

    /// Extract `depends_on` from one value table node and recurse.
    fn extract_table_depends_on(&mut self, table_id: NodeId) {
        let fields = match self.module.get(table_id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        if let Some(&id) = fields.get("depends_on") {
            if let Some(Node::Array { elements }) = self.module.get(id).cloned() {
                let mut paths = Vec::new();
                let mut bad = false;
                for element in &elements {
                    match self.module.get(*element) {
                        Some(Node::Literal(Value::String(path))) => paths.push(path.clone()),
                        _ => {
                            bad = true;
                            if let Some(span) = self.module.node_spans.get(element).cloned() {
                                self.error_at_span(
                                    "E022",
                                    "depends_on entries must be string literals",
                                    &span,
                                );
                            }
                        }
                    }
                }
                if !bad {
                    if let Some(Node::Table { fields }) = self.module.get_mut(table_id) {
                        fields.shift_remove("depends_on");
                    }
                    let span =
                        self.module
                            .node_spans
                            .get(&id)
                            .cloned()
                            .unwrap_or(crate::diag::Span {
                                line: 1,
                                column: 1,
                                end_line: 1,
                                end_column: 1,
                            });
                    self.module
                        .depends_on
                        .insert(table_id, DependsOn { paths, span });
                }
            }
        }
        // Recurse into remaining value-table children (inline tables
        // included — inline `depends_on` arrays already errored at
        // parse, so only statement-level blocks arrive here).
        let fields = match self.module.get(table_id) {
            Some(Node::Table { fields }) => fields.clone(),
            _ => return,
        };
        for (k, v) in &fields {
            if *k == "depends_on" {
                continue;
            }
            if matches!(self.module.get(*v), Some(Node::Table { .. })) {
                self.extract_table_depends_on(*v);
            }
        }
    }

    /// Display name of the file currently being parsed, for diagnostics.
    fn current_file_name(&self) -> Option<String> {
        self.current_file
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Add a node and record the span of the token that produced it, so
    /// evaluation-time diagnostics can point at real source positions.
    fn add_spanned(&mut self, node: Node, tok: &Token) -> NodeId {
        let id = self.module.add_node(node);
        self.module.record_span(id, span_of(tok));
        if let Some(file) = self.current_file_name() {
            self.module.record_file(id, file);
        }
        id
    }

    // ------------------------------------------------------------------
    // Healing (C-016): syntactic repairs with confidence scoring
    // ------------------------------------------------------------------

    /// Record a healing diagnostic. The confidence policy decides whether
    /// the repair is applied at all:
    /// - ≥ 0.95: applied in every mode (Info)
    /// - 0.85–0.94: applied in every mode (Warning; strict fails on it at
    ///   the exit-code level)
    /// - 0.70–0.84: applied only in resilient mode
    /// - < 0.70: never applied — the caller's error path stands
    fn heal(
        &mut self,
        code: &str,
        message: String,
        tok: &Token,
        repair: Repair,
        confidence: f64,
    ) -> bool {
        let applies = confidence >= thresholds::HIGH
            || (self.options.resilient && confidence >= thresholds::MEDIUM);
        if !applies {
            return false;
        }
        self.module.healing.push(Diagnostic {
            severity: if confidence >= thresholds::AUTO_APPLY {
                Severity::Info
            } else {
                Severity::Warning
            },
            code: code.into(),
            message,
            path: None,
            span: Some(span_of(tok)),
            repair: Some(repair),
            confidence,
            file: self.current_file_name(),
        });
        true
    }

    /// Consume a closing delimiter, healing a bracket/brace mismatch when
    /// the mode allows it (H004, medium confidence: single-token
    /// substitution). Returns true when the delimiter was consumed.
    fn expect_closing_healing(&mut self, expected: TokenKind, heal_code: &str) -> bool {
        let next = self.peek().clone();
        if next.kind == expected {
            self.bump();
            return true;
        }
        let healable = matches!(
            (&expected, &next.kind),
            (TokenKind::RBracket, TokenKind::RBrace) | (TokenKind::RBrace, TokenKind::RBracket)
        );
        if healable {
            let applied = self.heal(
                heal_code,
                format!(
                    "mismatched closing delimiter; replaced {:?} with {:?}",
                    next.kind, expected
                ),
                &next,
                Repair {
                    original: format!("{:?}", next.kind),
                    suggested: format!("{:?}", expected),
                    kind: RepairKind::Other("bracket_mismatch".into()),
                    safe: false,
                },
                thresholds::MEDIUM + 0.12, // 0.82 — resilient mode only
            );
            if applied {
                self.bump();
                return true;
            }
        }
        // Not healed: report the missing delimiter and leave the token for
        // the caller's error path.
        self.error(
            "E001",
            format!("expected {:?}, found {:?}", expected, next.kind),
        );
        false
    }

    // ------------------------------------------------------------------
    // Document entry point
    // ------------------------------------------------------------------

    fn parse_document(&mut self) {
        let mut root_fields = indexmap::IndexMap::new();
        self.ident_scopes.push(Vec::new());

        let mut pending = Vec::new();
        self.parse_body(&mut root_fields, BodyEnd::Document, &mut pending);
        self.splice_includes(&mut root_fields, pending, false);

        let root_idents = self.ident_scopes.pop().unwrap_or_default();
        let _ = self.resolve_sibling_refs(root_idents, &root_fields);

        // Final profile pass (spec §2.6): each profile scope resolves
        // against its own fields, then the leftovers fall back to the
        // completed root fields, so profiles reference base keys forward
        // and backward. Anything still unresolved falls through to `let`
        // bindings in build_graph, then W001. Runs only at the top-level
        // document: included files stash theirs for the outer pass so
        // every profile sees the composed root.
        if !self.in_include {
            let profiles = std::mem::take(&mut self.pending_profiles);
            for (idents, prof_fields) in profiles {
                let leftover = self.resolve_sibling_refs(idents, &prof_fields);
                // Root fallback: a profile key may shadow a base key of
                // the same name (`port = port`), so the self-reference
                // exclusion is lifted here — the nodes differ, no cycle
                // is possible (base keys never reference profiles).
                let unowned: Vec<(String, NodeId)> = leftover
                    .into_iter()
                    .map(|(_, id)| (String::new(), id))
                    .collect();
                let _ = self.resolve_sibling_refs(unowned, &root_fields);
            }
        }

        let root_id = self.module.add_node(Node::Table {
            fields: root_fields,
        });
        self.module.root = Some(root_id);
    }

    // ------------------------------------------------------------------
    // let name = expr
    // ------------------------------------------------------------------

    fn parse_let(&mut self) {
        let let_tok = self.peek().clone();
        self.bump(); // consume `let`
        let name_tok = match self.expect(TokenKind::Ident) {
            Some(t) => t,
            None => return,
        };
        if self.expect(TokenKind::Assign).is_none() {
            return;
        }
        let expr = self.parse_expression();
        self.record_stmt(
            &name_tok.text,
            let_tok.line,
            self.last_end_line(),
            let_tok.col.saturating_sub(1) as usize,
        );
        self.extract_annotations(&name_tok.text, expr, let_tok.line);
        if self.in_include {
            // Included files provide defaults: a local `let` of the same
            // name always wins (spec §2.7).
            self.module.bindings.entry(name_tok.text).or_insert(expr);
        } else {
            self.note_duplicate_key(
                &name_tok.text,
                Some(span_of(&name_tok)),
                self.module.bindings.contains_key(&name_tok.text),
            );
            self.module.bindings.insert(name_tok.text.clone(), expr);
            // A local `let` reclaims its name: later includes must skip it.
            self.binding_spliced.remove(&name_tok.text);
        }
    }

    // ------------------------------------------------------------------
    // [table] … key = expr
    // ------------------------------------------------------------------

    fn parse_table_header(&mut self, parent: &mut indexmap::IndexMap<String, NodeId>) {
        let open = self.peek().clone();
        // Source columns are 1-based; indentation levels are 0-based.
        let header_level = open.col.saturating_sub(1) as usize;
        self.bump(); // [
        let name_tok = match self.expect(TokenKind::Ident) {
            Some(t) => t,
            None => return,
        };
        // Dotted table paths [a.b.c]: each further segment descends
        // into (or creates) a nested table; the body belongs to the
        // leaf. Single-segment headers behave exactly as before.
        let mut segments = vec![name_tok.text.clone()];
        while matches!(self.peek_kind(), TokenKind::Dot) {
            self.bump(); // .
            match self.expect(TokenKind::Ident) {
                Some(t) => segments.push(t.text.clone()),
                None => return,
            }
        }
        let close = match self.expect(TokenKind::RBracket) {
            Some(t) => t,
            None => return,
        };
        self.brace_tables += 1;
        // Descend to the leaf container, creating intermediate tables
        // (span-less synthetics, like the document root). A present
        // non-table is a structural conflict, not a silent overwrite.
        // The descent resolves owned ids with short-lived borrows only,
        // so no map borrow is held across the body parse below.
        let mut scope: Option<NodeId> = None;
        for seg in &segments[..segments.len() - 1] {
            let found = match scope {
                None => parent.get(seg).copied(),
                Some(id) => match self.module.get(id) {
                    Some(Node::Table { fields }) => fields.get(seg).copied(),
                    _ => None,
                },
            };
            let next = match found {
                Some(id) => match self.module.get(id) {
                    Some(Node::Table { .. }) => id,
                    _ => {
                        self.error(
                            "E001",
                            format!(
                                "table path '{}' conflicts with a non-table value at '{}'",
                                segments.join("."),
                                seg
                            ),
                        );
                        return;
                    }
                },
                None => {
                    let id = self.module.add_node(Node::Table {
                        fields: indexmap::IndexMap::new(),
                    });
                    match scope {
                        None => {
                            parent.insert(seg.clone(), id);
                        }
                        Some(pid) => match self.module.get_mut(pid) {
                            Some(Node::Table { fields }) => {
                                fields.insert(seg.clone(), id);
                            }
                            _ => {
                                self.error(
                                    "E001",
                                    format!(
                                        "table path '{}' conflicts with a non-table value",
                                        segments.join(".")
                                    ),
                                );
                                return;
                            }
                        },
                    }
                    id
                }
            };
            scope = Some(next);
        }
        let name = segments.join(".");
        let leaf = segments.last().cloned().unwrap_or_default();
        let stmt_idx = self.statements.len();
        self.statements.push(StmtInfo {
            key: name.clone(),
            path: self.path_str(),
            start_line: open.line,
            end_line: open.line,
            level: header_level,
            file: self.current_file_name(),
        });
        for seg in &segments {
            self.current_path.push(seg.clone());
        }
        self.blocks.push(BlockInfo {
            path: self.path_str(),
            level: header_level,
            start_line: open.line,
            file: self.current_file_name(),
        });

        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        let mut pending = Vec::new();
        self.parse_body(&mut fields, BodyEnd::HeaderBody(header_level), &mut pending);
        self.splice_includes(&mut fields, pending, false);
        let idents = self.ident_scopes.pop().unwrap_or_default();
        let _ = self.resolve_sibling_refs(idents, &fields);
        // Name-token span and file stamp, as above (C-043 scopes),
        // widened to the full header so dotted tables point at
        // `[a.b]`, not just `a`.
        let table_id = self.module.add_node(Node::Table { fields });
        self.module.record_span(
            table_id,
            Span {
                line: open.line,
                column: open.col,
                end_line: close.end_line,
                end_column: close.end_col,
            },
        );
        if let Some(file) = self.current_file_name() {
            self.module.record_file(table_id, file);
        }
        self.statements[stmt_idx].end_line = self.last_end_line();
        for _ in &segments {
            self.current_path.pop();
        }
        self.extract_annotations(&name, table_id, open.line);
        match scope {
            None => {
                self.note_duplicate_key(&leaf, Some(span_of(&open)), parent.contains_key(&leaf));
                parent.insert(leaf, table_id);
            }
            Some(pid) => {
                let existed = matches!(
                    self.module.get(pid),
                    Some(Node::Table { fields }) if fields.contains_key(&leaf)
                );
                self.note_duplicate_key(&leaf, Some(span_of(&open)), existed);
                match self.module.get_mut(pid) {
                    Some(Node::Table { fields }) => {
                        fields.insert(leaf, table_id);
                    }
                    _ => {
                        self.error(
                            "E001",
                            format!(
                                "table path '{}' conflicts with a non-table value",
                                segments.join(".")
                            ),
                        );
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // key = expr
    // ------------------------------------------------------------------

    /// Statement-level `depends_on` gate (spec/linter.md §2): array
    /// values are dependency metadata (extracted post-parse, never
    /// exported); anything else stays ordinary data. Inside profiles
    /// the metadata form is E022 and dropped. Returns false when the
    /// entry must not enter the value fields.
    fn check_depends_on_statement(&mut self, key: &str, id: NodeId) -> bool {
        if key != "depends_on" || !matches!(self.module.get(id), Some(Node::Array { .. })) {
            return true;
        }
        if self.in_profile {
            if let Some(span) = self.module.node_spans.get(&id).cloned() {
                self.error_at_span("E022", "depends_on is not allowed inside profiles", &span);
            }
            return false;
        }
        true
    }

    fn parse_key_value(&mut self) -> Option<(String, NodeId)> {
        let (key, id) = self.parse_key_value_sep(TokenKind::Assign)?;
        // Dependency blocks belong to named scopes only: an inline or
        // array-element `depends_on` array is E022 (spec/linter.md §2).
        // Non-array values stay ordinary data wherever they appear.
        if key == "depends_on" && matches!(self.module.get(id), Some(Node::Array { .. })) {
            if let Some(span) = self.module.node_spans.get(&id).cloned() {
                self.error_at_span(
                    "E022",
                    "depends_on is only allowed as a direct table member",
                    &span,
                );
            }
        }
        Some((key, id))
    }

    /// `key = expr` or `key: expr` (spec/dual-syntax.md §2).
    fn parse_key_value_sep(&mut self, sep: TokenKind) -> Option<(String, NodeId)> {
        let key_tok = self.expect(TokenKind::Ident)?;
        let key = key_tok.text.clone();
        self.expect(sep)?;
        // A `schema` key with a table value parses as a schema block
        // (spec §2.10); anything else stays ordinary data.
        if key == "schema" && matches!(self.peek_kind(), TokenKind::LBrace) {
            if self.array_depth > 0 {
                self.error_at(
                    "E020",
                    "schemas may not appear inside arrays; describe element types with Array<T>",
                    &key_tok,
                );
                return None;
            }
            if self.in_profile {
                self.error_at("E020", "schemas may not appear inside profiles", &key_tok);
                return None;
            }
            let node = self.parse_schema_inline(&key_tok);
            self.record_stmt(
                &key,
                key_tok.line,
                self.last_end_line(),
                key_tok.col.saturating_sub(1) as usize,
            );
            return Some((key, node));
        }
        // Tag Ident nodes created directly in this key's value expression
        // with the owning key so sibling resolution can skip self-references.
        let scope_start = self.ident_scopes.last().map_or(0, |s| s.len());
        let value = self.parse_expression();
        if let Some(scope) = self.ident_scopes.last_mut() {
            for entry in scope.iter_mut().skip(scope_start) {
                entry.0 = key.clone();
            }
        }
        Some((key, value))
    }

    /// Rewrite Ident nodes collected in `idents` that match a sibling key of
    /// the just-parsed scope into Ref nodes with dependency edges. Runs after
    /// the whole scope is parsed, so forward references within the scope
    /// resolve too. Returns the idents that matched nothing, for fallback
    /// passes (profile root-fallback, spec §2.6).
    fn resolve_sibling_refs(
        &mut self,
        idents: Vec<(String, NodeId)>,
        fields: &indexmap::IndexMap<String, NodeId>,
    ) -> Vec<(String, NodeId)> {
        let mut leftover = Vec::new();
        for (owner_key, id) in idents {
            let name = match self.module.get(id) {
                Some(Node::Ident(n)) => n.clone(),
                _ => continue,
            };
            // A key's own value expression must not resolve to its own
            // binding; it falls through to let-binding resolution instead.
            if name == owner_key {
                leftover.push((owner_key, id));
                continue;
            }
            if let Some(&target) = fields.get(&name) {
                if let Some(slot) = self.module.get_mut(id) {
                    *slot = Node::Ref(target);
                }
                self.module.add_edge(target, id);
            } else {
                leftover.push((owner_key, id));
            }
        }
        leftover
    }

    // ------------------------------------------------------------------
    // # @intent / @constraint / @ai_immutable  (spec/annotations.md, C-033)
    // ------------------------------------------------------------------

    /// Extract annotations for a just-parsed statement (spec §1). The
    /// contiguous comment block directly above `stmt_line` is scanned;
    /// known `@…` lines attach to `node` (schemas never do), everything
    /// else stays an ordinary comment for the formatter. `node`'s scope
    /// path plus `key` becomes the enforcement path. Call sites are
    /// statement-level only — inline-table values never attach (their
    /// comments cannot precede a binding line).
    fn extract_annotations(&mut self, key: &str, node: NodeId, stmt_line: u32) {
        if matches!(self.module.get(node), Some(Node::SchemaTable { .. })) {
            return;
        }
        // Profile bodies take no annotations (spec §1): their paths
        // are variant-qualified and enforcement works on document
        // paths. Profiles keep their comments as plain trivia.
        if self.in_profile {
            return;
        }
        let scope = self.path_str();
        let full_path = if scope.is_empty() {
            key.to_string()
        } else {
            format!("{scope}.{key}")
        };
        // Walk upward through consecutive comment lines. A comment
        // sharing its line with a statement end is trailing on that
        // line — it belongs there, never to the block above. Matching
        // is by (line, file): line numbers repeat across includes.
        let here = self.current_file_name();
        let mut line = stmt_line.saturating_sub(1);
        let mut block: Vec<(String, Span)> = Vec::new();
        while line >= 1 {
            let comment = self
                .comments
                .iter()
                .zip(self.comment_files.iter())
                .find(|((_, s), f)| s.line == line && *f == &here)
                .map(|((text, span), _)| (text.clone(), span.clone()));
            let Some((text, span)) = comment else {
                break;
            };
            if self
                .statements
                .iter()
                .any(|st| st.end_line == line && st.file == here)
            {
                break;
            }
            block.push((text, span));
            if line == 1 {
                break;
            }
            line -= 1;
        }
        // Source order, so multi-annotation nodes read deterministically.
        for (text, span) in block.into_iter().rev() {
            if let Some(kind) = self.parse_annotation_line(&text, &span, node) {
                self.module
                    .annotations
                    .entry(node)
                    .or_default()
                    .push(Annotation {
                        kind,
                        path: full_path.clone(),
                        span,
                    });
            }
        }
    }

    /// Parse one comment line as an annotation. Ordinary comments —
    /// including unknown `@…` kinds — return None silently. Malformed
    /// known annotations are E021 at the comment's span.
    fn parse_annotation_line(
        &mut self,
        text: &str,
        span: &Span,
        target: NodeId,
    ) -> Option<AnnotationKind> {
        let body = text.strip_prefix('#').unwrap_or(text).trim_start();
        let rest = body.strip_prefix('@')?;
        let kind_len = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        let (kind, after) = rest.split_at(kind_len);
        match kind {
            "intent" | "constraint" | "ai_immutable" => {}
            _ => return None,
        }
        // Bare `# @ai_immutable` (no payload at all) is the flag form
        // of `true` — older documents used it as a plain comment, and
        // annotations must never turn previously-valid documents into
        // errors.
        if kind == "ai_immutable" && after.trim_start().is_empty() {
            return Some(AnnotationKind::AiImmutable(true));
        }
        let payload = match after.trim_start().strip_prefix(':') {
            Some(p) => p.trim_start(),
            None => {
                self.error_at_span(
                    "E021",
                    format!("invalid @{kind} annotation: expected ':' after the kind"),
                    span,
                );
                return None;
            }
        };
        match kind {
            // String literals are preferred, but bare prose is also an
            // intent: documentation must never punish informality, and
            // older documents already used free-text `@intent` comments.
            // A payload that opens a quote is held to literal rules.
            "intent" => {
                let wants_literal = payload.starts_with('"') || payload.starts_with('\'');
                match parse_annotation_string(payload) {
                    Ok(documented) => Some(AnnotationKind::Intent(documented)),
                    Err(_) if !wants_literal && !payload.is_empty() => {
                        Some(AnnotationKind::Intent(payload.to_string()))
                    }
                    Err(detail) => {
                        self.error_at_span(
                            "E021",
                            format!("invalid @intent annotation: {detail}"),
                            span,
                        );
                        None
                    }
                }
            }
            "ai_immutable" => match payload {
                "true" => Some(AnnotationKind::AiImmutable(true)),
                "false" => Some(AnnotationKind::AiImmutable(false)),
                _ => {
                    self.error_at_span(
                        "E021",
                        "invalid @ai_immutable annotation: expected 'true' or 'false'",
                        span,
                    );
                    None
                }
            },
            "constraint" => {
                if payload.is_empty() {
                    self.error_at_span(
                        "E021",
                        "invalid @constraint annotation: empty predicate",
                        span,
                    );
                    return None;
                }
                // 1-based column where the payload starts: the comment's
                // column plus the characters before the payload.
                let payload_col =
                    span.column + (text.chars().count() - payload.chars().count()) as u32;
                self.parse_constraint(payload, span, payload_col, target)
                    .map(|expr| AnnotationKind::Constraint {
                        source: payload.to_string(),
                        expr,
                    })
            }
            _ => None,
        }
    }

    /// Parse a `@constraint` payload as a boolean expression, splicing
    /// nodes into the shared arena (single-line sibling of
    /// parse_embedded_expression: same arena swap, ident transfer, and
    /// span rebase, but anchored on the comment position). `value`
    /// references rewrite to `target` before ident transfer so the name
    /// can never resolve as a sibling or binding. Returns None after
    /// recording E021.
    fn parse_constraint(
        &mut self,
        payload: &str,
        comment: &Span,
        payload_col: u32,
        target: NodeId,
    ) -> Option<NodeId> {
        let mut lexer = Lexer::new(payload);
        let lexed = lexer.tokenize();
        let module = std::mem::replace(&mut self.module, Module::new(""));
        let inherited_len = self.ident_scopes.last().map_or(0, |s| s.len());
        let mut sub = Parser {
            tokens: lexed.tokens,
            pos: 0,
            module,
            errors: Vec::new(),
            ident_scopes: self.ident_scopes.clone(),
            depth: self.depth,
            options: self.options.clone(),
            block_depth: 0,
            schema_depth: self.schema_depth,
            array_depth: self.array_depth,
            comments: Vec::new(),
            comment_files: Vec::new(),
            statements: Vec::new(),
            blocks: Vec::new(),
            current_path: self.current_path.clone(),
            brace_tables: 0,
            indent_tables: 0,
            current_file: self.current_file.clone(),
            include_stack: Vec::new(),
            in_include: self.in_include,
            in_profile: self.in_profile,
            binding_spliced: self.binding_spliced.clone(),
            pending_profiles: Vec::new(),
            trust: self.trust.clone(),
        };
        let node_base = sub.module.arena.len() as u32;
        let id = sub.parse_expression();
        while matches!(sub.peek_kind(), TokenKind::Newline) {
            sub.bump();
        }
        if !matches!(sub.peek_kind(), TokenKind::Eof) {
            let leftover = sub.peek().clone();
            sub.error_at(
                "E021",
                format!(
                    "invalid @constraint annotation: unexpected {:?} after the predicate",
                    leftover.kind
                ),
                &leftover,
            );
        }
        // `value` ≡ the annotated node (spec §3): rewrite direct
        // references so they never enter sibling/binding resolution.
        let mut rewritten = std::collections::HashSet::new();
        for i in node_base..sub.module.arena.len() as u32 {
            let nid = NodeId(i);
            if matches!(sub.module.get(nid), Some(Node::Ident(name)) if name == "value") {
                if let Some(slot) = sub.module.get_mut(nid) {
                    *slot = Node::Ref(target);
                }
                sub.module.add_edge(target, nid);
                rewritten.insert(nid);
            }
        }
        let new_idents: Vec<(String, NodeId)> = sub
            .ident_scopes
            .last()
            .map(|s| {
                s[inherited_len.min(s.len())..]
                    .iter()
                    .filter(|(_, nid)| !rewritten.contains(nid))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // Payload-local spans (line 1) land on the comment line with
        // columns shifted by the payload start. Payloads are single
        // line by construction (comment text never holds `\n`).
        let lift = |line_1: u32, col_1: u32| -> (u32, u32) {
            let line = comment.line + line_1.saturating_sub(1);
            let col = if line_1 <= 1 {
                payload_col + col_1.saturating_sub(1)
            } else {
                col_1
            };
            (line, col)
        };
        for (node, span) in sub.module.node_spans.iter_mut() {
            if node.0 >= node_base {
                let (line, col) = lift(span.line, span.column);
                span.line = line;
                span.column = col;
                let (line, col) = lift(span.end_line, span.end_column);
                span.end_line = line;
                span.end_column = col;
            }
        }
        let mut sub_errors = std::mem::take(&mut sub.errors);
        let mut sub_lex_errors = lexed.errors;
        for d in sub_errors.iter_mut().chain(sub_lex_errors.iter_mut()) {
            if let Some(s) = &mut d.span {
                let (line, col) = lift(s.line, s.column);
                s.line = line;
                s.column = col;
                let (line, col) = lift(s.end_line, s.end_column);
                s.end_line = line;
                s.end_column = col;
            }
            if d.file.is_none() {
                d.file = self.current_file_name();
            }
        }
        self.errors.extend(sub_lex_errors);
        self.errors.extend(sub_errors);
        self.module = std::mem::replace(&mut sub.module, Module::new(""));
        if let Some(scope) = self.ident_scopes.last_mut() {
            scope.extend(new_idents);
        }
        Some(id)
    }

    // ------------------------------------------------------------------
    // Expression parser — classic precedence climbing
    // ------------------------------------------------------------------

    fn parse_expression(&mut self) -> NodeId {
        let guard_tok = self.peek().clone();
        self.depth += 1;
        if self.depth > MAX_EXPR_DEPTH {
            self.error(
                "E004",
                format!("maximum expression nesting depth ({MAX_EXPR_DEPTH}) exceeded"),
            );
            self.depth -= 1;
            self.bump(); // consume the current token to guarantee progress
            return self.add_spanned(Node::Literal(Value::Null), &guard_tok);
        }
        let node = self.parse_or();
        self.depth -= 1;
        node
    }

    fn parse_or(&mut self) -> NodeId {
        let mut left = self.parse_and();
        while matches!(self.peek_kind(), TokenKind::OrOr) {
            let op_tok = self.bump();
            let right = self.parse_and();
            left = self.add_spanned(
                Node::Binary {
                    op: BinOp::Or,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_and(&mut self) -> NodeId {
        let mut left = self.parse_equality();
        while matches!(self.peek_kind(), TokenKind::AndAnd) {
            let op_tok = self.bump();
            let right = self.parse_equality();
            left = self.add_spanned(
                Node::Binary {
                    op: BinOp::And,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_equality(&mut self) -> NodeId {
        let mut left = self.parse_comparison();
        loop {
            let op = match self.peek_kind() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::NotEq => BinOp::Ne,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_comparison();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_comparison(&mut self) -> NodeId {
        let mut left = self.parse_term();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::LtEq => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::GtEq => BinOp::Ge,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_term();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_term(&mut self) -> NodeId {
        let mut left = self.parse_factor();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_factor();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_factor(&mut self) -> NodeId {
        let mut left = self.parse_unary();
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                _ => break,
            };
            let op_tok = self.bump();
            let right = self.parse_unary();
            left = self.add_spanned(
                Node::Binary {
                    op,
                    lhs: left,
                    rhs: right,
                },
                &op_tok,
            );
        }
        left
    }

    fn parse_unary(&mut self) -> NodeId {
        match self.peek_kind() {
            TokenKind::Minus | TokenKind::Bang => {
                let op_tok = self.peek().clone();
                // Unary chains recurse through parse_unary, so the depth
                // guard must live here too (parse_expression cannot see it).
                self.depth += 1;
                if self.depth > MAX_EXPR_DEPTH {
                    self.error(
                        "E004",
                        format!("maximum expression nesting depth ({MAX_EXPR_DEPTH}) exceeded"),
                    );
                    self.depth -= 1;
                    self.bump(); // consume the operator to guarantee progress
                    return self.add_spanned(Node::Literal(Value::Null), &op_tok);
                }
                let op = if matches!(self.peek_kind(), TokenKind::Minus) {
                    UnOp::Neg
                } else {
                    UnOp::Not
                };
                self.bump();
                let expr = self.parse_unary();
                self.depth -= 1;
                // Affine units are order-sensitive: `-40Fahr` must negate the
                // literal BEFORE the Fahrenheit→kelvin transform, otherwise
                // the famous crossover silently breaks (spec Ch. 4 §4.1).
                // Linear units are unaffected (negation commutes with scaling
                // and clamps), so only Celsius/Fahrenheit fold here.
                if op == UnOp::Neg {
                    if let Some(folded) = self.fold_affine_negation(expr) {
                        return folded;
                    }
                }
                self.add_spanned(Node::Unary { op, expr }, &op_tok)
            }
            _ => self.parse_primary(),
        }
    }

    /// Fold unary minus into an affine-unit quantity literal.
    ///
    /// `-40Fahr` must mean negative-forty-Fahrenheit (233.15 K), not the
    /// negation of positive-forty-Fahrenheit in kelvin: affine transforms
    /// do not commute with negation (spec Ch. 4 §4.1). Linear units are
    /// unaffected (negation commutes with scaling), so only
    /// Celsius/Fahrenheit fold here. Returns None for any other operand.
    fn fold_affine_negation(&mut self, expr: NodeId) -> Option<NodeId> {
        let value_id = match self.module.get(expr) {
            Some(Node::Quantity {
                value,
                unit: Unit::Celsius | Unit::Fahrenheit,
            }) => *value,
            _ => return None,
        };
        match self.module.get_mut(value_id) {
            Some(Node::Literal(Value::Int(n))) => {
                *n = n.saturating_neg();
            }
            Some(Node::Literal(Value::Float(f))) => {
                *f = -*f;
            }
            _ => return None,
        }
        Some(expr)
    }

    // ------------------------------------------------------------------
    // Primary expressions
    // ------------------------------------------------------------------

    fn parse_primary(&mut self) -> NodeId {
        let tok = self.peek().clone();
        match &tok.kind {
            TokenKind::String(src_pos) => {
                self.bump();
                let linemap = linemap_from_charmap(&tok.text, tok.line, src_pos);
                let marks = Self::sentinel_marks(&tok.text, &tok.kind);
                self.parse_string_literal(&tok.text, &tok, Some(&linemap), &marks)
            }

            TokenKind::SingleString => {
                // Single-quoted strings are literal: `${…}` is not special
                // (spec/grammar.md §2.5).
                self.bump();
                let text = tok.text.clone();
                self.add_spanned(Node::Literal(Value::String(text)), &tok)
            }

            TokenKind::MlString(src_pos) => {
                // Triple-double-quoted: common-indent stripping, then full
                // `${…}` expression interpolation (spec §1.4.1). The line
                // map keeps interpolation diagnostics on their true source
                // positions after stripping; src_pos corrects for `\n`
                // escapes decoded into newlines.
                self.bump();
                let (stripped, linemap, marks) =
                    strip_common_indent_mapped(&tok.text, tok.line, tok.col, 3, Some(src_pos));
                self.parse_string_literal(&stripped, &tok, Some(&linemap), &marks)
            }

            TokenKind::MlSingleString => {
                // Triple-single-quoted: common-indent stripping, fully
                // literal (spec §1.4.1).
                self.bump();
                let stripped = strip_common_indent(&tok.text);
                self.add_spanned(Node::Literal(Value::String(stripped)), &tok)
            }

            TokenKind::At => {
                self.bump();
                // Patch directives are statements, never values.
                if matches!(self.peek_kind(), TokenKind::Ident) {
                    let tag_tok = self.peek().clone();
                    let tag = tag_tok.text.clone();
                    if (tag == "patch" || tag == "remove" || tag == "rename")
                        && matches!(self.peek_kind_at(1), TokenKind::LParen)
                    {
                        self.error_at(
                            "E019",
                            "patch statements may only appear at document root, never as values",
                            &tag_tok,
                        );
                        return self.add_spanned(Node::Literal(Value::Null), &tok);
                    }
                }
                self.parse_semantic_block(&tok)
            }

            TokenKind::Integer => {
                self.bump();
                // Out-of-range literals must never silently become 0:
                // E023 records the overflow and the value saturates at
                // i64::MAX (spec/specification.md §1.3). A leading `-`
                // is a separate unary operator, so `-9223372036854775808`
                // evaluates to `i64::MIN + 1`, still with E023.
                let n: i64 = match tok.text.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        self.error_at(
                            "E023",
                            format!("integer literal out of i64 range: `{}`", tok.text),
                            &tok,
                        );
                        i64::MAX
                    }
                };
                let mut node = self.add_spanned(Node::Literal(Value::Int(n)), &tok);
                // optional unit suffix
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.add_spanned(Node::Quantity { value: node, unit }, &tok);
                    }
                }
                node
            }

            TokenKind::Float => {
                self.bump();
                let f: f64 = tok.text.parse().unwrap_or(0.0);
                let mut node = self.add_spanned(Node::Literal(Value::Float(f)), &tok);
                if let TokenKind::Unit(_) = self.peek_kind() {
                    let unit_tok = self.bump();
                    if let Some(unit) = Unit::from_suffix(&unit_tok.text) {
                        node = self.add_spanned(Node::Quantity { value: node, unit }, &tok);
                    }
                }
                node
            }

            TokenKind::True => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Bool(true)), &tok)
            }
            TokenKind::False => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Bool(false)), &tok)
            }
            TokenKind::Null => {
                self.bump();
                self.add_spanned(Node::Literal(Value::Null), &tok)
            }

            TokenKind::Env => {
                self.bump();
                self.expect(TokenKind::Dot);
                let var = self
                    .expect(TokenKind::Ident)
                    .map(|t| t.text)
                    .unwrap_or_default();
                let default = if matches!(self.peek_kind(), TokenKind::QuestionColon) {
                    self.bump();
                    Some(self.parse_expression())
                } else {
                    None
                };
                self.add_spanned(Node::Env { name: var, default }, &tok)
            }

            TokenKind::Secret => {
                self.bump();
                // `secret(expr)` — locator is the expression; or
                // `secret.env(name)` — name is a string literal and the
                // secret is the env var's value.
                let from_env = if matches!(self.peek_kind(), TokenKind::Dot) {
                    self.bump();
                    if !matches!(self.peek_kind(), TokenKind::Env) {
                        self.error(
                            "E003",
                            "expected `env` after `secret.` (use secret.env(name))",
                        );
                    } else {
                        self.bump();
                    }
                    true
                } else {
                    false
                };
                self.expect(TokenKind::LParen);
                let arg = self.parse_expression();
                self.expect(TokenKind::RParen);
                let locator = if from_env {
                    match self.module.get(arg) {
                        Some(Node::Literal(Value::String(name))) => self.add_spanned(
                            Node::Env {
                                name: name.clone(),
                                default: None,
                            },
                            &tok,
                        ),
                        _ => {
                            self.error(
                                "E003",
                                "secret.env() requires a string literal env var name",
                            );
                            arg
                        }
                    }
                } else {
                    arg
                };
                self.add_spanned(Node::Secret { locator }, &tok)
            }

            TokenKind::Ident => {
                self.bump();
                // Will be turned into Ref during build_graph if it matches a
                // let, or during sibling resolution if it matches a key in the
                // enclosing table scope.
                let text = tok.text.clone();
                let id = self.add_spanned(Node::Ident(text), &tok);
                if let Some(scope) = self.ident_scopes.last_mut() {
                    scope.push((String::new(), id));
                }
                id
            }

            TokenKind::LBracket => self.parse_array(),
            TokenKind::LBrace => self.parse_inline_table(),

            TokenKind::LParen => {
                self.bump();
                let expr = self.parse_expression();
                self.expect(TokenKind::RParen);
                expr
            }

            _ => {
                self.error("E002", format!("unexpected token {:?}", tok.kind));
                self.bump();
                self.module.add_node(Node::Literal(Value::Null))
            }
        }
    }

    /// Parse `@tag """…"""` — a multi-line string with a semantic tag
    /// (spec §1.4.1). The tag is tool treatment only; the value is the
    /// inner string. Unknown tags and non-multi-line payloads are E014.
    fn parse_semantic_block(&mut self, at_tok: &Token) -> NodeId {
        let tag_tok = self.peek().clone();
        let tag = match &tag_tok.kind {
            TokenKind::Ident => {
                self.bump();
                tag_tok.text.clone()
            }
            _ => {
                self.error_at(
                    "E014",
                    "expected a semantic tag (prompt, policy, template) after `@`",
                    &tag_tok,
                );
                String::new()
            }
        };
        let tag = match SemanticTag::parse_tag(&tag) {
            Some(t) => t,
            None => {
                self.error_at(
                    "E014",
                    format!("unknown semantic tag `@{tag}` (expected prompt, policy, or template)"),
                    &tag_tok,
                );
                SemanticTag::Prompt
            }
        };
        let str_tok = self.peek().clone();
        let inner = match &str_tok.kind {
            TokenKind::MlString(src_pos) => {
                self.bump();
                let (stripped, linemap, marks) = strip_common_indent_mapped(
                    &str_tok.text,
                    str_tok.line,
                    str_tok.col,
                    3,
                    Some(src_pos),
                );
                self.parse_string_literal(&stripped, &str_tok, Some(&linemap), &marks)
            }
            TokenKind::MlSingleString => {
                self.bump();
                let stripped = strip_common_indent(&str_tok.text);
                self.add_spanned(Node::Literal(Value::String(stripped)), &str_tok)
            }
            _ => {
                self.error_at(
                    "E014",
                    "a semantic tag must be followed by a multi-line string (`\"\"\"…\"\"\"` or `'''…'''`)",
                    &str_tok,
                );
                self.add_spanned(Node::Literal(Value::String(String::new())), at_tok)
            }
        };
        let id = self.add_spanned(Node::Semantic { tag, inner }, at_tok);
        // The inner→outer edge is recorded in build_graph with every other
        // node kind (single place for edge creation).
        id
    }
}

/// Strip the common leading-whitespace prefix from a multi-line string
/// (spec §1.4.1): drop the leading newline when the first line is blank,
/// remove the longest common whitespace prefix of all non-blank lines
/// (tabs and spaces are distinct characters — a line whose indentation
/// diverges keeps its excess), and drop a trailing whitespace-only line
/// before the closing delimiter.
fn strip_common_indent(raw: &str) -> String {
    strip_common_indent_mapped(raw, 1, 1, 0, None).0
}
/// Where one stripped line sits in the source: the source line number and
/// the source column of every stripped character on that line.
#[derive(Debug, Clone)]
struct MlLineMap {
    src_line: u32,
    src_cols: Vec<u32>,
}

/// Build a line map from a token's per-character source positions:
/// group decoded characters into lines, recording each line's source
/// line and per-character source columns. Used for single-line strings
/// (whose escapes still decode (\n, \t) and shift positions).
fn linemap_from_charmap(text: &str, tok_line: u32, charmap: &[(u32, u32)]) -> Vec<MlLineMap> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            lines.push((start, i));
            start = i + 1;
        }
    }
    lines.push((start, chars.len()));
    lines
        .iter()
        .map(|&(s, e)| {
            let src_line = tok_line + pos_entry(charmap, s, (0, 0)).0;
            let src_cols = (s..e).map(|k| pos_entry(charmap, k, (0, 0)).1).collect();
            MlLineMap { src_line, src_cols }
        })
        .collect()
}

/// Strip common indent and record, for every stripped line, its source
/// position. `tok_line`/`tok_col` locate the opening delimiter;
/// `src_pos` holds the source position (physical line offset, column) of
/// every decoded character (`None` means decoded and physical positions
/// coincide, as for verbatim strings). Also returns sentinel marks parallel
/// to the stripped text: `\$`-sentinels survive stripping and stay marked.
fn strip_common_indent_mapped(
    raw: &str,
    tok_line: u32,
    tok_col: u32,
    delim_len: u32,
    src_pos: Option<&[(u32, u32)]>,
) -> (String, Vec<MlLineMap>, Vec<bool>) {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    // Raw line indices that survive: drop a blank first line (the newline
    // after the delimiter) and a whitespace-only last line (indentation
    // before the closing delimiter).
    let mut kept: Vec<usize> = (0..lines.len()).collect();
    if !lines.is_empty() && lines[0].trim().is_empty() {
        kept.remove(0);
        lines.remove(0);
    }
    if !lines.is_empty() && lines.last().is_some_and(|l| l.trim().is_empty()) {
        kept.pop();
        lines.pop();
    }
    // Longest shared whitespace prefix over the surviving content lines.
    let mut prefix: Vec<char> = Vec::new();
    let mut seen_content = false;
    for line in lines.iter() {
        if line.trim().is_empty() {
            continue;
        }
        let indent: Vec<char> = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if !seen_content {
            prefix = indent;
            seen_content = true;
        } else {
            let shared = prefix
                .iter()
                .zip(indent.iter())
                .take_while(|(a, b)| a == b)
                .count();
            prefix.truncate(shared);
        }
        if prefix.is_empty() {
            break;
        }
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut map = Vec::with_capacity(lines.len());
    // Sentinel marks parallel to the stripped text: `\$`-sentinels survive
    // stripping (they are never whitespace), carried per kept character.
    let mut out_marks: Vec<Vec<bool>> = Vec::with_capacity(lines.len());
    // Decoded character index of each surviving line's first character,
    // for looking up per-character source positions. Recomputed from the
    // original split (before blank-line removal): each line starts after
    // the previous line's characters plus its newline.
    let all: Vec<&str> = raw.split('\n').collect();
    let mut line_start: Vec<usize> = Vec::with_capacity(all.len());
    let mut acc = 0usize;
    for l in &all {
        line_start.push(acc);
        acc += l.chars().count() + 1; // +1 for the newline
    }
    let line_start_for =
        |raw_idx: usize| -> usize { line_start.get(raw_idx).copied().unwrap_or(0) };
    for (j, l) in lines.iter().enumerate() {
        let raw_idx = kept[j];
        let decoded_start = line_start_for(raw_idx);
        let at = |k: usize| -> (u32, u32) {
            match src_pos {
                // Marked sentinel entries inherit a real position: marks
                // must never reach line/column arithmetic.
                Some(p) => pos_entry(p, k, (raw_idx as u32, k as u32)),
                None => (raw_idx as u32, k as u32),
            }
        };
        let is_marked =
            |k: usize| -> bool { src_pos.and_then(|p| p.get(k)).copied() == Some(SENTINEL_POS) };
        let shared = if l.trim().is_empty() {
            0
        } else {
            prefix
                .iter()
                .zip(l.chars())
                .take_while(|(a, b)| *a == b)
                .count()
        };
        // Whitespace-only interior lines normalize to empty (spec §1.4.1).
        if l.trim().is_empty() {
            let (line_off, _) = at(decoded_start);
            map.push(MlLineMap {
                src_line: tok_line + line_off,
                src_cols: Vec::new(),
            });
            out.push(String::new());
            out_marks.push(Vec::new());
            continue;
        }
        let text: String = l.chars().skip(shared).collect();
        // Source line/columns of the kept characters.
        let first_kept = decoded_start + shared;
        let (line_off, _) = at(first_kept.min(decoded_start + l.chars().count()));
        let src_line = tok_line + line_off;
        let cols: Vec<u32> = if src_pos.is_some() {
            (0..text.chars().count())
                .map(|c| at(first_kept + c).1)
                .collect()
        } else if raw_idx == 0 {
            // Synthetic positions: content follows the opening delimiter.
            (0..text.chars().count())
                .map(|c| tok_col + delim_len + shared as u32 + c as u32)
                .collect()
        } else {
            (0..text.chars().count())
                .map(|c| shared as u32 + 1 + c as u32)
                .collect()
        };
        // Marks for the kept characters: a kept `\$`-sentinel stays marked
        // exactly when its source entry is marked.
        let mline: Vec<bool> = text
            .chars()
            .enumerate()
            .map(|(c, ch)| ch == ESCAPED_DOLLAR && is_marked(first_kept + c))
            .collect();
        out.push(text);
        out_marks.push(mline);
        map.push(MlLineMap {
            src_line,
            src_cols: cols,
        });
    }
    let mut marks = Vec::new();
    for (j, mline) in out_marks.iter().enumerate() {
        if j > 0 {
            marks.push(false);
        }
        marks.extend(mline.iter().copied());
    }
    (out.join("\n"), map, marks)
}

impl Parser {
    // ------------------------------------------------------------------
    // @patch / @remove / @rename  (spec §2.9, C-028)
    // ------------------------------------------------------------------

    /// Parse a patch directive statement. The `@tag` and `(` are already
    /// confirmed by the caller; `at_tok` is the `@` token for spans.
    /// Directives live at document root only and are recorded in
    /// encounter order on `module.patches`. The full statement is always
    /// consumed (even on error) so the enclosing loop makes progress.
    fn parse_patch_statement(&mut self, at_tok: &Token) {
        let nested = !self.current_path.is_empty();
        self.bump(); // consume `@`
        let tag_tok = self.bump(); // consume patch/remove/rename
        let tag = tag_tok.text.clone();
        if self.expect(TokenKind::LParen).is_none() {
            return;
        }
        let path_tok = self.peek().clone();
        let path = match &path_tok.kind {
            TokenKind::String(_) | TokenKind::SingleString => {
                self.bump();
                path_tok.text.clone()
            }
            _ => {
                self.error_at("E019", "patch paths must be string literals", &path_tok);
                return;
            }
        };
        if path.contains("${")
            || Self::has_marked(&path, &Self::sentinel_marks(&path, &path_tok.kind))
        {
            self.error_at(
                "E019",
                "interpolation in patch paths is a hard error; paths must be static",
                &path_tok,
            );
            return;
        }
        if let Err(why) = validate_patch_path(&path) {
            self.error_at(
                "E019",
                format!("invalid patch path `{path}`: {why}"),
                &path_tok,
            );
            return;
        }
        let op = match tag.as_str() {
            "remove" => {
                if self.expect(TokenKind::RParen).is_none() {
                    return;
                }
                PatchOp::Remove { path: path.clone() }
            }
            "rename" => {
                if self.expect(TokenKind::Comma).is_none() {
                    return;
                }
                let new_tok = self.peek().clone();
                let new_name = match &new_tok.kind {
                    TokenKind::String(_) | TokenKind::SingleString => {
                        self.bump();
                        Self::render_marked(
                            &new_tok.text,
                            &Self::sentinel_marks(&new_tok.text, &new_tok.kind),
                        )
                    }
                    _ => {
                        self.error_at(
                            "E019",
                            "rename needs a new key name as a string literal",
                            &new_tok,
                        );
                        return;
                    }
                };
                if new_name.contains('.') || new_name.contains('[') || new_name.is_empty() {
                    self.error_at(
                        "E019",
                        "rename target must be a plain key name in the same parent",
                        &new_tok,
                    );
                    return;
                }
                if self.expect(TokenKind::RParen).is_none() {
                    return;
                }
                PatchOp::Rename {
                    from: path.clone(),
                    to: new_name,
                }
            }
            _ => {
                // patch
                if self.expect(TokenKind::Comma).is_none() {
                    return;
                }
                let value = self.parse_expression();
                if self.expect(TokenKind::RParen).is_none() {
                    return;
                }
                PatchOp::Set {
                    path: path.clone(),
                    value,
                }
            }
        };
        let stmt_key = match &op {
            PatchOp::Set { path, .. } => format!("@patch({path})"),
            PatchOp::Remove { path } => format!("@remove({path})"),
            PatchOp::Rename { from, .. } => format!("@rename({from})"),
        };
        if nested {
            self.error_at(
                "E019",
                "patch statements may only appear at document root",
                at_tok,
            );
            return;
        }
        self.record_stmt(
            &stmt_key,
            at_tok.line,
            self.last_end_line(),
            at_tok.col.saturating_sub(1) as usize,
        );
        let id = self.add_spanned(Node::PatchDirective { op }, at_tok);
        // The value expression participates in the graph so patches are
        // dependency-aware; paths are static and need no edges.
        if let Some(Node::PatchDirective {
            op: PatchOp::Set { value, .. },
        }) = self.module.get(id)
        {
            let value = *value;
            self.module.add_edge(value, id);
        }
        self.module.patches.push(id);
    }

    // ------------------------------------------------------------------
    // Schema blocks (spec §2.10, C-030)
    // ------------------------------------------------------------------

    /// Parse an inline schema table after `schema =` (the `{` is next).
    /// Returns a SchemaTable node for later extraction.
    fn parse_schema_inline(&mut self, key_tok: &Token) -> NodeId {
        let open = self
            .expect(TokenKind::LBrace)
            .unwrap_or_else(|| key_tok.clone());
        let (fields, closed) = self.parse_schema_entries(SchemaEnd::Brace);
        self.add_spanned(Node::SchemaTable { fields, closed }, &open)
    }

    /// Parse a `schema { … }` brace block (the `{` is next).
    fn parse_schema_brace(&mut self, key_tok: &Token) -> NodeId {
        let open = self
            .expect(TokenKind::LBrace)
            .unwrap_or_else(|| key_tok.clone());
        self.brace_tables += 1;
        let (fields, closed) = self.parse_schema_entries(SchemaEnd::Brace);
        self.add_spanned(Node::SchemaTable { fields, closed }, &open)
    }

    /// Parse a `schema:` indented body (the Indent is next).
    fn parse_schema_indent(&mut self, key_tok: &Token, _level: usize) -> NodeId {
        let (fields, closed) = self.parse_schema_entries(SchemaEnd::Indent);
        self.add_spanned(Node::SchemaTable { fields, closed }, key_tok)
    }

    /// Parse `name = type` entries (plus the reserved `closed` flag)
    /// until the terminator. Returns the fields and the closed flag.
    fn parse_schema_entries(&mut self, end: SchemaEnd) -> (IndexMap<String, NodeId>, bool) {
        let mut fields = IndexMap::new();
        let mut closed = false;
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                TokenKind::Eof => {
                    // An indentation-form schema ends naturally at EOF
                    // (like any indent block), but a brace form without
                    // its closer is a syntax error (E001), never silent.
                    if matches!(end, SchemaEnd::Brace) {
                        self.error(
                            "E001",
                            "unclosed schema block: expected `}` before end of file",
                        );
                    }
                    break;
                }
                TokenKind::RBrace if matches!(end, SchemaEnd::Brace) => {
                    self.bump();
                    break;
                }
                TokenKind::Dedent if matches!(end, SchemaEnd::Indent) => {
                    self.bump();
                    break;
                }
                TokenKind::RBrace | TokenKind::Dedent => {
                    // Wrong closer for this form: stop and let the caller
                    // diagnose (keeps progress either way).
                    break;
                }
                TokenKind::Comma => {
                    self.bump();
                    continue;
                }
                TokenKind::Ident => {
                    let name_tok = self.bump();
                    let name = name_tok.text.clone();
                    if self.expect(TokenKind::Assign).is_none() {
                        continue;
                    }
                    if name == "closed" {
                        match self.peek_kind() {
                            TokenKind::True => {
                                self.bump();
                                closed = true;
                            }
                            TokenKind::False => {
                                self.bump();
                                closed = false;
                            }
                            _ => {
                                let tok = self.peek().clone();
                                self.error_at("E020", "closed must be true or false", &tok);
                            }
                        }
                        self.skip_newlines();
                        if matches!(self.peek_kind(), TokenKind::Comma) {
                            self.bump();
                        }
                        continue;
                    }
                    let id = self.parse_schema_type(&name_tok);
                    // Tag arg idents with the entry name for sibling
                    // resolution parity with value parsing.
                    // Schema entries are statements for comment
                    // attachment (spec/dual-syntax.md §7): per-field
                    // documentation comments attach like value
                    // bindings, so the C-048 docgen renders them.
                    // Top-level tables only (schema_depth == 0):
                    // nested inline element fields would record
                    // document-level statements under bare names the
                    // docgen never renders, shadowing real ones.
                    if self.schema_depth == 0 {
                        self.record_stmt(
                            &name,
                            name_tok.line,
                            self.last_end_line(),
                            name_tok.col.saturating_sub(1) as usize,
                        );
                    }
                    fields.insert(name, id);
                    self.skip_newlines();
                    if matches!(self.peek_kind(), TokenKind::Comma) {
                        self.bump();
                    }
                }
                _ => {
                    let tok = self.peek().clone();
                    self.error_at("E020", "expected a schema entry (`name = Type`)", &tok);
                    self.bump();
                }
            }
        }
        (fields, closed)
    }

    /// Guard one level of schema-type recursion (`Array<…>` nesting,
    /// nested `Table {…}`) against stack exhaustion. Returns true when
    /// the limit is exceeded: the counter is already restored and E004
    /// recorded, so the caller bails out with a fallback field. Every
    /// `true`…`false` pair brackets exactly one recursive call, so a
    /// matching `exit_schema_depth` must follow the non-bail path.
    fn enter_schema_depth(&mut self, tok: &Token) -> bool {
        self.schema_depth += 1;
        if self.schema_depth > MAX_SCHEMA_DEPTH {
            self.schema_depth -= 1;
            self.error_at(
                "E004",
                format!("maximum schema nesting depth ({MAX_SCHEMA_DEPTH}) exceeded"),
                tok,
            );
            return true;
        }
        false
    }

    fn exit_schema_depth(&mut self) {
        self.schema_depth = self.schema_depth.saturating_sub(1);
    }

    /// Parse one schema type expression into a SchemaField,
    /// SchemaTable, or SchemaArray node (spec §2.10). `name_tok`
    /// positions the entry for diagnostics.
    fn parse_schema_type(&mut self, _name_tok: &Token) -> NodeId {
        let tok = self.peek().clone();
        let TokenKind::Ident = &tok.kind else {
            self.error_at("E020", "expected a schema type", &tok);
            self.bump();
            return self.fallback_schema_field(&tok);
        };
        let type_name = tok.text.clone();
        // Array<T>: element type in angle brackets (nestable).
        if type_name == "Array" {
            self.bump();
            if self.expect(TokenKind::Lt).is_none() {
                self.error_at("E020", "Array needs an element type: Array<T>", &tok);
                return self.fallback_schema_field(&tok);
            }
            let elem_tok = self.peek().clone();
            if self.enter_schema_depth(&tok) {
                return self.fallback_schema_field(&tok);
            }
            let element = self.parse_schema_type(&elem_tok);
            self.exit_schema_depth();
            if self.expect(TokenKind::Gt).is_none() {
                return self.fallback_schema_field(&tok);
            }
            return self.add_spanned(Node::SchemaArray { element }, &tok);
        }
        // Enum("a", "b"): positional strings, at least one.
        if type_name == "Enum" {
            self.bump();
            if self.expect(TokenKind::LParen).is_none() {
                self.error_at("E020", "Enum needs a value list: Enum(\"a\", ...)", &tok);
                return self.fallback_schema_field(&tok);
            }
            let mut values = Vec::new();
            let mut deprecated: Option<String> = None;
            loop {
                self.skip_newlines();
                match self.peek_kind() {
                    TokenKind::RParen | TokenKind::Eof => break,
                    TokenKind::Comma => {
                        self.bump();
                        continue;
                    }
                    TokenKind::String(_) | TokenKind::SingleString => {
                        let v = self.bump();
                        values.push(Self::render_marked(
                            &v.text,
                            &Self::sentinel_marks(&v.text, &v.kind),
                        ));
                    }
                    TokenKind::Ident if self.peek().text == "deprecated" => {
                        // Trailing `deprecated = true | "msg"` inside
                        // Enum parens (spec/linter.md §3): every leaf
                        // type accepts it, including Enum.
                        self.bump();
                        if self.expect(TokenKind::Assign).is_none() {
                            continue;
                        }
                        deprecated = match self.peek_kind() {
                            TokenKind::True => {
                                self.bump();
                                Some(String::new())
                            }
                            TokenKind::String(_) | TokenKind::SingleString => {
                                let v = self.bump();
                                Some(Self::render_marked(
                                    &v.text,
                                    &Self::sentinel_marks(&v.text, &v.kind),
                                ))
                            }
                            _ => {
                                let bad = self.peek().clone();
                                self.error_at(
                                    "E020",
                                    "deprecated takes `true` or a string message",
                                    &bad,
                                );
                                None
                            }
                        };
                    }
                    _ => {
                        let bad = self.peek().clone();
                        self.error_at("E020", "Enum values must be string literals", &bad);
                        self.bump();
                    }
                }
                self.skip_newlines();
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                }
            }
            self.expect(TokenKind::RParen);
            if values.is_empty() {
                self.error_at("E020", "Enum needs at least one value", &tok);
            }
            let field = SchemaField {
                typ: ActiveType::Enum(values),
                min: None,
                max: None,
                min_len: None,
                max_len: None,
                pattern: None,
                default: None,
                deprecated,
                span: Some(span_of(&tok)),
            };
            return self.add_spanned(Node::SchemaField { field }, &tok);
        }
        // Table { ... }: nested schema table (closed flag included).
        if type_name == "Table" && matches!(self.peek_kind_at(1), TokenKind::LBrace) {
            self.bump();
            let open = self
                .expect(TokenKind::LBrace)
                .unwrap_or_else(|| tok.clone());
            if self.enter_schema_depth(&tok) {
                return self.fallback_schema_field(&tok);
            }
            let (fields, closed) = self.parse_schema_entries(SchemaEnd::Brace);
            self.exit_schema_depth();
            return self.add_spanned(Node::SchemaTable { fields, closed }, &open);
        }
        // Bare or call-form type.
        let Some(typ) = ActiveType::bare(&type_name) else {
            self.error_at("E020", format!("unknown schema type `{type_name}`"), &tok);
            self.bump();
            return self.fallback_schema_field(&tok);
        };
        self.bump();
        if !matches!(self.peek_kind(), TokenKind::LParen) {
            let field = SchemaField::bare(typ, Some(span_of(&tok)));
            return self.add_spanned(Node::SchemaField { field }, &tok);
        }
        // `Table` takes no call arguments (spec §2.10): nested schemas
        // use `Table { … }`, handled above. Anything parenthesized here
        // is E020; the balanced list is skipped so recovery stays clean.
        if matches!(typ, ActiveType::Table { .. }) {
            self.error_at(
                "E020",
                "Table takes no call arguments; write Table { ... } for a nested schema",
                &tok,
            );
            self.bump(); // consume `(`
            let mut paren_depth = 1usize;
            while paren_depth > 0 {
                match self.peek_kind() {
                    TokenKind::LParen => {
                        self.bump();
                        paren_depth += 1;
                    }
                    TokenKind::RParen => {
                        self.bump();
                        paren_depth -= 1;
                    }
                    TokenKind::Eof => break,
                    _ => {
                        self.bump();
                    }
                }
            }
            return self.fallback_schema_field(&tok);
        }
        self.bump(); // consume `(`
        let mut field = SchemaField::bare(typ.clone(), Some(span_of(&tok)));
        loop {
            self.skip_newlines();
            match self.peek_kind() {
                TokenKind::RParen | TokenKind::Eof => break,
                TokenKind::Comma => {
                    self.bump();
                    continue;
                }
                TokenKind::Ident => {
                    let arg_tok = self.bump();
                    let arg = arg_tok.text.clone();
                    if self.expect(TokenKind::Assign).is_none() {
                        continue;
                    }
                    // `deprecated` is static text on every leaf type
                    // (spec/linter.md §3, C-042): `true` or a string
                    // literal migration message — never an expression.
                    if arg == "deprecated" {
                        let value = self.parse_expression();
                        // First wins, like every other argument.
                        if field.deprecated.is_none() {
                            match self.module.get(value) {
                                Some(Node::Literal(Value::Bool(true))) => {
                                    field.deprecated = Some(String::new());
                                }
                                Some(Node::Literal(Value::String(msg))) => {
                                    field.deprecated = Some(msg.clone());
                                }
                                _ => {
                                    self.error_at(
                                        "E020",
                                        "deprecated takes `true` or a string message",
                                        &arg_tok,
                                    );
                                }
                            }
                        }
                        continue;
                    }
                    let value = self.parse_expression();
                    if !Self::store_schema_arg(&mut field, &typ, &arg, value) {
                        self.error_at(
                            "E020",
                            format!("unknown argument `{arg}` for {}", typ.name()),
                            &arg_tok,
                        );
                    }
                }
                _ => {
                    let bad = self.peek().clone();
                    self.error_at("E020", "expected a named argument (`name = value`)", &bad);
                    self.bump();
                }
            }
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            }
        }
        self.expect(TokenKind::RParen);
        self.add_spanned(Node::SchemaField { field }, &tok)
    }

    /// Placeholder field used for error recovery (E020 already recorded).
    fn fallback_schema_field(&mut self, tok: &Token) -> NodeId {
        let field = SchemaField::bare(ActiveType::Any, Some(span_of(tok)));
        self.add_spanned(Node::SchemaField { field }, tok)
    }

    /// Store one named constraint argument if the type allows it.
    /// Returns false for unknown arguments (E020 by the caller).
    /// A repeated argument keeps the first value.
    fn store_schema_arg(
        field: &mut SchemaField,
        typ: &ActiveType,
        arg: &str,
        value: NodeId,
    ) -> bool {
        use ActiveType::*;
        let slot = match (typ, arg) {
            (Int | Float | Duration | Bytes, "min") => &mut field.min,
            (Int | Float | Duration | Bytes, "max") => &mut field.max,
            (String, "min_len") => &mut field.min_len,
            (String, "max_len") => &mut field.max_len,
            (String, "pattern") => &mut field.pattern,
            (_, "default") => &mut field.default,
            _ => return false,
        };
        if slot.is_none() {
            *slot = Some(value);
        }
        true
    }
    /// Sentinel-mark helpers (spec/grammar.md §1.4 string fidelity).
    ///
    /// The lexer pushes SENTINEL_POS for characters decoded from a `\$` escape
    /// and real positions otherwise. A genuine U+E000 in source keeps its real
    /// position, so the parser can tell `\$` (marked) from user data (unmarked):
    /// marked sentinels render as `$`, every other char — including a literal
    /// U+E000 — is kept verbatim. Kinds without a position vector can never
    /// carry a sentinel.
    /// Per-character marks parallel to `text.chars()`: true where the char is a
    /// `\$`-sentinel.
    fn sentinel_marks(text: &str, kind: &TokenKind) -> Vec<bool> {
        let pos = match kind {
            TokenKind::String(p) | TokenKind::MlString(p) => p,
            _ => return vec![false; text.chars().count()],
        };
        text.chars()
            .enumerate()
            .map(|(i, c)| c == ESCAPED_DOLLAR && pos.get(i) == Some(&SENTINEL_POS))
            .collect()
    }

    /// Render string text: marked sentinels become `$`; everything else stays.
    fn render_marked(text: &str, marks: &[bool]) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, c) in text.chars().enumerate() {
            if c == ESCAPED_DOLLAR && marks.get(i) == Some(&true) {
                out.push('$');
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Render one already-split char.
    fn render_char(c: char, marked: bool) -> char {
        if c == ESCAPED_DOLLAR && marked {
            '$'
        } else {
            c
        }
    }

    /// True if the text holds at least one `\$`-sentinel.
    fn has_marked(text: &str, marks: &[bool]) -> bool {
        text.chars()
            .enumerate()
            .any(|(i, c)| c == ESCAPED_DOLLAR && marks.get(i) == Some(&true))
    }

    fn parse_string_literal(
        &mut self,
        raw: &str,
        tok: &Token,
        linemap: Option<&[MlLineMap]>,
        marks: &[bool],
    ) -> NodeId {
        // The lexer marks `\$` with a sentinel; convert only marked chars so
        // a literal U+E000 in user data survives verbatim.
        if !raw.contains("${") && !Self::has_marked(raw, marks) {
            return self.add_spanned(Node::Literal(Value::String(raw.to_string())), tok);
        }

        let chars: Vec<char> = raw.chars().collect();
        let marked = |i: usize| -> bool { marks.get(i) == Some(&true) };
        let mut parts: Vec<InterpPart> = Vec::new();
        let mut literal = String::new();
        let mut i = 0usize;

        while i < chars.len() {
            // An escaped dollar is literal text, never interpolation — and
            // because the lexer marked it, a real `\` before `${` stays a
            // backslash (parity is preserved).
            if chars[i] == ESCAPED_DOLLAR && marked(i) {
                literal.push('$');
                i += 1;
                continue;
            }
            if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
                match self.find_interpolation_end(&chars, i + 2) {
                    Some(end) => {
                        if !literal.is_empty() {
                            parts.push(InterpPart::Literal(std::mem::take(&mut literal)));
                        }
                        let inner: String = chars[i + 2..end].iter().collect();
                        // Offset of the expression's first character inside
                        // the token, for precise diagnostic spans.
                        let offset = i + 2;
                        let id = self.parse_embedded_expression(&inner, tok, raw, offset, linemap);
                        parts.push(InterpPart::Expr(id));
                        i = end + 1;
                        continue;
                    }
                    None => {
                        // Unterminated interpolation: diagnose and treat the
                        // remainder as literal text (sentinels still render).
                        let span = self.interp_span(tok, raw, i, linemap);
                        self.error_at(
                            "E012",
                            "unterminated interpolation: `${` has no matching `}`",
                            &span,
                        );
                        for (k, c) in chars[i..].iter().enumerate() {
                            literal.push(Self::render_char(*c, marked(i + k)));
                        }
                        break;
                    }
                }
            }
            literal.push(Self::render_char(chars[i], marked(i)));
            i += 1;
        }
        if !literal.is_empty() {
            parts.push(InterpPart::Literal(literal));
        }

        self.add_spanned(Node::Interpolate { parts }, tok)
    }

    /// Find the `}` matching a `${` that starts at `start`, skipping string
    /// literals, nested interpolations, and plain brace nesting (an inline
    /// table inside `${ … }` is an expression with its own braces).
    fn find_interpolation_end(&self, chars: &[char], start: usize) -> Option<usize> {
        let mut i = start;
        let mut depth = 0usize;
        let mut in_string: Option<char> = None;
        let mut escaped = false;
        while i < chars.len() {
            let c = chars[i];
            if let Some(quote) = in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == quote {
                    in_string = None;
                }
                i += 1;
                continue;
            }
            match c {
                '"' | '\'' => in_string = Some(c),
                '$' if i + 1 < chars.len() && chars[i + 1] == '{' => {
                    depth += 1;
                    i += 2;
                    continue;
                }
                '{' => depth += 1,
                '}' => {
                    if depth == 0 {
                        return Some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// A span pointing inside the string token at character offset `offset`
    /// (offset 0 is the first character after the opening quote; the lexer's
    /// token text is the decoded content, so offsets are approximate when the
    /// source contained escapes). With a line map (multi-line strings),
    /// `stripped` locates the offset's stripped line and the map converts
    /// it to the true source position.
    fn interp_span(
        &self,
        tok: &Token,
        stripped: &str,
        offset: usize,
        linemap: Option<&[MlLineMap]>,
    ) -> Token {
        let mut t = tok.clone();
        match linemap {
            Some(map) => {
                // Walk the stripped text to the offset's stripped line and
                // column, then map through the per-character source columns.
                let mut line = 0usize;
                let mut col = 0usize;
                for c in stripped.chars().take(offset) {
                    if c == '\n' {
                        line += 1;
                        col = 0;
                    } else {
                        col += 1;
                    }
                }
                match map
                    .get(line)
                    .and_then(|m| m.src_cols.get(col).map(|&c| (m.src_line, c)))
                {
                    Some((src_line, src_col)) => {
                        t.line = src_line;
                        t.col = src_col;
                        t.end_line = src_line;
                        t.end_col = src_col + 1;
                    }
                    None => {
                        t.col = tok.col + 1 + offset as u32;
                        t.end_col = t.col + 1;
                    }
                }
            }
            None => {
                t.col = tok.col + 1 + offset as u32;
                t.end_col = t.col + 1;
            }
        }
        t
    }

    /// Parse the contents of `${ … }` as a full expression, splicing the
    /// resulting nodes into this module's arena. Supports nested
    /// interpolation, env defaults, arithmetic, and every other expression
    /// form; sub-expression diagnostics have their spans rebased onto the
    /// enclosing string's position (`stripped` is the scanned string and
    /// `offset` the expression's first character within it; `linemap`
    /// maps stripped lines to source positions for multi-line strings).
    fn parse_embedded_expression(
        &mut self,
        text: &str,
        tok: &Token,
        stripped: &str,
        offset: usize,
        linemap: Option<&[MlLineMap]>,
    ) -> NodeId {
        // Embedded expressions are not laid out: trim the padding so the
        // sub-lexer never measures indentation inside `${ … }`. Count
        // trimmed characters (not bytes) — `offset` is a char count used
        // by span rebasing.
        let leading_ws = text.chars().count() - text.trim_start().chars().count();
        let text = text.trim();
        let offset = offset + leading_ws;
        if text.is_empty() {
            let span = self.interp_span(tok, stripped, offset, linemap);
            self.error_at(
                "E013",
                "empty interpolation: `${}` has no expression",
                &span,
            );
            return self.add_spanned(Node::Literal(Value::Null), tok);
        }
        let mut lexer = Lexer::new(text);
        let lexed = lexer.tokenize();

        // Anchor of this expression within the stripped string: the
        // stripped line/column where the expression's first character
        // sits. Sub-lexer spans are expression-local; adding the anchor
        // lands them in the stripped frame, and the line map converts
        // stripped coordinates to source positions exactly once — for
        // direct spans and (recursively) for nested spans alike, because
        // nested calls first land their spans in *their* parent
        // expression's frame, which this same step then lifts.
        let (anchor_line, anchor_col) = if linemap.is_some() {
            let mut al = 0u32;
            let mut ac = 0u32;
            for c in stripped.chars().take(offset) {
                if c == '\n' {
                    al += 1;
                    ac = 0;
                } else {
                    ac += 1;
                }
            }
            (al, ac)
        } else {
            (0, 0)
        };

        // Move the arena into a sub-parser so embedded nodes land in the
        // same module.
        let module = std::mem::replace(&mut self.module, Module::new(""));
        // Length of the inherited scope: only identifiers created *by this
        // expression* are transferred back, so repeated interpolations cannot
        // duplicate the enclosing scope.
        let inherited_len = self.ident_scopes.last().map_or(0, |s| s.len());
        let mut sub = Parser {
            tokens: lexed.tokens,
            pos: 0,
            module,
            errors: Vec::new(),
            ident_scopes: self.ident_scopes.clone(),
            depth: self.depth,
            options: self.options.clone(),
            block_depth: 0,
            schema_depth: self.schema_depth,
            comments: Vec::new(),
            comment_files: Vec::new(),
            statements: Vec::new(),
            blocks: Vec::new(),
            current_path: self.current_path.clone(),
            brace_tables: 0,
            indent_tables: 0,
            current_file: self.current_file.clone(),
            include_stack: Vec::new(),
            in_include: self.in_include,
            in_profile: self.in_profile,
            array_depth: self.array_depth,
            binding_spliced: self.binding_spliced.clone(),
            pending_profiles: Vec::new(),
            trust: self.trust.clone(),
        };
        // Arena ids below this base predate the embedded expression (the
        // sub-parser shares the outer arena); only spans at or above it
        // may be rebased.
        let node_base = sub.module.arena.len() as u32;
        let id = sub.parse_expression();
        // Anything left over that is not a newline is unconsumed input.
        while matches!(sub.peek_kind(), TokenKind::Newline) {
            sub.bump();
        }
        if !matches!(sub.peek_kind(), TokenKind::Eof) {
            let leftover = sub.peek().clone();
            sub.error_at(
                "E002",
                format!(
                    "unexpected {:?} after the interpolated expression",
                    leftover.kind
                ),
                &leftover,
            );
        }

        let new_idents: Vec<(String, NodeId)> = sub
            .ident_scopes
            .last()
            .map(|s| s[inherited_len.min(s.len())..].to_vec())
            .unwrap_or_default();
        let mut sub_errors = std::mem::take(&mut sub.errors);
        let mut sub_repairs = std::mem::take(&mut sub.module.healing);
        let mut sub_lex_errors = lexed.errors;
        let mut sub_lex_repairs = lexed.repairs;
        // Embedded nodes carry sub-parser-relative spans in node_spans;
        // rebase only the spans created by this embedded expression
        // (arena ids at or above node_base) so evaluation-time errors
        // point correctly without touching outer nodes.
        // Must run before the module swap below. With a line map the
        // tokens were pre-shifted into the stripped frame, so the map
        // lookup suffices; without one, the same tok+offset shift as
        // diagnostics applies (this also lands nested spans in the
        // stripped frame for the outer map pass).
        // Lift an expression-local span into source coordinates. With a
        // line map, the anchor lands the span in the stripped frame and
        // the map converts stripped coordinates to source positions;
        // nested spans compose because each level's map targets its own
        // parent frame. Without a map, the tok+offset shift lands it in
        // source coordinates directly.
        let lift = |line_1: u32, col_1: u32| -> Option<(u32, u32)> {
            match linemap {
                Some(map) => {
                    let sl = anchor_line as usize + (line_1 as usize).saturating_sub(1);
                    let sc = if line_1 <= 1 {
                        anchor_col as usize + (col_1 as usize).saturating_sub(1)
                    } else {
                        (col_1 as usize).saturating_sub(1)
                    };
                    let m = map.get(sl)?;
                    let col = m.src_cols.get(sc).copied()?;
                    Some((m.src_line, col))
                }
                None => {
                    let line = tok.line + line_1.saturating_sub(1);
                    let col = if line_1 <= 1 {
                        tok.col + offset as u32 + col_1.saturating_sub(1)
                    } else {
                        col_1
                    };
                    Some((line, col))
                }
            }
        };
        for (node, span) in sub.module.node_spans.iter_mut() {
            if node.0 >= node_base {
                if let Some((line, col)) = lift(span.line, span.column) {
                    span.line = line;
                    span.column = col;
                }
                if let Some((line, col)) = lift(span.end_line, span.end_column) {
                    span.end_line = line;
                    span.end_column = col;
                }
            }
        }
        self.module = std::mem::replace(&mut sub.module, Module::new(""));

        if let Some(scope) = self.ident_scopes.last_mut() {
            scope.extend(new_idents);
        }
        // Rebase embedded diagnostics onto the string's source position
        // through the same lift: expression-local → stripped → source.
        let rebase = |d: &mut Diagnostic| {
            if let Some(s) = &mut d.span {
                if let Some((line, col)) = lift(s.line, s.column) {
                    s.line = line;
                    s.column = col;
                }
                if let Some((line, col)) = lift(s.end_line, s.end_column) {
                    s.end_line = line;
                    s.end_column = col;
                }
            }
        };
        for d in sub_lex_errors
            .iter_mut()
            .chain(sub_lex_repairs.iter_mut())
            .chain(sub_errors.iter_mut())
            .chain(sub_repairs.iter_mut())
        {
            rebase(d);
        }
        // Sub-lexer diagnostics belong to the same file as the enclosing
        // string (current_file already tracks include nesting).
        let sub_file = self.current_file_name();
        for d in sub_lex_errors.iter_mut().chain(sub_lex_repairs.iter_mut()) {
            if d.file.is_none() {
                d.file = sub_file.clone();
            }
        }
        self.errors.extend(sub_lex_errors);
        self.errors.extend(sub_errors);
        self.module.healing.extend(sub_lex_repairs);
        self.module.healing.extend(sub_repairs);
        id
    }

    fn parse_array(&mut self) -> NodeId {
        let open = self.peek().clone();
        self.bump(); // [
                     // Schema blocks are banned inside array elements (spec §2.10):
                     // the counter covers inline tables at any depth within.
        self.array_depth += 1;
        let mut elements = Vec::new();
        loop {
            self.skip_newlines();
            // RBrace is a mismatched closer: stop so the closing-delimiter
            // healer (H004) can see it instead of parse_primary eating it.
            if matches!(
                self.peek_kind(),
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
            ) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                // Stray or repeated comma: heal as a removal.
                let comma = self.bump();
                if !self.heal(
                    "H002",
                    "removed stray comma".into(),
                    &comma,
                    Repair {
                        original: ",".into(),
                        suggested: String::new(),
                        kind: RepairKind::RemoveTrailingComma,
                        safe: true,
                    },
                    thresholds::AUTO_APPLY + 0.03,
                ) {
                    self.error("E002", "unexpected comma");
                }
                continue;
            }
            elements.push(self.parse_expression());
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                self.skip_newlines();
                if matches!(
                    self.peek_kind(),
                    TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
                ) {
                    self.heal(
                        "H002",
                        "removed trailing comma before closing bracket".into(),
                        &comma,
                        Repair {
                            original: ",".into(),
                            suggested: String::new(),
                            kind: RepairKind::RemoveTrailingComma,
                            safe: true,
                        },
                        thresholds::AUTO_APPLY + 0.03,
                    );
                }
            } else if !matches!(
                self.peek_kind(),
                TokenKind::RBracket | TokenKind::RBrace | TokenKind::Eof
            ) {
                // Missing comma between elements.
                let tok = self.peek().clone();
                if !self.heal(
                    "H003",
                    "inserted missing comma between array elements".into(),
                    &tok,
                    Repair {
                        original: String::new(),
                        suggested: ",".into(),
                        kind: RepairKind::Other("insert_comma".into()),
                        safe: false,
                    },
                    thresholds::HIGH + 0.05, // 0.90 — auto-apply with warning
                ) {
                    self.error(
                        "E001",
                        format!("expected `,` between array elements, found {:?}", tok.kind),
                    );
                }
            }
        }
        self.expect_closing_healing(TokenKind::RBracket, "H004");
        self.array_depth = self.array_depth.saturating_sub(1);
        self.add_spanned(Node::Array { elements }, &open)
    }

    fn parse_inline_table(&mut self) -> NodeId {
        let open = self.peek().clone();
        self.bump(); // {
        self.ident_scopes.push(Vec::new());
        let mut fields = indexmap::IndexMap::new();
        loop {
            self.skip_newlines();
            if matches!(
                self.peek_kind(),
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
            ) {
                break;
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                if !self.heal(
                    "H002",
                    "removed stray comma".into(),
                    &comma,
                    Repair {
                        original: ",".into(),
                        suggested: String::new(),
                        kind: RepairKind::RemoveTrailingComma,
                        safe: true,
                    },
                    thresholds::AUTO_APPLY + 0.03,
                ) {
                    self.error("E002", "unexpected comma");
                }
                continue;
            }
            if let Some((k, id)) = self.parse_key_value() {
                let span = self.module.node_spans.get(&id).cloned();
                self.note_duplicate_key(&k, span, fields.contains_key(&k));
                fields.insert(k, id);
            } else {
                break;
            }
            self.skip_newlines();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                let comma = self.bump();
                self.skip_newlines();
                if matches!(
                    self.peek_kind(),
                    TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
                ) {
                    self.heal(
                        "H002",
                        "removed trailing comma before closing brace".into(),
                        &comma,
                        Repair {
                            original: ",".into(),
                            suggested: String::new(),
                            kind: RepairKind::RemoveTrailingComma,
                            safe: true,
                        },
                        thresholds::AUTO_APPLY + 0.03,
                    );
                }
            } else if !matches!(
                self.peek_kind(),
                TokenKind::RBrace | TokenKind::RBracket | TokenKind::Eof
            ) {
                let tok = self.peek().clone();
                if !self.heal(
                    "H003",
                    "inserted missing comma between table entries".into(),
                    &tok,
                    Repair {
                        original: String::new(),
                        suggested: ",".into(),
                        kind: RepairKind::Other("insert_comma".into()),
                        safe: false,
                    },
                    thresholds::HIGH + 0.05, // 0.90 — auto-apply with warning
                ) {
                    self.error(
                        "E001",
                        format!("expected `,` between table entries, found {:?}", tok.kind),
                    );
                }
            }
        }
        self.expect_closing_healing(TokenKind::RBrace, "H004");
        let idents = self.ident_scopes.pop().unwrap_or_default();
        let _ = self.resolve_sibling_refs(idents, &fields);
        self.add_spanned(Node::Table { fields }, &open)
    }

    // ------------------------------------------------------------------
    // Dependency-graph construction & Ident → Ref resolution
    // ------------------------------------------------------------------

    fn build_graph(&mut self) {
        let bindings = self.module.bindings.clone();
        // Declared dependencies become edges (spec/linter.md §2):
        // unresolvable paths are skipped here and reported as L002 by
        // the linter; declared cycles surface as S001 like all cycles.
        // Cloned up front: resolution borrows the module.
        let declared: Vec<(NodeId, Vec<String>)> = self
            .module
            .depends_on
            .iter()
            .map(|(id, dep)| (*id, dep.paths.clone()))
            .collect();
        for (scope, paths) in &declared {
            for path in paths {
                if let Some(target) = self.module.resolve_value_path(path) {
                    self.module.add_edge(target, *scope);
                }
            }
        }

        for i in 0..self.module.arena.len() {
            let id = NodeId(i as u32);
            let node = self.module.get(id).cloned();
            match node {
                Some(Node::Ident(name)) => {
                    if let Some(&target) = bindings.get(&name) {
                        if let Some(slot) = self.module.get_mut(id) {
                            *slot = Node::Ref(target);
                        }
                        // edge: dependency → dependent
                        self.module.add_edge(target, id);
                    }
                }
                Some(Node::Env {
                    default: Some(def), ..
                }) => {
                    self.module.add_edge(def, id);
                }
                Some(Node::Interpolate { parts }) => {
                    for part in &parts {
                        if let InterpPart::Expr(expr_id) = part {
                            self.module.add_edge(*expr_id, id);
                        }
                    }
                }
                Some(Node::Binary { lhs, rhs, .. }) => {
                    self.module.add_edge(lhs, id);
                    self.module.add_edge(rhs, id);
                }
                Some(Node::Unary { expr, .. }) => {
                    self.module.add_edge(expr, id);
                }
                Some(Node::Quantity { value, .. }) => {
                    self.module.add_edge(value, id);
                }
                Some(Node::Semantic { inner, .. }) => {
                    self.module.add_edge(inner, id);
                }
                Some(Node::SchemaField { field }) => {
                    // Constraint arguments evaluate with the document;
                    // C-031 reads their values during validation.
                    for arg in [
                        field.min,
                        field.max,
                        field.min_len,
                        field.max_len,
                        field.pattern,
                        field.default,
                    ]
                    .into_iter()
                    .flatten()
                    {
                        self.module.add_edge(arg, id);
                    }
                }
                Some(Node::SchemaTable { fields, .. }) => {
                    for child in fields.values() {
                        self.module.add_edge(*child, id);
                    }
                }
                Some(Node::SchemaArray { element }) => {
                    self.module.add_edge(element, id);
                }
                Some(Node::Array { elements }) => {
                    for e in &elements {
                        self.module.add_edge(*e, id);
                    }
                }
                Some(Node::Table { fields }) => {
                    for (_, fid) in &fields {
                        self.module.add_edge(*fid, id);
                    }
                }
                Some(Node::Secret { locator }) => {
                    self.module.add_edge(locator, id);
                }
                _ => {}
            }
        }
    }
}
