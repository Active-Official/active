# AI agent prompts

Prompts are configuration too — versioned, reviewed, and validated
like any other value. Semantic blocks tag each string by role so
tooling (and reviewers) treat system instructions, safety policy,
and output templates differently, while evaluation stays plain
strings with full interpolation:

```active
{{#include ./ai-prompts.active}}
```

`active-lsp` hover shows the tag; semantic blocks reparse to the same
values through `active fmt`; `active convert --to json` exports the
evaluated strings for the model call. Secrets (API keys beside
prompts) stay `secret()` and redact on every export path.
