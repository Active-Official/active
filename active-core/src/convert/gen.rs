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
fn literal_string(module: &Module, id: Option<NodeId>) -> Option<String> {
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
fn literal_default(module: &Module, field: &SchemaField) -> Option<JsonValue> {
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
    Ok(json_schema_node(module, root))
}

fn json_schema_node(module: &Module, id: NodeId) -> JsonValue {
    match module.get(id) {
        Some(Node::SchemaTable { fields, closed }) => {
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

/// The schema tree as TypeScript `export interface` declarations
/// (nested tables become PascalCase interfaces; `?` marks
/// defaulted/Null fields).
pub fn schema_to_typescript(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = TsGen {
        module,
        out: String::new(),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
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
                self.out.push_str(&format!("  /** {doc} */\n"));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("  /** @deprecated */\n");
                } else {
                    self.out
                        .push_str(&format!("  /** @deprecated {reason} */\n"));
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
}

// ------------------------------------------------------------------
// Go structs.
// ------------------------------------------------------------------

/// The schema tree as Go structs with `json` tags (nested tables
/// become PascalCase structs; enums stay strings with their members
/// documented).
pub fn schema_to_go(module: &Module, root_name: &str) -> Result<String, String> {
    let root = schema_root(module)?;
    if !matches!(module.get(root), Some(Node::SchemaTable { .. })) {
        return Err("schema block must be a table for code generation".to_string());
    }
    let mut gen = GoGen {
        module,
        out: String::new(),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    Ok(gen.out)
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
        for (key, field) in &fields {
            let mut field_name = pascal_case(key);
            while !taken.insert(field_name.clone()) {
                field_name.push('_');
            }
            let nominal = nominal_id(self.module, *field);
            let tname = self.type_name(nominal, format!("{name}{}", pascal_case(key)));
            let typ = self.go_type(&tname, *field);
            if let Some(doc) = field_description(self.module, *field) {
                self.out.push_str(&format!("// {doc}\n"));
            }
            if let Some(reason) = self.deprecated(*field) {
                if reason.is_empty() {
                    self.out.push_str("\t// Deprecated: no reason given.\n");
                } else {
                    self.out.push_str(&format!("\t// Deprecated: {reason}\n"));
                }
            }
            if let Some(doc) = self.doc(*field) {
                self.out.push_str(&format!("\t// {doc}\n"));
            }
            self.out
                .push_str(&format!("\t{field_name} {typ} `json:\"{key}\"`\n"));
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

    fn go_field(&self, field: &SchemaField) -> String {
        match &field.typ {
            ActiveType::Null | ActiveType::Any => "interface{}".to_string(),
            ActiveType::Bool => "bool".to_string(),
            ActiveType::Int | ActiveType::Duration | ActiveType::Bytes => "int64".to_string(),
            ActiveType::Float => "float64".to_string(),
            ActiveType::String | ActiveType::Secret | ActiveType::Enum(_) => "string".to_string(),
            ActiveType::Table { .. } => "map[string]interface{}".to_string(),
        }
    }

    fn doc(&self, id: NodeId) -> Option<String> {
        match self.module.get(id) {
            Some(Node::SchemaField { field }) => match &field.typ {
                ActiveType::Duration => Some("milliseconds".to_string()),
                ActiveType::Enum(members) => Some(format!("one of: {}", members.join(", "))),
                ActiveType::Secret => {
                    Some("secret value, redacted as ***SECRET*** on export".to_string())
                }
                _ => None,
            },
            _ => None,
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
        out: String::from(
            "// Requires: serde = { version = \"1\", features = [\"derive\"] }, serde_json = \"1\"\nuse serde::{Deserialize, Serialize};\n\n",
        ),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
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
        out: String::from("from typing import Any, Literal, NotRequired, TypedDict\n\n\n"),
        names: HashMap::new(),
        taken: HashSet::new(),
    };
    let root_name = gen.type_name(root, pascal_case(&sanitize_ident(root_name, "Config")));
    gen.emit_table(&root_name, root);
    Ok(gen.out)
}

struct PythonGen<'a> {
    module: &'a Module,
    out: String,
    names: HashMap<NodeId, String>,
    taken: HashSet<String>,
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
        for (key, field) in &fields {
            let field_name = sanitize_ident(&key.to_lowercase(), "field");
            if field_name != *key {
                self.out.push_str(&format!("    # key: {key}\n"));
            }
            if let Some(doc) = field_description(self.module, *field) {
                self.out.push_str(&format!("    # {doc}\n"));
            }
            if let Some(crate::ir::Node::SchemaField { field: f }) = self.module.get(*field) {
                if let Some(format) = literal_string(self.module, f.format) {
                    self.out.push_str(&format!("    # format: {format}\n"));
                }
                if let Some(default) = literal_default(self.module, f) {
                    self.out.push_str(&format!("    # default: {default}\n"));
                }
                if let Some(target) = literal_string(self.module, f.requires) {
                    self.out.push_str(&format!("    # requires: {target}\n"));
                }
                if let Some(reason) = f.deprecated.clone() {
                    if reason.is_empty() {
                        self.out.push_str("    # Deprecated\n");
                    } else {
                        self.out.push_str(&format!("    # Deprecated: {reason}\n"));
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
            ActiveType::Enum(members) => {
                let quoted: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
                format!("Literal[{}]", quoted.join(", "))
            }
            ActiveType::Table { .. } => "dict[str, Any]".to_string(),
            ActiveType::Any => "Any".to_string(),
        }
    }
}
