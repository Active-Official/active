//! L-P28: hermetic evaluation sandbox over an explicit input closure.
//!
//! A host builds an [`InputClosure`] from its own data, then parses and
//! evaluates through [`Sandbox`]. The sandbox replaces the ambient
//! environment with the closure's declared set, serves includes only
//! from closure bytes (digest-verified), validates the host profile
//! against the document, and refuses any read outside the closure with
//! **E048**. Legacy `Engine` constructors and `evaluate*` methods are
//! unchanged.

use crate::diag::Diagnostic;
use crate::eval::{canonical_env_name, Engine, EvalError, IncrementalMemo, StepEvent};
use crate::ir::{Module, Value};
use crate::parser::ParseOptions;
use crate::secret::SecretRedactor;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// Duplicate-key-rejecting JSON value. `serde_json::Value` keeps the last
/// duplicate; a closure manifest must never be interpreted differently by
/// an approving reader and the engine (H-2).
struct StrictJson(serde_json::Value);

impl<'de> serde::Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = serde_json::Value;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Number(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Number(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                serde_json::Number::from_f64(v)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(serde_json::Value::String(v.to_string()))
            }
            fn visit_string<E>(self, v: String) -> Result<Self::Value, E> {
                Ok(serde_json::Value::String(v))
            }
            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Null)
            }
            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(serde_json::Value::Null)
            }
            fn visit_some<D>(self, d: D) -> Result<Self::Value, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                d.deserialize_any(Visitor)
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut out = Vec::new();
                while let Some(StrictJson(value)) = seq.next_element()? {
                    out.push(value);
                }
                Ok(serde_json::Value::Array(out))
            }
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut out = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key `{key}`"
                        )));
                    }
                    let StrictJson(value) = map.next_value()?;
                    out.insert(key, value);
                }
                Ok(serde_json::Value::Object(out))
            }
        }
        deserializer.deserialize_any(Visitor).map(StrictJson)
    }
}

/// Exact include bytes plus their SHA-256 digest.
#[derive(Clone)]
pub struct IncludeFile {
    pub bytes: Arc<str>,
    pub sha256: [u8; 32],
}

impl fmt::Debug for IncludeFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IncludeFile")
            .field("bytes", &"<redacted>")
            .field("sha256", &"<redacted>")
            .finish()
    }
}

/// Everything a sandboxed evaluation may read. Built by the host; the
/// engine never touches a filesystem, network, clock, or ambient env
/// after this value exists.
#[derive(Clone)]
pub struct InputClosure {
    pub document: Arc<str>,
    pub source_name: String,
    /// Root-relative include identities (`/` separators) to exact bytes.
    pub includes: BTreeMap<String, IncludeFile>,
    /// Every env name the document may read. `None` is an explicit
    /// declared-absent input.
    pub env: BTreeMap<String, Option<String>>,
    /// Host profile selection; checked against the document after parse.
    pub profile_override: Option<String>,
}

impl fmt::Debug for InputClosure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputClosure")
            .field("document", &"<redacted>")
            .field("source_name", &self.source_name)
            .field("includes", &self.includes.len())
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("profile_override", &self.profile_override)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosureError {
    IncludeKey(String),
    IncludeDigest(String),
    IncludeNotUtf8(String),
    EnvName(String),
    DuplicateEnv(String),
    Manifest(String),
    Size(String),
}

impl fmt::Display for ClosureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClosureError::IncludeKey(key) => write!(f, "invalid include key `{key}`"),
            ClosureError::IncludeDigest(key) => write!(f, "include digest mismatch for `{key}`"),
            ClosureError::IncludeNotUtf8(key) => write!(f, "include `{key}` is not UTF-8"),
            ClosureError::EnvName(name) => write!(f, "invalid env name `{name}`"),
            ClosureError::DuplicateEnv(name) => {
                write!(f, "duplicate env name after folding: `{name}`")
            }
            ClosureError::Manifest(message) => write!(f, "invalid closure manifest: {message}"),
            ClosureError::Size(what) => write!(f, "closure `{what}` exceeds the size cap"),
        }
    }
}

impl std::error::Error for ClosureError {}

impl InputClosure {
    /// Validate and canonicalize a closure. Include bytes are `str` by
    /// construction; digests are checked here and again by the loader.
    pub fn new(
        document: impl Into<Arc<str>>,
        source_name: impl Into<String>,
        includes: BTreeMap<String, IncludeFile>,
        env: BTreeMap<String, Option<String>>,
        profile_override: Option<String>,
    ) -> Result<Self, ClosureError> {
        const MAX_CLOSURE_BYTES: usize = 16 * 1024 * 1024;
        let document: Arc<str> = document.into();
        if document.len() > MAX_CLOSURE_BYTES {
            return Err(ClosureError::Size("document".into()));
        }
        let mut normalized_includes = BTreeMap::new();
        let mut total = 0usize;
        for (key, file) in includes {
            let key = normalize_include_key(&key)?;
            total = total.saturating_add(file.bytes.len());
            if total > MAX_CLOSURE_BYTES {
                return Err(ClosureError::Size("includes".into()));
            }
            let digest = sha256(file.bytes.as_bytes());
            if digest != file.sha256 {
                return Err(ClosureError::IncludeDigest(key));
            }
            if normalized_includes.contains_key(&key) {
                return Err(ClosureError::Manifest(format!(
                    "duplicate normalized include `{key}`"
                )));
            }
            normalized_includes.insert(key, file);
        }
        let mut canonical_env = BTreeMap::new();
        for (name, value) in env {
            if name.is_empty() {
                return Err(ClosureError::EnvName(name));
            }
            let folded = canonical_env_name(&name);
            if canonical_env.insert(folded.clone(), value).is_some() {
                return Err(ClosureError::DuplicateEnv(folded));
            }
        }
        Ok(Self {
            document,
            source_name: source_name.into(),
            includes: normalized_includes,
            env: canonical_env,
            profile_override,
        })
    }

    /// Build a closure from the §7.2a JSON manifest. `document` is the
    /// host document when `document_b64` is absent. `read_include` is the
    /// host's include reader, called only when `bytes_b64` is absent.
    pub fn from_manifest(
        json: &str,
        document: Option<&str>,
        mut read_include: impl FnMut(&str) -> Result<String, String>,
    ) -> Result<Self, ClosureError> {
        use base64::Engine as _;
        let StrictJson(value): StrictJson = serde_json::from_str(json)
            .map_err(|error| ClosureError::Manifest(error.to_string()))?;
        let object = value
            .as_object()
            .ok_or_else(|| ClosureError::Manifest("manifest must be an object".into()))?;
        for key in object.keys() {
            if !matches!(
                key.as_str(),
                "version" | "document_b64" | "source_name" | "includes" | "env" | "profile"
            ) {
                return Err(ClosureError::Manifest(format!("unknown field `{key}`")));
            }
        }
        if object.get("version").and_then(|v| v.as_u64()) != Some(1) {
            return Err(ClosureError::Manifest("version must be 1".into()));
        }
        if object.get("document_b64").is_some_and(|v| !v.is_string()) {
            return Err(ClosureError::Manifest(
                "document_b64 must be a string".into(),
            ));
        }
        if object.get("source_name").is_some_and(|v| !v.is_string()) {
            return Err(ClosureError::Manifest(
                "source_name must be a string".into(),
            ));
        }
        if object.get("profile").is_some_and(|v| !v.is_string()) {
            return Err(ClosureError::Manifest("profile must be a string".into()));
        }
        for section in ["includes", "env"] {
            if object.get(section).is_some_and(|v| !v.is_array()) {
                return Err(ClosureError::Manifest(format!(
                    "{section} must be an array"
                )));
            }
        }
        let document = match value.get("document_b64").and_then(|v| v.as_str()) {
            Some(encoded) => {
                if encoded.len() > 24 * 1024 * 1024 {
                    return Err(ClosureError::Size("document_b64".into()));
                }
                String::from_utf8(
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|error| ClosureError::Manifest(error.to_string()))?,
                )
                .map_err(|_| ClosureError::Manifest("document_b64 is not UTF-8".into()))?
            }
            None => document
                .ok_or_else(|| ClosureError::Manifest("document is missing".into()))?
                .to_string(),
        };
        let source_name = value
            .get("source_name")
            .and_then(|v| v.as_str())
            .unwrap_or("main.active")
            .to_string();
        let mut includes = BTreeMap::new();
        if let Some(items) = value.get("includes").and_then(|v| v.as_array()) {
            if items.len() > 256 {
                return Err(ClosureError::Size("includes".into()));
            }
            for item in items {
                for field in item.as_object().map(|o| o.keys()).into_iter().flatten() {
                    if !matches!(field.as_str(), "path" | "sha256" | "bytes_b64") {
                        return Err(ClosureError::Manifest(format!(
                            "unknown include field `{field}`"
                        )));
                    }
                }
                let path = item
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ClosureError::Manifest("include path missing".into()))?;
                let key = normalize_include_key(path)?;
                if includes.contains_key(&key) {
                    return Err(ClosureError::Manifest(format!("duplicate include `{key}`")));
                }
                let digest = item
                    .get("sha256")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ClosureError::Manifest("include sha256 missing".into()))?;
                let sha256 = parse_hex_32(digest)
                    .ok_or_else(|| ClosureError::Manifest("include sha256 is not 64 hex".into()))?;
                if item.get("bytes_b64").is_some_and(|v| !v.is_string()) {
                    return Err(ClosureError::Manifest(format!(
                        "include `{path}` bytes_b64 must be a string"
                    )));
                }
                let bytes = match item.get("bytes_b64").and_then(|v| v.as_str()) {
                    Some(encoded) => {
                        if encoded.len() > 24 * 1024 * 1024 {
                            return Err(ClosureError::Size("includes".into()));
                        }
                        String::from_utf8(
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|error| ClosureError::Manifest(error.to_string()))?,
                        )
                        .map_err(|_| {
                            ClosureError::Manifest(format!("include `{path}` is not UTF-8"))
                        })?
                    }
                    None => read_include(&key).map_err(ClosureError::Manifest)?,
                };
                includes.insert(
                    key,
                    IncludeFile {
                        bytes: Arc::from(bytes),
                        sha256,
                    },
                );
            }
        }
        let mut env = BTreeMap::new();
        if let Some(items) = value.get("env").and_then(|v| v.as_array()) {
            if items.len() > 256 {
                return Err(ClosureError::Size("env".into()));
            }
            for item in items {
                for field in item.as_object().map(|o| o.keys()).into_iter().flatten() {
                    if !matches!(field.as_str(), "name" | "set" | "value") {
                        return Err(ClosureError::Manifest(format!(
                            "unknown env field `{field}`"
                        )));
                    }
                }
                let name = item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ClosureError::Manifest("env name missing".into()))?;
                if item.get("value").is_some_and(|v| !v.is_string()) {
                    return Err(ClosureError::Manifest(format!(
                        "env `{name}` value must be a string"
                    )));
                }
                if item.get("set").is_some_and(|v| !v.is_boolean()) {
                    return Err(ClosureError::Manifest(format!(
                        "env `{name}` set must be a boolean"
                    )));
                }
                if item
                    .get("value")
                    .and_then(|v| v.as_str())
                    .is_some_and(|value| value.len() > 16 * 1024 * 1024)
                {
                    return Err(ClosureError::Size("env value".into()));
                }
                let set = item.get("set").and_then(|v| v.as_bool()).unwrap_or(false);
                let value = if set {
                    Some(
                        item.get("value")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                ClosureError::Manifest(format!(
                                    "env `{name}` is set without a value"
                                ))
                            })?
                            .to_string(),
                    )
                } else {
                    None
                };
                if env.contains_key(name) {
                    return Err(ClosureError::Manifest(format!("duplicate env `{name}`")));
                }
                env.insert(name.to_string(), value);
            }
        }
        let profile = value
            .get("profile")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        Self::new(document, source_name, includes, env, profile)
    }

    /// Declared env map. A name present with `None` is explicitly absent;
    /// an adapter must not fall back to the process environment for it.
    pub fn env(&self) -> &BTreeMap<String, Option<String>> {
        &self.env
    }

    /// One declared env value, distinguishing "absent input" from
    /// "not declared".
    pub fn env_value(&self, name: &str) -> Option<Option<&str>> {
        self.env
            .get(&crate::eval::canonical_env_name(name))
            .map(|value| value.as_deref())
    }

    /// Deterministic closure identity (internal proof and memo token).
    pub fn id(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"active-sandbox-closure/v1");
        hasher.update(&[0u8]);
        put_str(&mut hasher, &self.document);
        put_str(&mut hasher, &self.source_name);
        hasher.update(&(self.includes.len() as u64).to_le_bytes());
        for (key, file) in &self.includes {
            put_str(&mut hasher, key);
            hasher.update(&file.sha256);
        }
        hasher.update(&(self.env.len() as u64).to_le_bytes());
        for (name, value) in &self.env {
            put_str(&mut hasher, name);
            match value {
                Some(value) => {
                    hasher.update(&[1u8]);
                    put_str(&mut hasher, value);
                }
                None => {
                    hasher.update(&[0u8]);
                }
            }
        }
        match &self.profile_override {
            Some(profile) => {
                hasher.update(&[1u8]);
                put_str(&mut hasher, profile);
            }
            None => {
                hasher.update(&[0u8]);
            }
        }
        *hasher.finalize().as_bytes()
    }
}

fn normalize_include_key(key: &str) -> Result<String, ClosureError> {
    if key.contains('\\')
        || key.starts_with('/')
        || key.starts_with("//")
        || key.contains(':')
        || key.len() > 1024
        || key.chars().any(|ch| ch.is_control())
    {
        return Err(ClosureError::IncludeKey(key.to_string()));
    }
    let mut segments = Vec::new();
    for segment in key.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return Err(ClosureError::IncludeKey(key.to_string())),
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(ClosureError::IncludeKey(key.to_string()));
    }
    Ok(segments.join("/"))
}

fn parse_hex_32(text: &str) -> Option<[u8; 32]> {
    if !text.is_ascii() || text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn put_str(hasher: &mut blake3::Hasher, text: &str) {
    hasher.update(&(text.len() as u64).to_le_bytes());
    hasher.update(text.as_bytes());
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// Closure guard attached to the engine clone; set only by [`Sandbox`].
#[derive(Clone)]
pub(crate) struct SandboxGuard {
    pub env_names: BTreeSet<String>,
    pub env_values: BTreeMap<String, String>,
    pub profile: Option<String>,
}

/// A validated closure.
#[derive(Clone)]
pub struct Sandbox {
    closure: Arc<InputClosure>,
    id: [u8; 32],
}

impl fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sandbox")
            .field("closure", &"<redacted>")
            .field("id", &"<redacted>")
            .finish()
    }
}

/// Parsed under a sandbox closure. The inner module is private, so a
/// foreign module can never enter sandbox mode.
pub struct SandboxedModule {
    module: Module,
    id: [u8; 32],
}

impl fmt::Debug for SandboxedModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxedModule")
            .field("module", &"<redacted>")
            .field("id", &"<redacted>")
            .finish()
    }
}

// The root filename must not be expressible as an include key (a dot
// segment normalizes away), or an include literally named `main.active`
// would alias the document identity (Part 0 review F2).
const SYNTHETIC_ROOT: &str = "/__active_sandbox__/.active-root";

impl Sandbox {
    pub fn new(mut closure: InputClosure) -> Result<Self, ClosureError> {
        // `InputClosure` fields are public so a host can build one field by
        // field; revalidate and canonicalize everything except the digest,
        // which the loader deliberately re-checks at parse time (E048) so
        // the relay's literal-digest test can reach the loader branch.
        let mut canonical_includes = BTreeMap::new();
        for (key, file) in std::mem::take(&mut closure.includes) {
            let key = normalize_include_key(&key)?;
            if canonical_includes.insert(key.clone(), file).is_some() {
                return Err(ClosureError::Manifest(format!(
                    "duplicate include key `{key}`"
                )));
            }
        }
        closure.includes = canonical_includes;
        let mut canonical_env = BTreeMap::new();
        for (name, value) in std::mem::take(&mut closure.env) {
            let folded = canonical_env_name(&name);
            if canonical_env.insert(folded.clone(), value).is_some() {
                return Err(ClosureError::Manifest(format!(
                    "duplicate env name after folding: `{folded}`"
                )));
            }
        }
        closure.env = canonical_env;
        if closure.document.len() > 16 * 1024 * 1024 {
            return Err(ClosureError::Size("document".into()));
        }
        let mut total = 0usize;
        let mut keys = BTreeSet::new();
        for (key, file) in &closure.includes {
            let normalized = normalize_include_key(key)?;
            if !keys.insert(normalized) {
                return Err(ClosureError::Manifest("duplicate include key".into()));
            }
            total = total.saturating_add(file.bytes.len());
            if total > 16 * 1024 * 1024 {
                return Err(ClosureError::Size("includes".into()));
            }
        }
        let mut env_names = BTreeSet::new();
        for name in closure.env.keys() {
            if !env_names.insert(canonical_env_name(name)) {
                return Err(ClosureError::Manifest(format!(
                    "duplicate env name after folding: `{name}`"
                )));
            }
        }
        let id = closure.id();
        Ok(Self {
            closure: Arc::new(closure),
            id,
        })
    }

    pub fn id(&self) -> [u8; 32] {
        self.id
    }

    /// Closure host profile, when one was selected.
    pub fn profile_override(&self) -> Option<&str> {
        self.closure.profile_override.as_deref()
    }

    /// Declared env map (forwarded from the closure).
    pub fn env(&self) -> &BTreeMap<String, Option<String>> {
        self.closure.env()
    }

    /// One declared env value (forwarded from the closure).
    pub fn env_value(&self, name: &str) -> Option<Option<&str>> {
        self.closure.env_value(name)
    }

    /// Parse the closure document. Every closure-bound parse field is set
    /// here; the caller's options supply only healing and parse budgets.
    pub fn parse(&self, options: ParseOptions) -> Result<SandboxedModule, Vec<Diagnostic>> {
        let mut options = options;
        options.enable_includes = true;
        options.resolve_includes = true;
        options.allow_parent_escapes = false;
        options.include_lock = None;
        options.include_store = None;
        options.include_error_code = Some("E048");
        let includes = self.closure.includes.clone();
        options.include_loader = Some(Arc::new(move |path: &str| {
            let key = include_key_from_callback(path);
            // The synthetic root is not an include. A path resolving to
            // it would alias the document identity; refuse before bytes
            // are read.
            if key == ".active-root" {
                return Err("include path aliases the sandbox root".to_string());
            }
            let file = includes
                .get(&key)
                .ok_or_else(|| format!("include `{key}` is outside the input closure"))?;
            let digest = sha256(file.bytes.as_bytes());
            if digest != file.sha256 {
                return Err(format!("include `{key}` digest mismatch"));
            }
            Ok(file.bytes.to_string())
        }));
        let mut module = match crate::parse_text_with_file(
            &self.closure.document,
            &self.closure.source_name,
            options,
            Path::new(SYNTHETIC_ROOT),
        ) {
            Ok(module) => module,
            Err(mut diagnostics) => {
                for diagnostic in &mut diagnostics {
                    if let Some(file) = diagnostic.file.as_mut() {
                        *file = self.closure_identity(file);
                    }
                }
                return Err(diagnostics);
            }
        };
        self.rebrand_files(&mut module);
        if let Some(profile) = &self.closure.profile_override {
            if !module.profiles.contains_key(profile) {
                let diagnostic = Diagnostic {
                    severity: crate::diag::Severity::Error,
                    code: "E048".into(),
                    message: format!("sandbox profile `{profile}` is not declared by the document"),
                    path: Some("active_profile".into()),
                    span: None,
                    repair: None,
                    confidence: 1.0,
                    file: None,
                };
                return Err(vec![diagnostic]);
            }
        }
        Ok(SandboxedModule {
            module,
            id: self.id,
        })
    }

    fn checked_module<'a>(&self, module: &'a SandboxedModule) -> Result<&'a Module, EvalError> {
        if module.id != self.id {
            return Err(EvalError::SandboxRefused {
                message: "sandboxed module belongs to a different closure".into(),
                span: None,
                file: None,
            });
        }
        Ok(&module.module)
    }

    /// Replace synthetic/drive-qualified root paths with closure
    /// identities in every observable provenance surface, so diagnostics,
    /// pins, and comment attachment are host-independent.
    fn closure_identity(&self, path: &str) -> String {
        let normalized = path.replace('\\', "/");
        let marker = "/__active_sandbox__/";
        if let Some(index) = normalized.find(marker) {
            let tail = &normalized[index + marker.len()..];
            if tail == ".active-root" {
                return self.closure.source_name.clone();
            }
            return format!("include:{tail}");
        }
        if normalized == SYNTHETIC_ROOT {
            return self.closure.source_name.clone();
        }
        path.to_string()
    }

    fn rebrand_files(&self, module: &mut Module) {
        for stamp in module.node_files.values_mut() {
            *stamp = self.closure_identity(stamp);
        }
        for diagnostic in module
            .diagnostics
            .iter_mut()
            .chain(module.healing.iter_mut())
        {
            if let Some(file) = diagnostic.file.as_mut() {
                *file = self.closure_identity(file);
            }
        }
        for comment in &mut module.trivia {
            if let Some(file) = comment.file.as_mut() {
                *file = self.closure_identity(file);
            }
        }
        for pin in &mut module.include_pins {
            pin.stamp = self.closure_identity(&pin.stamp);
            pin.resolved = self.closure_identity(&pin.resolved);
        }
        for (pin, _) in &mut module.include_resolutions {
            pin.stamp = self.closure_identity(&pin.stamp);
            pin.resolved = self.closure_identity(&pin.resolved);
        }
    }

    /// Apply the closure to an existing engine: closure env/profile and
    /// the private guard, with the fallback refusal. Public so a CLI
    /// command adapter can configure its own engine; the guard still
    /// only becomes meaningful through a `Sandbox`.
    pub fn configure_engine(&self, engine: &mut Engine) -> Result<(), EvalError> {
        let guarded = self.guarded_engine(engine)?;
        *engine = guarded;
        Ok(())
    }

    /// Parse and return the module. Evaluation still goes through
    /// `Sandbox`; this accessor exists for command adapters that need
    /// the arena (graph queries, lock, audit).
    pub fn parse_module(&self, options: ParseOptions) -> Result<Module, Vec<Diagnostic>> {
        self.parse(options).map(|module| module.module().clone())
    }

    /// The guarded engine: caller config, closure env/profile, no fallback.
    fn guarded_engine(&self, engine: &Engine) -> Result<Engine, EvalError> {
        let mut guarded = engine.clone();
        guarded.env = self
            .closure
            .env
            .iter()
            .filter_map(|(name, value)| value.clone().map(|value| (name.clone(), value)))
            .collect();
        guarded.profile_override = self.closure.profile_override.clone();
        guarded.sandbox_guard = Some(SandboxGuard {
            env_names: self.closure.env.keys().cloned().collect(),
            env_values: self
                .closure
                .env
                .iter()
                .filter_map(|(name, value)| value.clone().map(|value| (name.clone(), value)))
                .collect(),
            profile: self.closure.profile_override.clone(),
        });
        Ok(guarded)
    }

    pub fn evaluate(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        self.guarded_engine(engine)?
            .evaluate(self.checked_module(module)?)
    }

    /// Evaluate and redact through the real sandboxed module, so material
    /// paths and blob identities are known exactly (FFI/WASM boundary).
    /// Returns the sealed [`crate::convert::RedactedValue`], so the host
    /// that receives it cannot hand a raw tree to a generic serializer or
    /// a materialised exporter (L-P36).
    pub fn evaluate_redacted(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> Result<(crate::convert::RedactedValue, Vec<Diagnostic>), EvalError> {
        let checked = self.checked_module(module)?;
        let guarded = self.guarded_engine(engine)?;
        let (value, diagnostics, seeds) = guarded.evaluate_with_seeds(checked)?;
        let profile = crate::secret::SecretProfile::for_engine(&module.module, &guarded, &seeds);
        if profile.truncated() {
            return Err(EvalError::SandboxRefused {
                message: "sandbox secret analysis hit its analysis bounds; refusing to emit a possibly over-redacted tree".into(),
                span: None,
                file: None,
            });
        }
        Ok((crate::convert::redact_value(&profile, &value), diagnostics))
    }

    pub fn evaluate_with_seeds(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> Result<(Value, Vec<Diagnostic>, SecretRedactor), EvalError> {
        self.guarded_engine(engine)?
            .evaluate_with_seeds(self.checked_module(module)?)
    }

    pub fn evaluate_collecting_seeds(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> (Result<(Value, Vec<Diagnostic>), EvalError>, SecretRedactor) {
        match self.guarded_engine(engine) {
            Ok(engine) => match self.checked_module(module) {
                Ok(checked) => engine.evaluate_collecting_seeds(checked),
                Err(error) => (Err(error), SecretRedactor::new()),
            },
            Err(error) => (Err(error), SecretRedactor::new()),
        }
    }

    pub fn evaluate_with_steps(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> Result<(Value, Vec<Diagnostic>, Vec<StepEvent>), EvalError> {
        self.guarded_engine(engine)?
            .evaluate_with_steps(self.checked_module(module)?)
    }

    pub fn evaluate_with_steps_and_seeds(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
    ) -> Result<(Value, Vec<Diagnostic>, Vec<StepEvent>, SecretRedactor), EvalError> {
        self.guarded_engine(engine)?
            .evaluate_with_steps_and_seeds(self.checked_module(module)?)
    }

    pub fn evaluate_incremental(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
        memo: &mut IncrementalMemo,
    ) -> Result<(Value, Vec<Diagnostic>), EvalError> {
        self.guarded_engine(engine)?
            .evaluate_incremental(self.checked_module(module)?, memo)
    }

    /// Feature hatch for a helper that needs `&Engine` + `&Module`
    /// (repair, debug steps, inspect): the guarded engine and the
    /// wrapped module are handed to the callback, and any evaluation it
    /// triggers still passes the closure guard.
    pub fn with_engine<T>(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
        callback: impl FnOnce(&Engine, &Module) -> T,
    ) -> Result<T, EvalError> {
        if module.id != self.id {
            return Err(EvalError::SandboxRefused {
                message: "sandboxed module belongs to a different closure".into(),
                span: None,
                file: None,
            });
        }
        let guarded = self.guarded_engine(engine)?;
        Ok(callback(&guarded, &module.module))
    }

    pub fn evaluate_incremental_with_seeds(
        &self,
        engine: &Engine,
        module: &SandboxedModule,
        memo: &mut IncrementalMemo,
    ) -> Result<(Value, Vec<Diagnostic>, SecretRedactor), EvalError> {
        self.guarded_engine(engine)?
            .evaluate_incremental_with_seeds(self.checked_module(module)?, memo)
    }
}

impl SandboxedModule {
    pub fn closure_id(&self) -> [u8; 32] {
        self.id
    }

    /// The parsed module. Analysis surfaces that need to inspect the
    /// arena (LSP hover, steps) read it here; evaluation still goes
    /// through `Sandbox` so the closure guard applies.
    pub fn module(&self) -> &Module {
        &self.module
    }

    /// Parse/healing diagnostics captured while the closure document was
    /// parsed (LSP publishes these).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.module.diagnostics
    }

    pub fn healing(&self) -> &[Diagnostic] {
        &self.module.healing
    }
}

fn include_key_from_callback(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if let Some(index) = normalized.find("/__active_sandbox__/") {
        return normalized[index + "/__active_sandbox__/".len()..].to_string();
    }
    normalized.trim_start_matches("./").to_string()
}
