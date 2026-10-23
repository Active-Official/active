//! Typed code generation from Active schemas (C-047,
//! spec/converters.md §7). Reads the module's schema tree and emits
//! JSON Schema, an OpenAPI fragment, TypeScript interfaces, Go
//! structs, and Rust serde types. Only static literal constraints
//! render — dynamic bounds still validate at runtime but have no
//! static form (documented, never guessed).

use crate::ir::{Module, Node, NodeId, SchemaField};
use crate::types::ActiveType;
use serde_json::{json, Value as JsonValue};
use std::collections::{HashMap, HashSet};

fn no_schema() -> String {
    "document has no schema block".to_string()
}

fn schema_root(module: &Module) -> Result<NodeId, String> {
    module.schema.ok_or_else(no_schema)
}

// ------------------------------------------------------------------
// Shared shape helpers.
// ------------------------------------------------------------------

/// Static literal integer bound (dynamic expressions skipped).
fn literal_int(module: &Module, id: Option<NodeId>) -> Option<i64> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::Int(i))) => Some(*i),
        _ => None,
    }
}

/// Static literal float bound (plain integers widen).
fn literal_float(module: &Module, id: Option<NodeId>) -> Option<f64> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::Float(f))) => Some(*f),
        Some(Node::Literal(crate::ir::Value::Int(i))) => Some(*i as f64),
        _ => None,
    }
}

/// Static literal string bound.
pub(crate) fn literal_string(module: &Module, id: Option<NodeId>) -> Option<String> {
    match id.and_then(|node| module.get(node)) {
        Some(Node::Literal(crate::ir::Value::String(s))) => Some(s.clone()),
        _ => None,
    }
}

/// Field `description` slot for doc comments across codegen targets.
fn field_description(module: &Module, id: NodeId) -> Option<String> {
    match module.get(id) {
        Some(crate::ir::Node::SchemaField { field }) => literal_string(module, field.description),
        _ => None,
    }
}

/// Enum member descriptions joined as `"member: description"` pairs for
/// targets with a single description string (JSON Schema `description`).
/// Length mismatches cannot reach here: validation reports them first,
/// and codegen reads the same evaluated nodes.
fn enum_descriptions_text(module: &Module, field: &SchemaField) -> Option<String> {
    let members = match &field.typ {
        ActiveType::Enum(members) => members,
        _ => return None,
    };
    let id = field.enum_descriptions?;
    let texts: Vec<String> = match module.get(id) {
        Some(Node::Literal(crate::ir::Value::Array(items))) => items
            .iter()
            .map(|v| match v {
                crate::ir::Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        Some(Node::Array { elements }) => elements
            .iter()
            .map(|e| match module.get(*e) {
                Some(Node::Literal(crate::ir::Value::String(text))) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    if texts.len() != members.len() {
        return None;
    }
    Some(
        members
            .iter()
            .zip(texts.iter())
            .map(|(member, text)| format!("{member}: {text}"))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

/// Static literal default as JSON (any literal kind, redacted like
/// every export — secrets never leak through generated defaults).
pub(crate) fn literal_default(module: &Module, field: &SchemaField) -> Option<JsonValue> {
    match field.default.and_then(|node| module.get(node)) {
        Some(Node::Literal(value)) => Some(super::value_to_json(value)),
        _ => None,
    }
}

/// Required unless a static default exists or the field is Null
/// (absent Null keys validate clean — C-032). Tables and arrays are
/// always required: they carry no default slot.
fn is_required(module: &Module, id: NodeId) -> bool {
    match module.get(id) {
        Some(Node::SchemaField { field }) => {
            !matches!(field.typ, ActiveType::Null) && literal_default(module, field).is_none()
        }
        _ => true,
    }
}

fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    for part in s.split(['_', '-']) {
        let mut chars = part.chars();
        match chars.next() {
            None => out.push('_'),
            Some(first) => {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    if out.is_empty() {
        return "Type".to_string();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn sanitize_ident(s: &str, fallback: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty()
        || clean.starts_with(|c: char| c.is_ascii_digit())
        || !clean
            .chars()
            .all(|c: char| c.is_ascii_alphanumeric() || c == '_')
    {
        return fallback.to_string();
    }
    clean
}

/// Codegen pipeline version (CHECKLIST C.008): bump on any output-shape
/// change in any target. Emitters stamp it into every artifact next to
/// the workspace version so agents can detect stale bindings.
/// v2: Blob identity shape (x-active-blob / ActiveBlob) on all targets.
pub const CODEGEN_VERSION: u32 = 2;

/// Single-line generated-file header for text targets (`prefix` is the
/// comment marker: `//` for TS/Go/Rust, `#` for Python).
fn version_header(prefix: &str) -> String {
    format!(
        "{prefix} Generated by Active {} (codegen v{}). Do not edit.\n",
        env!("CARGO_PKG_VERSION"),
        CODEGEN_VERSION
    )
}

/// Innermost nominal node through array wrappers: arrays are
/// transparent for naming (the element table/enum owns the name).
fn nominal_id(module: &Module, mut id: NodeId) -> NodeId {
    while let Some(Node::SchemaArray { element }) = module.get(id) {
        id = *element;
    }
    id
}

// ------------------------------------------------------------------
// JSON Schema (Draft 2020-12 compatible subset).
// ------------------------------------------------------------------

/// The schema tree as JSON Schema: objects for tables (closed →
/// `additionalProperties: false`), enums, arrays, constraints from
/// static literals, static defaults, and `deprecated` flags.
pub fn schema_to_json_schema(module: &Module) -> Result<JsonValue, String> {
    let root = schema_root(module)?;
    let mut node = json_schema_node(module, root);
    if let JsonValue::Object(ref mut map) = node {
        map.insert(
            "$comment".to_string(),
            JsonValue::String(format!(
                "Generated by Active {} (codegen v{}). Do not edit.",
                env!("CARGO_PKG_VERSION"),
                CODEGEN_VERSION
            )),
        );
    }
    Ok(node)
}

fn json_schema_node(module: &Module, id: NodeId) -> JsonValue {
    match module.get(id) {
        Some(Node::SchemaTable { fields, closed, .. }) => {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for (name, field) in fields {
                properties.insert(name.clone(), json_schema_node(module, *field));
                if is_required(module, *field) {
                    required.push(JsonValue::String(name.clone()));
                }
            }
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert("properties".to_string(), JsonValue::Object(properties));
            if !required.is_empty() {
                obj.insert("required".to_string(), JsonValue::Array(required));
            }
            let mut dependent: serde_json::Map<String, JsonValue> = serde_json::Map::new();
            for (name, field_id) in fields {
                if let Some(crate::ir::Node::SchemaField { field }) = module.get(*field_id) {
                    if let Some(target) = literal_string(module, field.requires) {
                        dependent
                            .entry(target)
                            .or_insert_with(|| JsonValue::Array(Vec::new()))
                            .as_array_mut()
                            .expect("dependentRequired arrays only")
                            .push(JsonValue::String(name.clone()));
                    }
                }
            }
            if !dependent.is_empty() {
                obj.insert(
                    "dependentRequired".to_string(),
                    JsonValue::Object(dependent),
                );
            }
            if *closed {
                obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            }
            JsonValue::Object(obj)
        }
        Some(Node::SchemaArray { element }) => {
            json!({"type": "array", "items": json_schema_node(module, *element)})
        }
        Some(Node::SchemaField { field }) => json_schema_field(module, field),
        _ => JsonValue::Bool(true),
    }
}

fn json_schema_field(module: &Module, field: &SchemaField) -> JsonValue {
    let mut obj = serde_json::Map::new();
    match &field.typ {
        ActiveType::Null => {
            obj.insert("type".to_string(), JsonValue::String("null".to_string()));
        }
        ActiveType::Bool => {
            obj.insert("type".to_string(), JsonValue::String("boolean".to_string()));
        }
        ActiveType::Int => {
            obj.insert("type".to_string(), JsonValue::String("integer".to_string()));
            if let Some(min) = literal_int(module, field.min) {
                obj.insert("minimum".to_string(), JsonValue::from(min));
            }
            if let Some(max) = literal_int(module, field.max) {
                obj.insert("maximum".to_string(), JsonValue::from(max));
            }
        }
        ActiveType::Float => {
            obj.insert("type".to_string(), JsonValue::String("number".to_string()));
            if let Some(min) = literal_float(module, field.min) {
                obj.insert("minimum".to_string(), JsonValue::from(min));
            }
            if let Some(max) = literal_float(module, field.max) {
                obj.insert("maximum".to_string(), JsonValue::from(max));
            }
        }
        ActiveType::String => {
            obj.insert("type".to_string(), JsonValue::String("string".to_string()));
            if let Some(min) = literal_int(module, field.min_len) {
                obj.insert("minLength".to_string(), JsonValue::from(min));
            }
            if let Some(max) = literal_int(module, field.max_len) {
                obj.insert("maxLength".to_string(), JsonValue::from(max));
            }
            if let Some(pattern) = literal_string(module, field.pattern) {
                obj.insert("pattern".to_string(), JsonValue::String(pattern));
            }
            if let Some(format) = literal_string(module, field.format) {
                obj.insert("format".to_string(), JsonValue::String(format));
            }
        }
        // Matches the §1 export shape: durations serialize as
        // `{"ms": n}` objects.
        ActiveType::Duration => {
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert("properties".to_string(), json!({"ms": {"type": "integer"}}));
            obj.insert("required".to_string(), json!(["ms"]));
            obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
        }
        // Bytes normalize to integers on export.
        ActiveType::Bytes => {
            obj.insert("type".to_string(), JsonValue::String("integer".to_string()));
        }
        ActiveType::Table { closed } => {
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            if *closed {
                obj.insert("additionalProperties".to_string(), JsonValue::Bool(false));
            }
        }
        ActiveType::Enum(members) => {
            obj.insert(
                "enum".to_string(),
                JsonValue::Array(members.iter().cloned().map(JsonValue::String).collect()),
            );
            if let Some(joined) = enum_descriptions_text(module, field) {
                obj.insert("description".to_string(), JsonValue::String(joined));
            }
        }
        // Secrets export as redacted strings.
        ActiveType::Secret => {
            obj.insert("type".to_string(), JsonValue::String("string".to_string()));
            obj.insert(
                "description".to_string(),
                JsonValue::String("secret value, redacted as ***SECRET*** on export".to_string()),
            );
        }
        // Blobs export as their identity object (spec/blob.md §5).
        ActiveType::Blob => {
            obj.insert("type".to_string(), JsonValue::String("object".to_string()));
            obj.insert("x-active-blob".to_string(), JsonValue::Bool(true));
            if let Some(media) = literal_string(module, field.blob_media_type) {
                obj.insert("x-active-media-type".to_string(), JsonValue::String(media));
            }
            if let Some(max) = literal_int(module, field.blob_max_size) {
                obj.insert("x-active-max-size".to_string(), JsonValue::from(max));
            }
        }
        ActiveType::Any => return JsonValue::Bool(true),
    }
    if let Some(default) = literal_default(module, field) {
        obj.insert("default".to_string(), default);
    }
    if field.description.is_some() {
        if let Some(doc) = literal_string(module, field.description) {
            obj.insert("description".to_string(), JsonValue::String(doc));
        }
    }
    if let Some(reason) = field.deprecated.as_deref() {
        obj.insert("deprecated".to_string(), JsonValue::Bool(true));
        if !reason.is_empty() {
            obj.insert(
                "description".to_string(),
                JsonValue::String(reason.to_string()),
            );
        }
    }
    JsonValue::Object(obj)
}

// ------------------------------------------------------------------
// OpenAPI fragment (3.1: plain JSON Schema under components).
// ------------------------------------------------------------------

/// The JSON Schema wrapped as an OpenAPI 3.1
/// `components.schemas.<name>` fragment.
pub fn schema_to_openapi(module: &Module, name: &str) -> Result<JsonValue, String> {
    let schema = schema_to_json_schema(module)?;
    let mut schemas = serde_json::Map::new();
    schemas.insert(sanitize_ident(name, "Config"), schema);
    let mut components = serde_json::Map::new();
    components.insert("schemas".to_string(), JsonValue::Object(schemas));
    let mut out = serde_json::Map::new();
    out.insert(
        "openapi".to_string(),
        JsonValue::String("3.1.0".to_string()),
    );
    out.insert("components".to_string(), JsonValue::Object(components));
    Ok(JsonValue::Object(out))
}

// ------------------------------------------------------------------
// TypeScript interfaces.
// ------------------------------------------------------------------

/// The schema tree as TypeScript `export interface` declarations plus
/// zod runtime schemas (`<Name>Schema`) mirroring every interface with
/// the same constraints (min/max, patterns, formats, enums, defaults).
/// Nested tables become PascalCase interfaces; `?`/`.optional()` marks
/// defaulted/Null fields.
pub fn schema_to_typescript(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = TsGen {
        module,
        out: format!("{}import {{ z }} from \"zod\";\n\n", version_header("//")),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    gen.out
        .push_str("\n// Runtime validation schemas (zod) mirroring the interfaces above.\n");
    gen.emit_zod_table(&root_name, root);
    Ok(gen.out)
}

struct TsGen<'a> {
    module: &'a Module,
    out: String,
    /// Type name per node plus every assigned name: same-derived
    /// names for different nodes (`port` vs `Port` tables) gain
    /// suffixes deterministically instead of colliding.
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl TsGen<'_> {
    /// Canonical type name for a node under a derived base: stable
    /// across the reference and declaration passes, which compute
    /// the same base independently.
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("export interface {name} {{\n"));
        for (key, field) in &fields {
            let optional = !is_required(self.module, *field);
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let typ = self.ts_type(&tname, *field);
            let mark = if optional { "?" } else { "" };
            if let Some(doc) = field_description(self.module, *field) {
                self.out
                    .push_str(&format!("  /** {} */\n", ts_comment(&doc)));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("  /** @deprecated */\n");
                } else {
                    self.out
                        .push_str(&format!("  /** @deprecated {} */\n", ts_comment(&reason)));
                }
            }
            self.out.push_str(&format!("  {key}{mark}: {typ};\n"));
        }
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn ts_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            // Named by the caller: emit_table/emit_nested declares
            // the same hint, so references always resolve.
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("{}[]", self.ts_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.ts_field(field),
            _ => "unknown".to_string(),
        }
    }

    /// Inline type for array elements, threaded with the same naming
    /// hint the declaration pass uses — element tables resolve to
    /// the named interface, never to anonymous duplicates.
    fn ts_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("{}[]", self.ts_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.ts_field(field),
            _ => "unknown".to_string(),
        }
    }

    fn ts_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null => "null".to_string(),
            ActiveType::Bool => "boolean".to_string(),
            ActiveType::Int | ActiveType::Float | ActiveType::Duration | ActiveType::Bytes => {
                "number".to_string()
            }
            ActiveType::String | ActiveType::Secret => "string".to_string(),
            ActiveType::Blob => "{ \"$active.blob\": { hash: string; size: number; media_type: string; tainted: boolean } }".to_string(),
            ActiveType::Table { .. } => "Record<string, unknown>".to_string(),
            ActiveType::Enum(members) => members
                .iter()
                .map(|m| format!("{m:?}"))
                .collect::<Vec<_>>()
                .join(" | "),
            ActiveType::Any => "unknown".to_string(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field.deprecated.clone(),
            _ => None,
        }
    }

    /// Zod schema const for one table, children first so every
    /// referenced `<Name>Schema` const already exists at module load
    /// (no temporal-dead-zone reads). Names reuse the interface memo,
    /// so refs always resolve to the declared interfaces' schemas.
    /// Closed tables emit `.strict()` so extra keys are rejected exactly
    /// like `additionalProperties: false` in JSON Schema and Active.
    fn emit_zod_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, closed, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            match self.module.get(*field) {
                Some(Node::SchemaTable { .. }) => self.emit_zod_table(&tname, *field),
                Some(Node::SchemaArray { element }) => {
                    self.emit_zod_nested(&tname, *element);
                }
                _ => {}
            }
        }
        self.out
            .push_str(&format!("export const {name}Schema = z.object({{\n"));
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let expr = self.zod_type(&tname, *field);
            self.out.push_str(&format!("  {key}: {expr},\n"));
        }
        if *closed {
            self.out.push_str("}).strict();\n");
        } else {
            self.out.push_str("});\n");
        }
    }

    fn emit_zod_nested(&mut self, hint: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_zod_table(hint, id),
            Some(Node::SchemaArray { element }) => self.emit_zod_nested(hint, *element),
            _ => {}
        }
    }

    /// Zod expression for a field node: table refs, arrays, or a leaf
    /// with the same static checks the interfaces document.
    fn zod_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => {
                format!("{}Schema", self.type_name(id, hint.to_string()))
            }
            Some(Node::SchemaArray { element }) => {
                format!("z.array({})", self.zod_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => {
                let required = is_required(self.module, id);
                self.zod_leaf(field, required)
            }
            _ => "z.unknown()".to_string(),
        }
    }

    fn zod_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => {
                format!("{}Schema", self.type_name(id, hint.to_string()))
            }
            Some(Node::SchemaArray { element }) => {
                format!("z.array({})", self.zod_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.zod_leaf(field, true),
            _ => "z.unknown()".to_string(),
        }
    }

    /// Leaf zod expression with static checks. `required` is false for
    /// defaulted/Null fields (`.optional()`/`.default()`); array
    /// elements always pass true (element position has no optionality).
    fn zod_leaf(&self, field: &SchemaField, required: bool) -> String {
        let mut out = match &field.typ {
            ActiveType::Null => "z.null()".to_string(),
            ActiveType::Bool => "z.boolean()".to_string(),
            ActiveType::Int => "z.number().int()".to_string(),
            ActiveType::Float => "z.number()".to_string(),
            ActiveType::Duration | ActiveType::Bytes => "z.number().int()".to_string(),
            ActiveType::Secret => "z.string()".to_string(),
            ActiveType::Blob => "z.object({ \"$active.blob\": z.object({ hash: z.string(), size: z.number(), media_type: z.string(), tainted: z.boolean() }) })".to_string(),
            ActiveType::Table { .. } => "z.record(z.string(), z.unknown())".to_string(),
            ActiveType::Any => "z.unknown()".to_string(),
            ActiveType::Enum(members) => {
                let items: Vec<String> = members
                    .iter()
                    .map(|m| JsonValue::String(m.clone()).to_string())
                    .collect();
                format!("z.enum([{}])", items.join(", "))
            }
            ActiveType::String => {
                let mut expr = "z.string()".to_string();
                if let Some(min) = literal_int(self.module, field.min_len) {
                    expr.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_int(self.module, field.max_len) {
                    expr.push_str(&format!(".max({max})"));
                }
                if let Some(pattern) = literal_string(self.module, field.pattern) {
                    let quoted = JsonValue::String(pattern).to_string();
                    expr.push_str(&format!(".regex(new RegExp({quoted}))"));
                }
                match literal_string(self.module, field.format).as_deref() {
                    // zod's email/url/uuid definitions are its own (close
                    // to but not identical with Active's anchored
                    // patterns); strict format equality lives in Active
                    // and JSON Schema, runtime convenience here.
                    Some("email") => expr.push_str(".email()"),
                    Some("uri") => expr.push_str(".url()"),
                    Some("uuid") => expr.push_str(".uuid()"),
                    Some("semver") => expr.push_str(
                        ".regex(/^(0|[1-9]\\d*)\\.(0|[1-9]\\d*)\\.(0|[1-9]\\d*)(?:-[0-9A-Za-z.-]+)?(?:\\+[0-9A-Za-z.-]+)?$/)",
                    ),
                    _ => {}
                }
                expr
            }
        };
        match &field.typ {
            ActiveType::Int => {
                if let Some(min) = literal_int(self.module, field.min) {
                    out.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_int(self.module, field.max) {
                    out.push_str(&format!(".max({max})"));
                }
            }
            ActiveType::Float => {
                if let Some(min) = literal_float(self.module, field.min) {
                    out.push_str(&format!(".min({min})"));
                }
                if let Some(max) = literal_float(self.module, field.max) {
                    out.push_str(&format!(".max({max})"));
                }
            }
            _ => {}
        }
        if !required {
            if let Some(default) = literal_default(self.module, field) {
                match default {
                    JsonValue::String(_) | JsonValue::Number(_) | JsonValue::Bool(_) => {
                        out.push_str(&format!(".default({default})"));
                    }
                    _ => out.push_str(".optional()"),
                }
            } else {
                out.push_str(".optional()");
            }
        }
        out
    }
}

// ------------------------------------------------------------------
// Go structs.
// ------------------------------------------------------------------

/// The schema tree as Go structs with `json` tags plus
/// go-playground/validator `validate` tags (required, min/max, email,
/// url, uuid, oneof; enforced by that library, not by this file).
/// Nested tables become PascalCase structs; enums stay strings with
/// their members documented.
pub fn schema_to_go(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = GoGen {
        module,
        out: format!(
            "{}// Validation tags follow go-playground/validator conventions.\n",
            version_header("//")
        ),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    if module.arena.iter().any(|n| {
        matches!(
            n,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    }) {
        // Shared identity shape for Blob fields (spec/blob.md §5):
        // hash, size, media type, taint. Payloads never cross codegen.
        // Aligned through the same run formatter as tables so the
        // output stays gofmt-clean (the equivalence gate checks).
        gen.out.push_str("\ntype ActiveBlob struct {\n");
        let mut run = vec![
            (
                "Hash".to_string(),
                "string".to_string(),
                "`json:\"hash\"`".to_string(),
            ),
            (
                "Size".to_string(),
                "uint64".to_string(),
                "`json:\"size\"`".to_string(),
            ),
            (
                "MediaType".to_string(),
                "string".to_string(),
                "`json:\"media_type\"`".to_string(),
            ),
            (
                "Tainted".to_string(),
                "bool".to_string(),
                "`json:\"tainted\"`".to_string(),
            ),
        ];
        flush_go_run(&mut run, &mut gen.out);
        gen.out.push_str("}\n");
    }
    Ok(gen.out)
}

/// Emit one contiguous run of struct fields with gofmt-style column
/// alignment: comment lines break runs, so each run aligns names and
/// types independently, exactly as gofmt does. Widths use byte length,
/// which equals display width here: schema keys lex as ASCII-only
/// identifiers (lexer.rs), so names and derived type names never
/// contain multi-byte characters.
fn flush_go_run(run: &mut Vec<(String, String, String)>, out: &mut String) {
    if run.is_empty() {
        return;
    }
    let nw = run.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
    let tw = run.iter().map(|(_, t, _)| t.len()).max().unwrap_or(0);
    for (n, t, tag) in run.drain(..) {
        out.push_str(&format!("\t{n:<nw$} {t:<tw$} {tag}\n", nw = nw, tw = tw));
    }
}

struct GoGen<'a> {
    module: &'a Module,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl GoGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("type {name} struct {{\n"));
        let mut taken = HashSet::new();
        // Collect the body first so gofmt-style column alignment can be
        // computed per contiguous run of fields (comment lines break runs,
        // exactly as gofmt does).
        enum BodyLine {
            Comment(String),
            Field {
                name: String,
                typ: String,
                tag: String,
            },
        }
        let mut lines: Vec<BodyLine> = Vec::new();
        for (key, field) in &fields {
            let mut field_name = pascal_case(key);
            while !taken.insert(field_name.clone()) {
                field_name.push('_');
            }
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let typ = self.go_type(&tname, *field);
            if let Some(doc) = field_description(self.module, *field) {
                lines.push(BodyLine::Comment(doc));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    lines.push(BodyLine::Comment(
                        "Deprecated: no reason given.".to_string(),
                    ));
                } else {
                    lines.push(BodyLine::Comment(format!("Deprecated: {reason}")));
                }
            }
            for note in self.doc(*field) {
                lines.push(BodyLine::Comment(note));
            }
            lines.push(BodyLine::Field {
                name: field_name,
                typ,
                tag: self.go_tag(key, *field),
            });
        }
        let mut run: Vec<(String, String, String)> = Vec::new();
        for line in lines {
            match line {
                BodyLine::Comment(c) => {
                    flush_go_run(&mut run, &mut self.out);
                    self.out.push_str(&format!("\t// {}\n", comment_line(&c)));
                }
                BodyLine::Field { name, typ, tag } => run.push((name, typ, tag)),
            }
        }
        flush_go_run(&mut run, &mut self.out);
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn go_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("[]{}", self.go_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.go_field(field),
            _ => "interface{}".to_string(),
        }
    }

    fn go_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            // Element tables resolve to the named struct the
            // declaration pass emits under the same hint.
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("[]{}", self.go_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.go_field(field),
            _ => "interface{}".to_string(),
        }
    }

    /// Struct tag with `json` name plus go-playground/validator rules
    /// derived from static constraints. `pattern` has no validator
    /// equivalent and is documented on the field comment instead;
    /// `requires`/`deprecated` are schema-level concerns, not field tags.
    /// Divergence note: validator `required` means non-zero, while Active
    /// required means merely present — so `Bool` fields never carry
    /// `required` (an explicit `false` is a legitimate present value).
    /// The same non-zero reading applies to `0`, `""`, and `0.0` on other
    /// types: presence is authoritative in Active and JSON Schema, and the
    /// tags are a best-effort hint for the validator library.
    fn go_tag(&self, key: &str, id: NodeId) -> String {
        let mut rules: Vec<String> = Vec::new();
        if is_required(self.module, id) {
            let is_bool = matches!(
                self.module.get(id),
                Some(Node::SchemaField { field }) if matches!(field.typ, ActiveType::Bool)
            );
            if !is_bool {
                rules.push("required".to_string());
            }
        }
        if let Some(Node::SchemaField { field }) = self.module.get(id) {
            match &field.typ {
                ActiveType::Int => {
                    if let Some(min) = literal_int(self.module, field.min) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_int(self.module, field.max) {
                        rules.push(format!("max={max}"));
                    }
                }
                ActiveType::Float => {
                    if let Some(min) = literal_float(self.module, field.min) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_float(self.module, field.max) {
                        rules.push(format!("max={max}"));
                    }
                }
                ActiveType::String => {
                    if let Some(min) = literal_int(self.module, field.min_len) {
                        rules.push(format!("min={min}"));
                    }
                    if let Some(max) = literal_int(self.module, field.max_len) {
                        rules.push(format!("max={max}"));
                    }
                    match literal_string(self.module, field.format).as_deref() {
                        Some("email") => rules.push("email".to_string()),
                        Some("uri") => rules.push("url".to_string()),
                        Some("uuid") => rules.push("uuid".to_string()),
                        _ => {}
                    }
                }
                // Spaces or commas in a member would make oneof
                // ambiguous; the `// one of:` comment still documents
                // them, and the JSON Schema target stays exact.
                ActiveType::Enum(members) if !members.iter().any(|m| m.contains([' ', ','])) => {
                    rules.push(format!("oneof={}", members.join(" ")));
                }
                _ => {}
            }
        }
        if rules.is_empty() {
            format!("`json:\"{key}\"`")
        } else {
            format!("`json:\"{key}\" validate:\"{}\"`", rules.join(","))
        }
    }

    fn go_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null | ActiveType::Any => "interface{}".to_string(),
            ActiveType::Bool => "bool".to_string(),
            ActiveType::Int | ActiveType::Duration | ActiveType::Bytes => "int64".to_string(),
            ActiveType::Float => "float64".to_string(),
            ActiveType::String | ActiveType::Secret | ActiveType::Enum(_) => "string".to_string(),
            ActiveType::Blob => "ActiveBlob".to_string(),
            ActiveType::Table { .. } => "map[string]interface{}".to_string(),
        }
    }

    /// Field documentation notes: type-specific remarks plus the
    /// static facets with no validator-tag equivalent (`format`,
    /// `pattern`, `requires`) so nothing is silently dropped.
    fn doc(&self, id: NodeId) -> Vec<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => {
                let mut notes = Vec::new();
                match &field.typ {
                    ActiveType::Duration => notes.push("milliseconds".to_string()),
                    ActiveType::Enum(members) => {
                        notes.push(format!("one of: {}", members.join(", ")))
                    }
                    ActiveType::Secret => {
                        notes.push("secret value, redacted as ***SECRET*** on export".to_string())
                    }
                    _ => {}
                }
                if let Some(format) = literal_string(self.module, field.format) {
                    notes.push(format!("format: {format}"));
                }
                if let Some(pattern) = literal_string(self.module, field.pattern) {
                    notes.push(format!("pattern {pattern}"));
                }
                if let Some(target) = literal_string(self.module, field.requires) {
                    notes.push(format!("requires: {target}"));
                }
                notes
            }
            _ => Vec::new(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field.deprecated.clone(),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------
// Rust serde types.
// ------------------------------------------------------------------

const RUST_KEYWORDS: &[&str] = &[
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
];

/// The schema tree as Rust serde types: structs with `rename` tags,
/// real enums for `Enum`, `Option` for defaulted/Null fields.
pub fn schema_to_rust(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = RustGen {
        module,
        out: format!(
            "{}\
            // Requires: serde = {{ version = \"1\", features = [\"derive\"] }}, serde_json = \"1\"\nuse serde::{{Deserialize, Serialize}};\n\n",
            version_header("//")
        ),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    if module.arena.iter().any(|n| {
        matches!(
            n,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    }) {
        // Shared identity shape for Blob fields (spec/blob.md §5).
        gen.out.push_str(
            "\n#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct ActiveBlob {\n    pub hash: String,\n    pub size: u64,\n    pub media_type: String,\n    pub tainted: bool,\n}\n",
        );
    }
    Ok(gen.out)
}

struct RustGen<'a> {
    module: &'a Module,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

impl RustGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out
            .push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct ");
        self.out.push_str(name);
        self.out.push_str(" {\n");
        let mut taken = HashSet::new();
        for (key, field) in &fields {
            let optional = !is_required(self.module, *field);
            let raw = sanitize_ident(key, "field");
            let (field_name, mut renamed) = if RUST_KEYWORDS.contains(&raw.as_str()) {
                (format!("r#{raw}"), true)
            } else {
                (raw.clone(), raw != *key)
            };
            let mut field_name = field_name;
            while !taken.insert(field_name.clone()) {
                field_name.push('_');
                renamed = true;
            }
            let inner = {
                let nominal = nominal_id(self.module, *field);
                let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
                self.rust_type(&tname, *field)
            };
            let typ = if optional {
                format!("Option<{inner}>")
            } else {
                inner
            };
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("\t#[deprecated]\n");
                } else {
                    self.out
                        .push_str(&format!("\t#[deprecated(note = {reason:?})]\n"));
                }
            }
            // Raw identifiers (`r#type`) never match their key
            // textually; serde_derive strips the prefix, so an
            // explicit rename keeps the mapping exact either way.
            if renamed {
                self.out
                    .push_str(&format!("\t#[serde(rename = {key:?})]\n"));
            }
            self.out.push_str(&format!("\tpub {field_name}: {typ},\n"));
        }
        self.out.push_str("}\n");
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            Some(Node::SchemaField { field }) => {
                if let ActiveType::Enum(members) = &field.typ {
                    self.emit_enum(name, members);
                }
            }
            _ => {}
        }
    }

    fn emit_enum(&mut self, name: &str, members: &[String]) {
        self.out
            .push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\npub enum ");
        self.out.push_str(name);
        self.out.push_str(" {\n");
        let mut taken = HashSet::new();
        for member in members {
            let mut variant = pascal_case(&sanitize_ident(member, "Value"));
            while !taken.insert(variant.clone()) {
                variant.push('_');
            }
            if &variant != member {
                self.out
                    .push_str(&format!("\t#[serde(rename = {member:?})]\n"));
            }
            self.out.push_str(&format!("\t{variant},\n"));
        }
        self.out.push_str("}\n");
    }

    fn rust_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("Vec<{}>", self.rust_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => match &field.typ {
                ActiveType::Null => "()".to_string(),
                ActiveType::Bool => "bool".to_string(),
                ActiveType::Int => "i64".to_string(),
                ActiveType::Float => "f64".to_string(),
                ActiveType::String | ActiveType::Secret => "String".to_string(),
                // Shared identity shape (spec/blob.md §5), emitted once
                // below as `ActiveBlob`: hash, size, media type, taint.
                ActiveType::Blob => "ActiveBlob".to_string(),
                ActiveType::Duration | ActiveType::Bytes => "u64".to_string(),
                ActiveType::Table { .. } => {
                    "std::collections::HashMap<String, serde_json::Value>".to_string()
                }
                ActiveType::Enum(_) => hint.to_string(),
                ActiveType::Any => "serde_json::Value".to_string(),
            },
            _ => "serde_json::Value".to_string(),
        }
    }

    /// Element types under the declaration hint: element tables and
    /// enums resolve to the named items `emit_nested` declares.
    fn rust_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => hint.to_string(),
            Some(Node::SchemaArray { element }) => {
                format!("Vec<{}>", self.rust_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => match &field.typ {
                ActiveType::Enum(_) => hint.to_string(),
                _ => self.rust_type(hint, id),
            },
            _ => "serde_json::Value".to_string(),
        }
    }

    fn deprecated(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => field.deprecated.clone(),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------
// Python TypedDicts (stdlib typing only; floor Python 3.11 for
// Required/NotRequired). Descriptions render as comments, formats
// and defaults as comments, requires as a comment: the types carry
// shape, Active carries verdicts.
// ----------------------------------------------------------------

/// The schema tree as Python `TypedDict` classes: nested tables become
/// nested classes, enums become `Literal`, optional fields use
/// `NotRequired`. Descriptions, formats, defaults, and `requires`
/// render as comments above each field.
pub fn schema_to_python(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = PythonGen {
        module,
        out: format!(
            "{}from typing import Any, Literal, NotRequired, TypedDict\n\n\n",
            version_header("#")
        ),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    // Shared identity shape for Blob fields (spec/blob.md §5) comes
    // FIRST: TypedDict field annotations resolve at class creation,
    // so a use-before-definition would NameError at import.
    if module.arena.iter().any(|n| {
        matches!(
            n,
            Node::SchemaField {
                field: SchemaField {
                    typ: ActiveType::Blob,
                    ..
                },
            }
        )
    }) {
        gen.out.push_str(
            "class ActiveBlob(TypedDict):\n    hash: str\n    size: int\n    media_type: str\n    tainted: bool\n\n\n",
        );
    }
    gen.emit_table(&root_name, root);
    Ok(gen.out)
}

struct PythonGen<'a> {
    module: &'a Module,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
}

/// Python string literal with escaping (members and defaults come from
/// schema authors, not a fixed dictionary: quotes, backslashes, and
/// control characters must not break the emitted module).
fn py_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Python keywords plus builtins that must not become bare field names.
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield", "match", "case", "type",
];

/// Comment-safe single line for `//` and `#` comments: a newline in a
/// description would otherwise inject an uncommented line into the output.
fn comment_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Comment-safe text for `/** */` blocks: folded to one line with the
/// block terminator neutralized, so `*/` in a description cannot close
/// the comment early.
fn ts_comment(s: &str) -> String {
    comment_line(s).replace("*/", "* /")
}

impl PythonGen<'_> {
    fn type_name(&mut self, id: NodeId, base: String) -> String {
        if let Some(name) = self.names.get(&id) {
            return name.clone();
        }
        let mut name = base;
        while !self.taken.insert(name.clone()) {
            name.push('_');
        }
        self.names.insert(id, name.clone());
        name
    }

    fn emit_table(&mut self, name: &str, id: NodeId) {
        let Some(Node::SchemaTable { fields, .. }) = self.module.get(id) else {
            return;
        };
        let fields: Vec<(String, NodeId)> = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.out.push_str(&format!("class {name}(TypedDict):\n"));
        if fields.is_empty() {
            self.out.push_str("    pass\n");
        }
        let mut taken_names = std::collections::HashSet::new();
        for (key, field) in &fields {
            let mut field_name = sanitize_ident(&key.to_lowercase(), "field");
            if PYTHON_KEYWORDS.contains(&field_name.as_str()) {
                field_name.push('_');
            }
            while !taken_names.insert(field_name.clone()) {
                field_name.push('_');
            }
            if field_name != *key {
                self.out
                    .push_str(&format!("    # key: {}\n", comment_line(key)));
            }
            if let Some(doc) = field_description(self.module, *field) {
                self.out
                    .push_str(&format!("    # {}\n", comment_line(&doc)));
            }
            if let Some(crate::ir::Node::SchemaField { field: f }) = self.module.get(*field) {
                if let Some(format) = literal_string(self.module, f.format) {
                    self.out
                        .push_str(&format!("    # format: {}\n", comment_line(&format)));
                }
                if let Some(pattern) = literal_string(self.module, f.pattern) {
                    self.out
                        .push_str(&format!("    # pattern: {}\n", comment_line(&pattern)));
                }
                if let Some(default) = literal_default(self.module, f) {
                    self.out.push_str(&format!(
                        "    # default: {}\n",
                        comment_line(&default.to_string())
                    ));
                }
                if let Some(target) = literal_string(self.module, f.requires) {
                    self.out
                        .push_str(&format!("    # requires: {}\n", comment_line(&target)));
                }
                if let Some(reason) = f.deprecated.clone() {
                    if reason.is_empty() {
                        self.out.push_str("    # Deprecated\n");
                    } else {
                        self.out
                            .push_str(&format!("    # Deprecated: {}\n", comment_line(&reason)));
                    }
                }
            }
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let mut typ = self.py_type(&tname, *field);
            if !is_required(self.module, *field) {
                typ = format!("NotRequired[{typ}]");
            }
            self.out.push_str(&format!("    {field_name}: {typ}\n"));
        }
        self.out.push('\n');
        for (key, field) in &fields {
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            self.emit_nested(&tname, *field);
        }
    }

    fn emit_nested(&mut self, name: &str, id: NodeId) {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => self.emit_table(name, id),
            Some(Node::SchemaArray { element }) => self.emit_nested(name, *element),
            _ => {}
        }
    }

    fn py_type(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => format!("\"{hint}\""),
            Some(Node::SchemaArray { element }) => {
                format!("list[{}]", self.py_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.py_field(field),
            _ => "Any".to_string(),
        }
    }

    fn py_inline(&mut self, hint: &str, id: NodeId) -> String {
        match self.module.get(id) {
            Some(Node::SchemaTable { .. }) => format!("\"{hint}\""),
            Some(Node::SchemaArray { element }) => {
                format!("list[{}]", self.py_inline(hint, *element))
            }
            Some(Node::SchemaField { field }) => self.py_field(field),
            _ => "Any".to_string(),
        }
    }

    fn py_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null => "None".to_string(),
            ActiveType::Bool => "bool".to_string(),
            ActiveType::Int | ActiveType::Duration | ActiveType::Bytes => "int".to_string(),
            ActiveType::Float => "float".to_string(),
            ActiveType::String | ActiveType::Secret => "str".to_string(),
            // Shared identity shape (spec/blob.md §5), emitted once
            // below as an `ActiveBlob` TypedDict.
            ActiveType::Blob => "ActiveBlob".to_string(),
            ActiveType::Enum(members) => {
                let quoted: Vec<String> = members.iter().map(|m| py_string(m)).collect();
                format!("Literal[{}]", quoted.join(", "))
            }
            ActiveType::Table { .. } => "dict[str, Any]".to_string(),
            ActiveType::Any => "Any".to_string(),
        }
    }
}
