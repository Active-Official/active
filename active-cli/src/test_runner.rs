//! `active test` (L-P20): one pass over schema validation, lint,
//! fmt-check, and conformance vectors, with one exit code.
//!
//! No external test framework: the command walks `.active` files with
//! std, reuses the shipped parser/engine/linter/formatter, and runs the
//! repository conformance manifest through the same core APIs.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use active_core::diag::Severity;
use active_core::lint;
use active_core::{fmt, Engine, ParseOptions, Style};

/// The authority this process holds (`--capability`), so `active test`
/// evaluates with the same narrow set as every other surface instead of
/// always holding all five authorities (L-P35 review H2).
fn held_authority() -> active_core::CapabilitySet {
    crate::HELD_CAPABILITIES
        .get()
        .copied()
        .unwrap_or_else(active_core::CapabilitySet::all)
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    pub files: usize,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub cases: usize,
    pub case_failures: usize,
}

/// Run every requested check. `paths` may be files or directories; a
/// directory is walked for `.active` files (hidden/target/node_modules
/// subtrees are skipped, and the `conformance` subtree is skipped when
/// this run owns its manifest).
pub(crate) fn run(
    paths: &[PathBuf],
    manifest: Option<&Path>,
    strict: bool,
    env: &HashMap<String, String>,
) -> anyhow::Result<Report> {
    let mut report = Report::default();
    let mut manifests: Vec<PathBuf> = Vec::new();
    if let Some(path) = manifest {
        manifests.push(path.to_path_buf());
    }
    if manifest.is_none() {
        for path in paths {
            let candidate = path.join("conformance").join("manifest.toml");
            if path.is_dir() && candidate.is_file() && !manifests.contains(&candidate) {
                manifests.push(candidate);
            }
        }
        if paths.is_empty() {
            let candidate = PathBuf::from("conformance").join("manifest.toml");
            if candidate.is_file() {
                manifests.push(candidate);
            } else {
                anyhow::bail!(
                    "nothing to test: pass files/directories or --conformance <manifest>"
                );
            }
        }
    }
    // Only the conformance directory OWNED by a manifest is skipped; an
    // unowned or nested `conformance` directory is ordinary source.
    let owned_roots: Vec<PathBuf> = manifests
        .iter()
        .filter_map(|manifest| manifest.parent().map(Path::to_path_buf))
        .collect();

    let mut files = BTreeSet::new();
    for path in paths {
        if path.is_file() {
            files.insert(path.clone());
        } else if path.is_dir() {
            collect_active(path, 0, &owned_roots, &mut files)?;
        } else {
            anyhow::bail!("{} does not exist", path.display());
        }
    }
    for file in &files {
        report.files += 1;
        check_file(file, strict, env, &mut report);
    }
    for manifest in &manifests {
        run_conformance(manifest, &mut report);
    }
    if report.files == 0 && report.cases == 0 {
        report
            .failures
            .push("no checks ran: no .active files and no conformance cases".to_string());
    }
    Ok(report)
}

fn collect_active(
    dir: &Path,
    depth: usize,
    owned_roots: &[PathBuf],
    files: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    if depth > 64 {
        anyhow::bail!("directory nesting exceeds 64 at {}", dir.display());
    }
    if owned_roots.iter().any(|root| same_dir(root, dir)) {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| anyhow::anyhow!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|e| anyhow::anyhow!("cannot inspect {}: {e}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if name.starts_with('.')
                || matches!(name.as_ref(), "target" | "node_modules")
                || owned_roots.iter().any(|root| same_dir(root, &path))
            {
                continue;
            }
            collect_active(&path, depth + 1, owned_roots, files)?;
        } else if metadata.is_file()
            && !name.starts_with('.')
            && path.extension().is_some_and(|e| e == "active")
        {
            files.insert(path);
        }
    }
    Ok(())
}

/// Compare directories across relative/absolute spellings.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// One check's result for a single file (L-P62). `active test` renders
/// these as its failure and note strings; `active verify` renders the
/// same findings as structured data, so the two cannot disagree about a
/// file.
#[derive(Debug, Clone)]
pub(crate) struct Finding {
    /// Stage key: `read`, `size`, `parse`, `repair`, `fmt`,
    /// `fmt-refused`, `lint-config`, `lint`, `schema`, `schema-fatal`.
    pub check: &'static str,
    pub fatal: bool,
    pub codes: Vec<String>,
    /// Diagnostic count behind a note (lint findings, schema warnings).
    pub count: usize,
    /// Value-free detail text (an I/O error, a formatter refusal, or a
    /// fatal evaluation code).
    pub detail: Option<String>,
}

impl Finding {
    fn fatal(check: &'static str, codes: Vec<String>) -> Self {
        Self {
            check,
            fatal: true,
            codes,
            count: 0,
            detail: None,
        }
    }

    fn detail(check: &'static str, detail: impl Into<String>, fatal: bool) -> Self {
        Self {
            check,
            fatal,
            codes: Vec::new(),
            count: 0,
            detail: Some(detail.into()),
        }
    }

    fn note(check: &'static str, count: usize, codes: Vec<String>) -> Self {
        Self {
            check,
            fatal: false,
            codes,
            count,
            detail: None,
        }
    }

    /// Severity reported to `active verify` consumers.
    pub fn severity(&self) -> Severity {
        if self.fatal {
            Severity::Error
        } else {
            Severity::Warning
        }
    }
}

/// The full verdict for one file: every stage's findings, in stage
/// order, plus what the caller needs to bind a receipt (the parsed
/// module and the one evaluation this verdict already ran — no caller
/// re-evaluates).
#[derive(Debug, Default)]
pub(crate) struct Verdict {
    pub source: Option<String>,
    pub module: Option<active_core::Module>,
    pub findings: Vec<Finding>,
    /// Evaluation result, when it ran and succeeded.
    pub evaluated: Option<(active_core::Value, Vec<active_core::Diagnostic>)>,
    /// Evaluation failure (fatal code or validation diagnostics), when
    /// it ran and failed.
    pub eval_failure: Option<(String, Vec<active_core::Diagnostic>)>,
    /// The engine's environment and budget, so a receipt binds exactly
    /// what the verdict ran under.
    pub env: std::collections::HashMap<String, String>,
    pub budget: active_core::eval::Budget,
}

impl Verdict {
    pub fn clean(&self) -> bool {
        !self.findings.iter().any(|finding| finding.fatal)
    }
}

/// Run the shipped per-file checks and collect structured findings.
/// This is the one implementation behind `active test` and
/// `active verify`; the stages, options, and severity rules are the
/// tooling contract (docs/tooling.md).
pub(crate) fn verdict(
    path: &Path,
    strict: bool,
    engine: &Engine,
    trust: Option<&active_core::sign::TrustedKeys>,
    allow_parent_escapes: bool,
    lock: Option<&active_core::provenance::IncludeLock>,
) -> Verdict {
    // Read with a hard cap: a file that grows (or is swapped) between a
    // stat and the read still cannot allocate past the limit.
    let options = crate::with_parse_limits(ParseOptions::default());
    let cap = if options.max_input_bytes == 0 {
        usize::MAX
    } else {
        options.max_input_bytes
    };
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(e) => {
            let mut out = Verdict::default();
            out.findings
                .push(Finding::detail("read", e.to_string(), true));
            return out;
        }
    };
    if !metadata.is_file() {
        let mut out = Verdict::default();
        out.findings
            .push(Finding::detail("read", "not a regular file", true));
        return out;
    }
    if metadata.len() > cap as u64 {
        let mut out = Verdict::default();
        out.findings.push(Finding::detail(
            "size",
            format!("input is {} bytes; the limit is {cap}", metadata.len()),
            true,
        ));
        return out;
    }
    let mut src = String::new();
    {
        use std::io::Read;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(e) => {
                let mut out = Verdict::default();
                out.findings
                    .push(Finding::detail("read", e.to_string(), true));
                return out;
            }
        };
        if let Err(e) = file
            .take(cap.saturating_add(1) as u64)
            .read_to_string(&mut src)
        {
            let mut out = Verdict::default();
            out.findings
                .push(Finding::detail("read", e.to_string(), true));
            return out;
        }
    }
    verdict_from(
        path,
        &src,
        strict,
        engine,
        trust,
        allow_parent_escapes,
        lock,
    )
}

/// The same verdict over caller-supplied source text (the propose loop
/// keeps the candidate in memory: no path read, so no swap can change
/// what is verified). `path` still anchors include resolution and lint
/// discovery.
pub(crate) fn verdict_from(
    path: &Path,
    src: &str,
    strict: bool,
    engine: &Engine,
    trust: Option<&active_core::sign::TrustedKeys>,
    allow_parent_escapes: bool,
    lock: Option<&active_core::provenance::IncludeLock>,
) -> Verdict {
    let mut out = Verdict::default();
    let options = crate::with_parse_limits(ParseOptions::default());
    let cap = if options.max_input_bytes == 0 {
        usize::MAX
    } else {
        options.max_input_bytes
    };
    if src.len() > cap {
        out.findings.push(Finding::detail(
            "size",
            format!("input is {} bytes; the limit is {cap}", src.len()),
            true,
        ));
        return out;
    }
    out.source = Some(src.to_string());
    out.env = engine.env.clone();
    out.budget = engine.budget;
    let name = path.to_string_lossy();
    // Syntax-only module for fmt (the formatter never merges includes),
    // and the resolved module for lint/eval.
    let syntax_options = crate::with_parse_limits(ParseOptions {
        resolve_includes: false,
        allow_parent_escapes,
        ..ParseOptions::default()
    });
    let syntax = match active_core::parser::parse_text_with_file(src, &name, syntax_options, path) {
        Ok(module) => module,
        Err(diags) => {
            out.findings.push(Finding::fatal("parse", codes(&diags)));
            return out;
        }
    };
    if !syntax.healing.is_empty() {
        out.findings
            .push(Finding::fatal("repair", codes(&syntax.healing)));
    } else {
        // The canonical check uses the same entry point as `fmt --write`,
        // so `active test` never advises a rewrite (or accepts a file)
        // that the formatter would refuse.
        match fmt::try_format(&syntax, Style::Indent) {
            Ok(formatted) if formatted != src => {
                out.findings.push(Finding::fatal("fmt", Vec::new()));
            }
            Ok(_) => {}
            Err(error) => {
                out.findings
                    .push(Finding::detail("fmt-refused", error.to_string(), true));
            }
        }
    }
    let options = ParseOptions {
        allow_parent_escapes,
        include_lock: lock.cloned(),
        ..options
    };
    // With a trust file the resolved parse runs the shipped file path
    // (signature and lockfile checks on includes); without one it parses
    // the buffer we already read.
    let parsed = match trust {
        // Verify the bytes we hold: never re-open the path (a swap
        // between read and parse would otherwise decide the verdict).
        Some(keys) => active_core::parser::parse_text_with_trust(src, &name, options, path, keys),
        None => active_core::parser::parse_text_with_file(src, &name, options, path),
    };
    let module = match parsed {
        Ok(module) => module,
        Err(diags) => {
            out.findings.push(Finding::fatal("parse", codes(&diags)));
            return out;
        }
    };
    // Project lint policy travels with the file, exactly as `active
    // lint` applies it.
    let lint_options = match lint::LintConfig::discover(path) {
        Ok(Some((_, config))) => match config.options(&[]) {
            Ok(options) => options,
            Err(e) => {
                out.findings
                    .push(Finding::detail("lint-config", e.to_string(), true));
                return out;
            }
        },
        Ok(None) => lint::LintOptions::default(),
        Err(e) => {
            out.findings
                .push(Finding::detail("lint-config", e.to_string(), true));
            return out;
        }
    };
    let findings = lint::lint_with(&module, &lint_options);
    let lint_fatal = findings
        .iter()
        .any(|d| d.severity == Severity::Error || (strict && d.severity == Severity::Warning));
    if lint_fatal {
        out.findings.push(Finding::fatal("lint", codes(&findings)));
    } else if !findings.is_empty() {
        out.findings
            .push(Finding::note("lint", findings.len(), codes(&findings)));
    }
    // Strict, hermetic evaluation: schema repairs are not silently
    // downgraded. Warnings fail only under --strict and are notes
    // otherwise.
    match engine.evaluate(&module) {
        Ok((value, diags)) => {
            let fatal = diags.iter().any(|d| {
                d.severity == Severity::Error || (strict && d.severity == Severity::Warning)
            });
            if fatal {
                out.findings.push(Finding::fatal("schema", codes(&diags)));
            } else if !diags.is_empty() {
                out.findings
                    .push(Finding::note("schema", diags.len(), codes(&diags)));
            }
            out.evaluated = Some((value, diags));
        }
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => {
            out.findings
                .push(Finding::fatal("schema", codes(&diagnostics)));
            out.eval_failure = Some((String::new(), diagnostics));
        }
        Err(error) => {
            out.findings
                .push(Finding::detail("schema-fatal", error.code(), true));
            out.eval_failure = Some((error.code().to_string(), Vec::new()));
        }
    }
    out.module = Some(module);
    out
}

/// Render one finding exactly as `active test` has always printed it.
pub(crate) fn render(path: &Path, finding: &Finding) -> String {
    let file = path.display();
    match finding.check {
        "read" => format!(
            "read {file}: {}",
            finding.detail.as_deref().unwrap_or_default()
        ),
        "size" => format!(
            "parse {file}: {}",
            finding.detail.as_deref().unwrap_or_default()
        ),
        "parse" => format!("parse {file}: [{}]", finding.codes.join(",")),
        "repair" => format!(
            "repair {file}: needs repair [{}] (run `active repair`)",
            finding.codes.join(",")
        ),
        "fmt" => format!("fmt {file}: not canonical (run `active fmt --write`)"),
        "fmt-refused" => format!(
            "fmt {file}: cannot be formatted ({})",
            finding.detail.as_deref().unwrap_or_default()
        ),
        "lint-config" => format!(
            "lint {file}: {}",
            finding.detail.as_deref().unwrap_or_default()
        ),
        "lint" if finding.fatal => format!("lint {file}: [{}]", finding.codes.join(",")),
        "lint" => format!(
            "lint {file}: {} finding(s) [{}]",
            finding.count,
            finding.codes.join(",")
        ),
        "schema" if finding.fatal => format!("schema {file}: [{}]", finding.codes.join(",")),
        "schema" => format!(
            "schema {file}: {} warning(s) [{}]",
            finding.count,
            finding.codes.join(",")
        ),
        _ => format!(
            "schema {file}: {}",
            finding.detail.as_deref().unwrap_or_default()
        ),
    }
}

fn check_file(path: &Path, strict: bool, env: &HashMap<String, String>, report: &mut Report) {
    let mut engine = Engine::strict().with_env(env.clone());
    engine.capabilities = held_authority();
    let verdict = verdict(path, strict, &engine, None, false, None);
    for finding in verdict.findings {
        let text = render(path, &finding);
        if finding.fatal {
            report.failures.push(text);
        } else {
            report.notes.push(text);
        }
    }
}

fn run_conformance(manifest: &Path, report: &mut Report) {
    let mut cases = 0usize;
    let mut failed = 0usize;
    let text = match std::fs::read_to_string(manifest) {
        Ok(text) => text,
        Err(e) => {
            report.failures.push(format!(
                "conformance {}: cannot read: {e}",
                manifest.display()
            ));
            return;
        }
    };
    let doc: toml::Value = match toml::from_str(&text) {
        Ok(doc) => doc,
        Err(e) => {
            report
                .failures
                .push(format!("conformance {}: {e}", manifest.display()));
            return;
        }
    };
    if let Some(version) = doc.get("version").and_then(|v| v.as_integer()) {
        if version != 1 {
            report.failures.push(format!(
                "conformance {}: unsupported manifest version {version}",
                manifest.display()
            ));
            return;
        }
    }
    let root = manifest.parent().unwrap_or_else(|| Path::new("."));
    let Some(list) = doc.get("cases").and_then(|v| v.as_array()) else {
        report
            .failures
            .push(format!("conformance {}: no [[cases]]", manifest.display()));
        return;
    };
    if list.is_empty() {
        report.failures.push(format!(
            "conformance {}: manifest has no cases",
            manifest.display()
        ));
        return;
    }
    for case in list {
        cases += 1;
        let (Some(id), Some(file), Some(kind)) = (
            case.get("id").and_then(|v| v.as_str()),
            case.get("file").and_then(|v| v.as_str()),
            case.get("kind").and_then(|v| v.as_str()),
        ) else {
            failed += 1;
            report
                .failures
                .push("conformance: case without id/file/kind".to_string());
            continue;
        };
        let path = root.join(file);
        let want_codes = if kind == "clean" {
            Vec::new()
        } else {
            match read_codes(root, id) {
                Ok(codes) => codes,
                Err(e) => {
                    failed += 1;
                    report.failures.push(format!("conformance {id}: {e}"));
                    continue;
                }
            }
        };
        if let Err(e) = check_case(&path, kind, case, &want_codes, root, id) {
            failed += 1;
            report.failures.push(format!("conformance {id}: {e}"));
        }
    }
    report.cases += cases;
    report.case_failures += failed;
    report.notes.push(format!(
        "conformance {}: {} case(s), {} failure(s)",
        manifest.display(),
        cases,
        failed
    ));
}

fn read_codes(root: &Path, id: &str) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(root.join("errors").join(format!("{id}.json")))
        .map_err(|e| format!("cannot read expected codes: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("bad expected codes: {e}"))?;
    let list = value
        .get("codes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "expected codes file has no `codes` array".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        out.push(
            item.as_str()
                .ok_or_else(|| "expected codes contain a non-string entry".to_string())?
                .to_string(),
        );
    }
    Ok(out)
}

fn check_case(
    path: &Path,
    kind: &str,
    case: &toml::Value,
    want_codes: &[String],
    root: &Path,
    id: &str,
) -> Result<(), String> {
    let mut options = crate::with_parse_limits(ParseOptions::default());
    options.enable_includes = true;
    let module = match active_core::parse_file(path, options) {
        Ok(module) => module,
        Err(diags) => {
            let got = norm(codes(&diags));
            return if got == norm(want_codes.to_vec()) && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "parse refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    norm(want_codes.to_vec()).join(",")
                ))
            };
        }
    };
    let mut engine = Engine::new().with_env(HashMap::new());
    engine.capabilities = held_authority();
    engine.profile_override = case
        .get("profile")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    match engine.evaluate(&module) {
        Ok((value, diags)) => {
            if kind == "refused" {
                // The reference suite compares refused codes exactly and
                // requires a non-empty set (NORMALIZATION.md).
                let got = codes(&diags);
                if got.is_empty() || got != want_codes {
                    return Err(format!(
                        "refused codes [{}]; wanted [{}]",
                        got.join(","),
                        want_codes.join(",")
                    ));
                }
                return Ok(());
            }
            let got = norm(codes(&diags));
            let want = if kind == "clean" {
                Vec::new()
            } else {
                norm(want_codes.to_vec())
            };
            if got != want {
                return Err(format!(
                    "codes [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    want.join(",")
                ));
            }
            let json = active_core::convert::value_to_pretty_json(&value);
            let got_value: serde_json::Value =
                serde_json::from_str(&json).map_err(|e| format!("bad value JSON: {e}"))?;
            let text = std::fs::read_to_string(root.join("expected").join(format!("{id}.json")))
                .map_err(|e| format!("cannot read expected value: {e}"))?;
            let want_value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("bad expected value: {e}"))?;
            if got_value != want_value {
                return Err("value diverged from expected/<id>.json".to_string());
            }
            Ok(())
        }
        Err(active_core::EvalError::ValidationFailed { diagnostics }) => {
            let got = norm(codes(&diagnostics));
            if !got.is_empty() && got == norm(want_codes.to_vec()) && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "validation refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    norm(want_codes.to_vec()).join(",")
                ))
            }
        }
        Err(error) => {
            let got = vec![error.code().to_string()];
            if got == want_codes && kind == "refused" {
                Ok(())
            } else {
                Err(format!(
                    "refused [{}]; wanted [{}] ({kind})",
                    got.join(","),
                    want_codes.join(",")
                ))
            }
        }
    }
}

fn codes(diags: &[active_core::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| d.code.clone()).collect()
}

fn norm(mut codes: Vec<String>) -> Vec<String> {
    codes.sort();
    codes.dedup();
    codes
}
