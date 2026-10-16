# Documentation Generator (C-048, P-044)

Schema + `@intent` + comments → Markdown reference pages, a single
searchable HTML page, and man(7) pages. Secret values render
redacted in every format (C-036).

## 1. Model

One `Item` per dotted value path (depth-first, source order —
`IndexMap` iteration, deterministic), each with: key, depth, type
text (schema type or value kind), required flag, default text,
rendered value, description (intent paragraphs plus attached
comments), constraint phrases, and deprecation notes. `let`
bindings are not documented (implementation, not configuration).

- Types/required/defaults follow the generator rules
  (spec/converters.md §7): schema first, value kind as fallback.
- Descriptions: `@intent` texts in order, then comments whose
  trivia attachment names the item's own dotted path
  (`Leading`/`Trailing` — the formatter's normative attachment,
  spec/dual-syntax.md §7, so docs and formatting never disagree
  on what documents what). Markers stripped, source order kept.
- Constraints: `min/max`, `min_len/max_len`, `pattern` from static
  literals only; dynamic bounds are skipped silently (validation
  still enforces them).
- Values render like hover values (scalars inline, composites by
  kind); secrets render `***SECRET***` only.

## 2. Formats

- **Markdown**: `# <Title>`, one `## <table>` section per
  top-level table (scalars first under `## General`), field rows
  as `### <path>` subsections with a `Type / Required / Default /
  Value` line, description paragraphs, constraint bullets, and
  `> Deprecated: …` quotes. Pipes escaped in prose.
- **HTML**: one self-contained page (inline CSS + ~10 lines of
  filter JS): a search box filters `.field` blocks by text, plus
  the Markdown content as headings/paragraphs/lists. All content
  HTML-escaped.
- **Man**: `.TH "<TITLE>" 5`, `.SH NAME` (one-line summary),
  `.SH FIELDS` with one `.TP` entry per item (path as tag,
  type/required/value/description as body). Groff escapes applied
  throughout (`\` doubled; `\&` prefixed to lines starting with
  `'` or `.`).

## 3. CLI

`solid doc --from <file.solid> --to markdown|html|man [--title
<Title>]` (title defaults to the file stem): parses, evaluates
(resilient library default), prints to stdout. Evaluation failure
exits non-zero.

## 4. Testing contract

- Golden probes per format over a fixture with nesting, intents,
  comments, constraints, defaults, deprecations, and a secret.
- Comment attachment (leading/trailing/doc-comment) unit tests.
- Redaction audit: secret text in no output.
- No-schema documents still generate (values + kinds).
- CLI tests for all three formats incl. exit codes.
