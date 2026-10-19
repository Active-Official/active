//! Import: JSON / YAML / TOML into Active source (C-046,
//! spec/converters.md §4). Best-effort by design: every lossy corner
//! carries a diagnostic, and the emitted source always parses.
//! Redacted `"***SECRET***"` markers import as plain strings —
//! exports are one-way, never resurrected into live secrets.

use crate::diag::{Diagnostic, Severity};
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// Active source plus import notes for JSON text. Syntax failures are
/// hard errors (the source format itself is broken).
pub fn json_text_to_active(text: &str) -> Result<(String, Vec<Diagnostic>), String> {
    let value: JsonValue =
        serde_json::from_str(text).map_err(|e| hard_error("invalid JSON", &e.to_string()))?;
    Ok(json_value_to_active(&value))
}

/// Active source plus import notes for YAML text.
pub fn yaml_text_to_active(text: &str) -> Result<(String, Vec<Diagnostic>), String> {
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| hard_error("invalid YAML", &e.to_string()))?;
    let mut emitter = Emitter::new();
    let value = emitter.yaml_to_json(&yaml, "");
    let (source, mut diags) = emitter.finish(&value);
    diags.sort_by(|a, b| a.code.cmp(&b.code));
    Ok((source, diags))
}

/// Active source plus import notes for TOML text.
pub fn toml_text_to_active(text: &str) -> Result<(String, Vec<Diagnostic>), String> {
    let toml: toml::Value =
        toml::from_str(text).map_err(|e| hard_error("invalid TOML", &e.to_string()))?;
    let mut emitter = Emitter::new();
    let value = emitter.toml_to_json(&toml, "");
    let (source, mut diags) = emitter.finish(&value);
    diags.sort_by(|a, b| a.code.cmp(&b.code));
    Ok((source, diags))
}

/// Hard import failure without source excerpts: format parsers echo
/// the offending line, which may contain a secret — keep the reason
/// and location (first line), withhold the rest.
fn hard_error(prefix: &str, detail: &str) -> String {
    let first = detail.lines().next().unwrap_or("").trim();
    format!("{prefix}: {first} (source excerpt withheld)")
}

/// Active source plus import notes for an already-parsed JSON value.
/// Objects become nested brace blocks; anything else wraps as
/// `value = …` with a W009 note. Always parses; lossy corners are
/// diagnosed, never silent.
pub fn json_value_to_active(v: &JsonValue) -> (String, Vec<Diagnostic>) {
    Emitter::new().finish(v)
}

/// Shared emission state: import notes in document order.
struct Emitter {
    diags: Vec<Diagnostic>,
}

impl Emitter {
    fn new() -> Self {
        Self { diags: Vec::new() }
    }

    fn finish(mut self, v: &JsonValue) -> (String, Vec<Diagnostic>) {
        let mut out = String::new();
        match v {
            JsonValue::Object(map) => {
                self.emit_block(map, &mut out, 0, "");
            }
            other => {
                self.note(
                    "W009",
                    format!("top-level {} wrapped as 'value'", Self::kind_name(other)),
                );
                let inline = self.inline(other);
                out.push_str(&format!("value = {inline}\n"));
            }
        }
        (out, self.diags)
    }

    fn kind_name(v: &JsonValue) -> &'static str {
        match v {
            JsonValue::Null => "null",
            JsonValue::Bool(_) => "boolean",
            JsonValue::Number(_) => "number",
            JsonValue::String(_) => "string",
            JsonValue::Array(_) => "array",
            JsonValue::Object(_) => "object",
        }
    }

    /// A document block: scalars/arrays as `key = …` lines, objects
    /// as nested `key { … }` blocks. `path` is the dotted scope for
    /// messages; duplicate sanitized keys are last-wins with a note.
    fn emit_block(
        &mut self,
        map: &serde_json::Map<String, JsonValue>,
        out: &mut String,
        indent: usize,
        path: &str,
    ) {
        let pad = "  ".repeat(indent);
        let mut seen = HashSet::new();
        for (key, val) in map {
            let (clean, renamed) = self.block_key(key, val);
            if renamed {
                self.note("W007", format!("imported key '{key}' renamed to '{clean}'"));
            }
            if !seen.insert(clean.clone()) {
                self.note(
                    "W007",
                    format!("duplicate key '{clean}' after sanitization; last wins"),
                );
            }
            let child = if path.is_empty() {
                clean.clone()
            } else {
                format!("{path}.{clean}")
            };
            match val {
                JsonValue::Object(nested) => {
                    out.push_str(&format!("{pad}{clean} {{\n"));
                    self.emit_block(nested, out, indent + 1, &child);
                    out.push_str(&format!("{pad}}}\n"));
                }
                _ => {
                    let inline = self.inline(val);
                    out.push_str(&format!("{pad}{clean} = {inline}\n"));
                }
            }
        }
    }

    /// Cleaned key plus whether it changed: identifier sanitizing
    /// first, then the contextual renames (`schema` objects and
    /// `depends_on` arrays would be consumed as language metadata).
    fn block_key(&self, key: &str, val: &JsonValue) -> (String, bool) {
        let clean = sanitize_key(key);
        if let Some(renamed) = contextual_rename(&clean, val) {
            return (renamed, true);
        }
        let changed = clean != key;
        (clean, changed)
    }

    /// Inline literal for any value. Objects nest inline as
    /// `{ k = v, … }` (comma-separated, matching the dual-syntax
    /// surface). Numbers prefer Int; u64 beyond i64::MAX falls back
    /// to Float with a W010 note (precision documented in
    /// spec/converters.md §4).
    fn inline(&mut self, val: &JsonValue) -> String {
        match val {
            JsonValue::Null => "null".to_string(),
            JsonValue::Bool(b) => b.to_string(),
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(u) = n.as_u64() {
                    self.note(
                        "W010",
                        format!("integer {u} exceeds i64 range; emitted as float"),
                    );
                    // Display, never Debug: Debug switches to
                    // scientific notation (unparseable as Active),
                    // Display prints full digits; `.0` forces the
                    // Float token (precision documented in
                    // spec/converters.md §4).
                    format!("{}.0", u as f64)
                } else {
                    // Display, never Debug: Debug switches to scientific
                    // notation (`5e22`) for large/small magnitudes, which
                    // Active cannot parse (grammar has no exponents) —
                    // violating the always-parses contract. Display always
                    // emits plain decimals and round-trips. A bare `300`
                    // would lex as Int, so integers-valued floats keep
                    // their `.0`; non-finite stays null (W010, §4).
                    n.as_f64()
                        .map(|f| {
                            if !f.is_finite() {
                                "null".to_string()
                            } else {
                                let s = format!("{f}");
                                if s.contains('.') {
                                    s
                                } else {
                                    format!("{s}.0")
                                }
                            }
                        })
                        .unwrap_or("null".to_string())
                }
            }
            JsonValue::String(s) => Self::quote(s),
            JsonValue::Array(items) => {
                let parts: Vec<String> = items.iter().map(|item| self.inline(item)).collect();
                format!("[{}]", parts.join(", "))
            }
            JsonValue::Object(map) => {
                let mut parts = Vec::with_capacity(map.len());
                let mut seen = HashSet::new();
                for (key, item) in map {
                    let clean = sanitize_key(key);
                    let clean = contextual_rename(&clean, item).unwrap_or(clean);
                    if clean != *key {
                        self.note("W007", format!("imported key '{key}' renamed to '{clean}'"));
                    }
                    if !seen.insert(clean.clone()) {
                        self.note(
                            "W007",
                            format!("duplicate key '{clean}' after sanitization; last wins"),
                        );
                    }
                    let inline = self.inline(item);
                    parts.push(format!("{clean} = {inline}"));
                }
                format!("{{ {} }}", parts.join(", "))
            }
        }
    }

    /// Double-quoted Active string with the language's escapes
    /// (`\n \t \r \\ \" \0 \$`; anything else verbatim).
    fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\0' => out.push_str("\\0"),
                '$' => out.push_str("\\$"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// Quote for schema-emitted strings (patterns, defaults, enum members):
    /// identical to `quote` except `$` escapes only before `{`. A bare `$`
    /// is already literal in Active strings, and blanket `\$` would corrupt
    /// regex anchors (`$`) and literal dollars in defaults (found by the
    /// L-021 probe: `^[0-9]+$` arrived as `^[0-9]+\$`).
    fn quote_schema(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\0' => out.push_str("\\0"),
                '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn note(&mut self, code: &str, message: String) {
        self.diags.push(Self::note_static(code, message));
    }

    fn note_static(code: &str, message: String) -> Diagnostic {
        Diagnostic {
            severity: Severity::Warning,
            code: code.into(),
            message,
            path: None,
            span: None,
            repair: None,
            confidence: 1.0,
            file: None,
        }
    }

    /// YAML → JSON model: non-string keys stringify with a W007 note
    /// each; timestamps already arrive as strings.
    fn yaml_to_json(&mut self, v: &serde_yaml::Value, path: &str) -> JsonValue {
        match v {
            serde_yaml::Value::Null => JsonValue::Null,
            serde_yaml::Value::Bool(b) => JsonValue::Bool(*b),
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    JsonValue::from(i)
                } else if let Some(u) = n.as_u64() {
                    JsonValue::from(u)
                } else {
                    match n.as_f64() {
                        Some(f) if f.is_finite() => JsonValue::from(f),
                        // serde_json maps NaN/inf to null silently;
                        // announce the loss instead.
                        _ => {
                            self.note(
                                "W010",
                                format!(
                                    "non-finite YAML number at '{}' converted to null",
                                    display_path(path)
                                ),
                            );
                            JsonValue::Null
                        }
                    }
                }
            }
            serde_yaml::Value::String(s) => JsonValue::String(s.clone()),
            serde_yaml::Value::Sequence(items) => JsonValue::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| self.yaml_to_json(item, &format!("{path}[{i}]")))
                    .collect(),
            ),
            serde_yaml::Value::Mapping(map) => {
                let mut out = serde_json::Map::new();
                for (key, val) in map {
                    let key_str = match key {
                        serde_yaml::Value::String(s) => s.clone(),
                        serde_yaml::Value::Null => {
                            self.note("W007", "YAML null key stringified to 'null'".to_string());
                            "null".to_string()
                        }
                        other => {
                            let rendered = match other {
                                serde_yaml::Value::Bool(b) => b.to_string(),
                                serde_yaml::Value::Number(n) => n.to_string(),
                                _ => "key".to_string(),
                            };
                            self.note(
                                "W007",
                                format!("YAML non-string key stringified to '{rendered}'"),
                            );
                            rendered
                        }
                    };
                    let child = if path.is_empty() {
                        key_str.clone()
                    } else {
                        format!("{path}.{key_str}")
                    };
                    out.insert(key_str, self.yaml_to_json(val, &child));
                }
                JsonValue::Object(out)
            }
            serde_yaml::Value::Tagged(tagged) => self.yaml_to_json(&tagged.value, path),
        }
    }

    /// TOML → JSON model: datetimes stringify with a W008 note each
    /// (no Active equivalent).
    fn toml_to_json(&mut self, v: &toml::Value, path: &str) -> JsonValue {
        match v {
            toml::Value::String(s) => JsonValue::String(s.clone()),
            toml::Value::Integer(i) => JsonValue::from(*i),
            toml::Value::Float(f) => {
                if f.is_finite() {
                    JsonValue::from(*f)
                } else {
                    self.note(
                        "W010",
                        format!(
                            "non-finite TOML number at '{}' converted to null",
                            display_path(path)
                        ),
                    );
                    JsonValue::Null
                }
            }
            toml::Value::Boolean(b) => JsonValue::Bool(*b),
            toml::Value::Datetime(dt) => {
                self.note(
                    "W008",
                    format!(
                        "TOML datetime at '{}' imported as string",
                        display_path(path)
                    ),
                );
                JsonValue::String(dt.to_string())
            }
            toml::Value::Array(items) => JsonValue::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| self.toml_to_json(item, &format!("{path}[{i}]")))
                    .collect(),
            ),
            toml::Value::Table(map) => {
                let mut out = serde_json::Map::new();
                for (key, val) in map {
                    let child = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    out.insert(key.clone(), self.toml_to_json(val, &child));
                }
                JsonValue::Object(out)
            }
        }
    }
}

/// Identifier-sanitize an imported key: `[A-Za-z0-9_]` kept (leading
/// digit prefixed), anything else becomes `_`; empty becomes `key`;
/// hard keyword spellings (which never parse as keys) gain a
/// trailing underscore. Contextual words (`include`, `profile`,
/// …) are left alone here — they parse as ordinary data in every
/// shape except the two `contextual_rename` covers. Returns the
/// clean name (equality with the input means untouched).
fn sanitize_key(key: &str) -> String {
    const RESERVED: &[&str] = &["let", "env", "secret", "true", "false", "null"];
    let mut clean = String::with_capacity(key.len());
    for c in key.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            clean.push(c);
        } else {
            clean.push('_');
        }
    }
    if clean.is_empty() {
        return "key".to_string();
    }
    if clean.starts_with(|c: char| c.is_ascii_digit()) {
        clean.insert(0, '_');
    }
    if RESERVED.contains(&clean.as_str()) {
        clean.push('_');
    }
    clean
}

/// Contextual renames, applied after `sanitize_key` wherever a value
/// is emitted: a `schema` object would be consumed as a schema
/// declaration (E020) and a `depends_on` array as dependency
/// metadata (vanishing from the value) — each gains a suffix exactly
/// in those shapes. Every other shape under those names parses as
/// ordinary data (probed: `include`, `as`, `private`, `profile`,
/// scalars, and non-array `depends_on` need no renaming).
fn contextual_rename(clean: &str, val: &JsonValue) -> Option<String> {
    match (clean, val) {
        ("schema", JsonValue::Object(_)) => Some("schema_".to_string()),
        ("depends_on", JsonValue::Array(_)) => Some("depends_on_".to_string()),
        _ => None,
    }
}

fn display_path(path: &str) -> &str {
    if path.is_empty() {
        "(root)"
    } else {
        path
    }
}

// ------------------------------------------------------------------
// JSON Schema to Active schema blocks (L-021, spec/converters.md §4).
// Best-effort onboarding: every unmappable construct carries a W012
// diagnostic, and the emitted source always parses. Optional properties
// without defaults tighten to required (loud, never silent); combinators,
// references, and conditionals have no Active counterpart and are skipped
// with notes. `true` accepts everything; `false` has no counterpart and
// is refused loudly.
// ------------------------------------------------------------------

/// Active source (a `schema` block) plus import notes for a JSON Schema
/// document. Syntax failures are hard errors; semantic gaps are W012.
pub fn jsonschema_to_active(text: &str) -> Result<(String, Vec<Diagnostic>), String> {
    let schema: JsonValue = serde_json::from_str(text)
        .map_err(|e| hard_error("invalid JSON Schema", &e.to_string()))?;
    let mut conv = SchemaConverter::new();
    let out = conv.convert_root(&schema);
    conv.diags.sort_by(|a, b| a.code.cmp(&b.code));
    Ok((out, conv.diags))
}

struct SchemaConverter {
    diags: Vec<Diagnostic>,
}

impl SchemaConverter {
    fn new() -> Self {
        Self { diags: Vec::new() }
    }

    fn note(&mut self, message: String) {
        self.note_code("W012", message);
    }

    fn note_code(&mut self, code: &str, message: String) {
        self.diags.push(Diagnostic {
            severity: Severity::Warning,
            code: code.into(),
            message,
            path: None,
            span: None,
            repair: None,
            confidence: 1.0,
            file: None,
        });
    }

    fn convert_root(&mut self, schema: &JsonValue) -> String {
        let mut out = String::from("# Imported from JSON Schema (L-021): constructs without an Active counterpart, if any, are reported as W012 notes.\n");
        match schema {
            JsonValue::Object(_) => {
                out.push_str("schema = ");
                out.push_str(&self.convert_root_object(schema));
                out.push('\n');
            }
            JsonValue::Bool(true) => {
                out.push_str("schema = {}\n");
            }
            JsonValue::Bool(false) => {
                self.note("boolean schema `false` (accept-nothing) has no Active counterpart; emitted open schema".to_string());
                out.push_str("schema = {}\n");
            }
            other => {
                self.note(format!(
                    "top-level schema must be an object or boolean, got {}; emitted open schema",
                    kind_word(other)
                ));
                out.push_str("schema = {}\n");
            }
        }
        out
    }

    /// Top-level object schema emits the schema BLOCK form (`schema = {
    /// ... }`); a `Table {...}` type constructor is only valid nested
    /// inside a block (probed: top-level `Table` parses as an inline-table
    /// value, not a type). Nested objects keep the `Table` form.
    fn convert_root_object(&mut self, schema: &JsonValue) -> String {
        let Some(obj) = schema.as_object() else {
            return "Table".to_string();
        };
        let props = obj.get("properties").and_then(|v| v.as_object());
        let required: Vec<&str> = obj
            .get("required")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        // Loud skips for top-level combinators/refs, same list as nodes.
        for key in [
            "allOf",
            "anyOf",
            "oneOf",
            "not",
            "$ref",
            "$defs",
            "definitions",
            "$anchor",
            "$dynamicRef",
            "$dynamicAnchor",
            "if",
            "then",
            "else",
            "dependentRequired",
            "dependentSchemas",
            "contains",
            "prefixItems",
            "unevaluatedProperties",
            "unevaluatedItems",
            "contentMediaType",
            "contentEncoding",
            "multipleOf",
            "const",
        ] {
            if obj.contains_key(key) {
                self.note(format!(
                    "JSON Schema '{key}' at '(root)' has no Active counterpart; skipped"
                ));
            }
        }
        let empty = empty_map();
        let mut fields = self.object_fields(obj, props.unwrap_or(&empty), &required, "");
        if obj.get("additionalProperties").and_then(|v| v.as_bool()) == Some(false) {
            if !fields.is_empty() {
                fields.push_str(", ");
            }
            fields.push_str("closed = true");
        }
        format!("{{ {fields} }}")
    }

    fn field_name(&mut self, key: &str) -> String {
        let clean = sanitize_key(key);
        if clean != key {
            // Same code and message shape as value imports (W007): the
            // sanitizer is one rule everywhere.
            self.note_code("W007", format!("imported key '{key}' renamed to '{clean}'"));
        }
        clean
    }

    fn convert_node(&mut self, schema: &JsonValue, path: &str) -> String {
        let obj = match schema.as_object() {
            Some(o) => o,
            None => return "Any".to_string(),
        };
        // Loud skips first: combinators, references, conditionals.
        for key in [
            "allOf",
            "anyOf",
            "oneOf",
            "not",
            "$ref",
            "$defs",
            "definitions",
            "$anchor",
            "$dynamicRef",
            "$dynamicAnchor",
            "if",
            "then",
            "else",
            "dependentRequired",
            "dependentSchemas",
            "contains",
            "prefixItems",
            "unevaluatedProperties",
            "unevaluatedItems",
            "contentMediaType",
            "contentEncoding",
        ] {
            if obj.contains_key(key) {
                self.note(format!(
                    "JSON Schema '{key}' at '{}' has no Active counterpart; skipped",
                    display_path(path)
                ));
            }
        }
        // `title`/`description` are documentation metadata, not constraints:
        // nothing to emit, nothing lost.
        let props = obj.get("properties").and_then(|v| v.as_object());
        let required: Vec<&str> = obj
            .get("required")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let typ = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Properties win over a contradictory scalar type (JSON Schema
        // applies them to objects; flag the contradiction loudly).
        if props.is_some() && !matches!(typ, "" | "object") {
            self.note(format!(
                "properties at '{}' contradict type '{typ}'; properties win",
                display_path(path)
            ));
        }
        match typ {
            "object" => self.convert_object(obj, props.unwrap_or(&empty_map()), &required, path),
            "" if props.is_some() => {
                self.convert_object(obj, props.unwrap_or(&empty_map()), &required, path)
            }
            // `{}` or metadata-only: open `Any` accepts everything, which
            // is what an unconstrained schema means.
            "" => "Any".to_string(),
            "array" => self.convert_array(obj, path),
            "string" => self.convert_string(obj, path),
            "integer" => self.convert_int(obj, path),
            "number" => self.convert_number(obj, path),
            "boolean" => "Bool".to_string(),
            "null" => "Null".to_string(),
            other => {
                self.note(format!(
                    "unknown type '{other}' at '{}'; emitted Any",
                    display_path(path)
                ));
                "Any".to_string()
            }
        }
    }

    fn convert_object(
        &mut self,
        obj: &serde_json::Map<String, JsonValue>,
        props: &serde_json::Map<String, JsonValue>,
        required: &[&str],
        path: &str,
    ) -> String {
        let fields = self.object_fields(obj, props, required, path);
        let closed = obj.get("additionalProperties").and_then(|v| v.as_bool()) == Some(false);
        let mut out = String::from("Table { ");
        out.push_str(&fields);
        if closed {
            if !fields.is_empty() {
                out.push_str(", ");
            }
            out.push_str("closed = true");
        }
        out.push_str(" }");
        out
    }

    fn object_fields(
        &mut self,
        _obj: &serde_json::Map<String, JsonValue>,
        props: &serde_json::Map<String, JsonValue>,
        required: &[&str],
        path: &str,
    ) -> String {
        let mut parts = Vec::new();
        for (key, subschema) in props {
            let name = self.field_name(key);
            let child = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            let rendered = self.convert_node(subschema, &child);
            let is_required = required.contains(&key.as_str());
            let has_default = subschema
                .as_object()
                .is_some_and(|o| o.contains_key("default"));
            if !is_required && !has_default {
                self.note(format!(
                    "optional property '{child}' without default tightens to required (Active has no optional-without-default)"
                ));
            }
            // A property literally named `schema` holding a table would be
            // consumed as a schema declaration: suffix exactly then.
            let name = if name == "schema" && rendered.starts_with("Table") {
                self.note("property 'schema' holding a table renamed to 'schema_' (would consume as declaration)".to_string());
                "schema_".to_string()
            } else {
                name
            };
            parts.push(format!("{name} = {rendered}"));
        }
        // Defaults ride on the type args via convert_* below (they read
        // `default` from their own node); nothing extra to emit here.
        parts.join(", ")
    }

    fn default_arg(&self, obj: &serde_json::Map<String, JsonValue>) -> Option<String> {
        match obj.get("default") {
            Some(JsonValue::String(s)) => Some(format!("default = {}", Emitter::quote_schema(s))),
            Some(JsonValue::Bool(b)) => Some(format!("default = {b}")),
            Some(JsonValue::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    Some(format!("default = {i}"))
                } else {
                    n.as_f64().map(|f| format!("default = {f}"))
                }
            }
            _ => None,
        }
    }

    fn convert_string(&mut self, obj: &serde_json::Map<String, JsonValue>, path: &str) -> String {
        let mut args = Vec::new();
        if let Some(n) = obj.get("minLength").and_then(|v| v.as_u64()) {
            args.push(format!("min_len = {n}"));
        }
        if let Some(n) = obj.get("maxLength").and_then(|v| v.as_u64()) {
            args.push(format!("max_len = {n}"));
        }
        if let Some(p) = obj.get("pattern").and_then(|v| v.as_str()) {
            args.push(format!("pattern = {}", Emitter::quote_schema(p)));
        }
        if let Some(members) = obj.get("enum").and_then(|v| v.as_array()) {
            let mut strs = Vec::new();
            let mut ok = true;
            for m in members {
                match m.as_str() {
                    Some(s) => strs.push(Emitter::quote_schema(s)),
                    None => {
                        ok = false;
                    }
                }
            }
            if ok && !strs.is_empty() {
                if obj.contains_key("default") {
                    // Enum takes members only: a default has nowhere to
                    // attach, so it drops loudly instead of smuggling into
                    // the member list (which E020 rejects).
                    self.note(format!(
                        "default at '{}' dropped: Enum members must be bare string literals",
                        display_path(path)
                    ));
                }
                return format!("Enum({})", strs.join(", "));
            }
            self.note(format!(
                "non-string enum at '{}' has no Active counterpart; emitted String",
                display_path(path)
            ));
        }
        if obj.contains_key("const") {
            self.note(format!(
                "const at '{}' has no Active counterpart; emitted String",
                display_path(path)
            ));
        }
        let mut all = args;
        all.extend(self.default_arg(obj));
        if all.is_empty() {
            "String".to_string()
        } else {
            format!("String({})", all.join(", "))
        }
    }

    fn int_bound(
        &mut self,
        obj: &serde_json::Map<String, JsonValue>,
        path: &str,
        key: &str,
        out_key: &str,
    ) -> Option<String> {
        if let Some(n) = obj.get(key).and_then(|v| v.as_i64()) {
            return Some(format!("{out_key} = {n}"));
        }
        if obj.contains_key(key) {
            self.note(format!(
                "non-integer '{key}' at '{}' skipped (only integer bounds map)",
                display_path(path)
            ));
        }
        None
    }

    fn convert_int(&mut self, obj: &serde_json::Map<String, JsonValue>, path: &str) -> String {
        let mut args = Vec::new();
        if let Some(b) = self.int_bound(obj, path, "minimum", "min") {
            args.push(b);
        }
        if let Some(b) = self.int_bound(obj, path, "maximum", "max") {
            args.push(b);
        }
        for key in ["exclusiveMinimum", "exclusiveMaximum"] {
            if obj.contains_key(key) {
                self.note(format!(
                    "'{key}' at '{}' imported as inclusive bound (direction loosened, announced)",
                    display_path(path)
                ));
                if let Some(n) = obj.get(key).and_then(|v| v.as_i64()) {
                    args.push(format!(
                        "{} = {n}",
                        if key == "exclusiveMinimum" {
                            "min"
                        } else {
                            "max"
                        }
                    ));
                }
            }
        }
        if obj.contains_key("multipleOf") {
            self.note(format!(
                "'multipleOf' at '{}' has no Active counterpart; skipped",
                display_path(path)
            ));
        }
        if obj.contains_key("const") {
            self.note(format!(
                "const at '{}' has no Active counterpart; emitted Int",
                display_path(path)
            ));
        }
        let mut all = args;
        all.extend(self.default_arg(obj));
        if all.is_empty() {
            "Int".to_string()
        } else {
            format!("Int({})", all.join(", "))
        }
    }

    fn convert_number(&mut self, obj: &serde_json::Map<String, JsonValue>, path: &str) -> String {
        let mut args = Vec::new();
        for (key, out_key) in [("minimum", "min"), ("maximum", "max")] {
            if let Some(n) = obj.get(key).and_then(|v| v.as_f64()) {
                args.push(format!("{out_key} = {n}"));
            }
        }
        for key in ["exclusiveMinimum", "exclusiveMaximum"] {
            if obj.contains_key(key) {
                self.note(format!(
                    "'{key}' at '{}' imported as inclusive bound (direction loosened, announced)",
                    display_path(path)
                ));
                if let Some(n) = obj.get(key).and_then(|v| v.as_f64()) {
                    args.push(format!(
                        "{} = {n}",
                        if key == "exclusiveMinimum" {
                            "min"
                        } else {
                            "max"
                        }
                    ));
                }
            }
        }
        if obj.contains_key("multipleOf") {
            self.note(format!(
                "'multipleOf' at '{}' has no Active counterpart; skipped",
                display_path(path)
            ));
        }
        let mut all = args;
        all.extend(self.default_arg(obj));
        if all.is_empty() {
            "Float".to_string()
        } else {
            format!("Float({})", all.join(", "))
        }
    }

    fn convert_array(&mut self, obj: &serde_json::Map<String, JsonValue>, path: &str) -> String {
        for key in ["minItems", "maxItems"] {
            if obj.contains_key(key) {
                self.note(format!(
                    "'{key}' at '{}' has no Active counterpart; skipped",
                    display_path(path)
                ));
            }
        }
        match obj.get("items") {
            Some(items) => format!("Array<{}>", self.convert_node(items, &format!("{path}[]"))),
            None => "Array<Any>".to_string(),
        }
    }
}

fn kind_word(v: &JsonValue) -> &'static str {
    match v {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

fn empty_map() -> serde_json::Map<String, JsonValue> {
    serde_json::Map::new()
}
