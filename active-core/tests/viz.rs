//! Visualizer data tests (TOOLING VIZ.008): stable JSON shapes and
//! redaction on every emitter.

use active_core::viz;
use active_core::{parse, Budget, Engine};

fn fixture() -> (
    active_core::Module,
    active_core::Value,
    Vec<active_core::Diagnostic>,
) {
    let src = "schema = { port = Int(min = 1), token = String }\nport = 8080\ntoken = secret(\"hunter2-viz\")\n";
    let module = parse(src, "viz").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    (module, value, diags)
}

#[test]
fn taint_data_lists_sources_without_values() {
    let (module, value, _) = fixture();
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"token"), "{sources:?}");
    let rendered = serde_json::to_string(&data).expect("json");
    assert!(!rendered.contains("hunter2"), "raw secret in taint data");
    // A flat document has no containment edges, and the root (no path)
    // is never spelled as an empty-string parent.
    assert!(
        data["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .all(|edge| edge["from"].as_str() != Some("")),
        "anonymous parent edge: {data}"
    );
}

#[test]
fn taint_edges_use_containment_for_array_elements() {
    let src = "a = [secret(\"x\"), { b = secret(\"y\") }]\n";
    let module = parse(src, "viz-edges").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let edges: Vec<(String, String)> = data["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["from"].as_str().unwrap_or("").to_string(),
                edge["to"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    assert!(
        edges.contains(&("a".to_string(), "a[0]".to_string())),
        "array element edge: {edges:?}"
    );
    assert!(
        edges.contains(&("a".to_string(), "a[1]".to_string())),
        "nested table edge: {edges:?}"
    );
    assert!(
        edges.iter().all(|(from, _)| !from.is_empty()),
        "no anonymous parent edges: {edges:?}"
    );
}

#[test]
fn taint_data_includes_plain_locator_sources() {
    let src = "x = \"hunter2-loc\"\nmsg = \"prefix ${x}\"\nk = secret(x)\n";
    let module = parse(src, "viz-source").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"x"), "{sources:?}");
    assert!(sources.contains(&"msg"), "{sources:?}");
    assert!(sources.contains(&"k"), "{sources:?}");
    assert!(!data.to_string().contains("hunter2"), "{data}");
}

#[test]
fn taint_edges_never_dangle_to_unemitted_parents() {
    use std::collections::HashSet;
    let src = "let p = env.R13V\nk = secret(p)\n@patch(\"out.a\", p)\nout = { a = \"orig\" }\n";
    let module = parse(src, "viz-dangling").expect("parse");
    let mut env = std::collections::HashMap::new();
    env.insert("R13V".to_string(), "ENVSECRET-viz".to_string());
    let (value, _) = Engine::hermetic()
        .with_env(env)
        .evaluate(&module)
        .expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let ids: HashSet<String> = data["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|node| node["id"].as_str().map(str::to_string))
        .collect();
    for edge in data["edges"].as_array().expect("edges") {
        let from = edge["from"].as_str().unwrap_or("");
        let to = edge["to"].as_str().unwrap_or("");
        assert!(ids.contains(from), "dangling from '{from}': {data}");
        assert!(ids.contains(to), "dangling to '{to}': {data}");
    }
}

#[test]
fn budget_size_counts_redacted_strings() {
    // The byte estimate describes the exported (redacted) document: a
    // material string reports the marker length, not its own length.
    let src = "let t = env.R20TOK\nk = secret(t)\nleak = t\n";
    let module = parse(src, "viz-size").expect("parse");
    let measure = |value: &str| {
        let mut env = std::collections::HashMap::new();
        env.insert("R20TOK".to_string(), value.to_string());
        let (evaluated, diags) = Engine::hermetic()
            .with_env(env)
            .evaluate(&module)
            .expect("evaluate");
        let data = viz::budget_data(&module, &evaluated, &Budget::default(), &diags);
        data["usage"]["value_size"].as_u64().expect("value_size")
    };
    assert_eq!(
        measure("short"),
        measure("a-much-longer-secret-value"),
        "the size estimate must not reveal material length"
    );
}

#[test]
fn taint_data_with_profile_honors_an_override() {
    let src = "n = 4242\nk = secret(n)\nactive_profile = \"a\"\nprofile \"a\" { x = 1 }\nprofile \"b\" { x = n }\nx = 0\n";
    let module = parse(src, "viz-override").expect("parse");
    let mut engine = Engine::hermetic();
    engine.profile_override = Some("b".to_string());
    let (value, _) = engine.evaluate(&module).expect("evaluate");
    let sources = viz::taint_data_with_profile(&module, &value, Some("b"))["sources"]
        .as_array()
        .expect("sources")
        .clone();
    assert!(
        sources.iter().any(|s| s == "x"),
        "override material missing: {sources:?}"
    );
}

#[test]
fn budget_data_reports_limits_usage_and_headroom() {
    let (module, value, diags) = fixture();
    let budget = Budget::strict();
    let data = viz::budget_data(&module, &value, &budget, &diags);
    assert_eq!(data["limits"]["nodes"], budget.max_nodes);
    assert_eq!(data["limits"]["depth"], budget.max_depth);
    assert_eq!(data["usage"]["nodes"], module.arena.len());
    let used = data["usage"]["nodes"].as_u64().expect("nodes");
    let headroom = data["headroom"]["nodes"].as_u64().expect("headroom");
    assert_eq!(headroom, budget.max_nodes - used);
    assert!(data["usage"]["depth"].as_u64().unwrap() >= 2);
    assert!(data["warnings"].as_array().expect("warnings").is_empty());
}

#[test]
fn budget_data_disabled_limit_reports_null_headroom_and_sentinel_charge() {
    // A disabled limit is the `UNLIMITED` sentinel, not a zero field, so
    // `headroom` reads null and `charge.limit` carries the sentinel — a
    // consumer can tell "disabled" from a finite ceiling. Negative control:
    // testing `limit == 0` reported a huge numeric headroom (and a zero
    // field is "shipped default", never disabled).
    let (module, value, diags) = fixture();
    let data = viz::budget_data(&module, &value, &Budget::unlimited(), &diags);
    assert_eq!(data["limits"]["nodes"], active_core::eval::UNLIMITED);
    assert!(
        data["headroom"]["nodes"].is_null(),
        "a disabled limit must report null headroom: {data}"
    );

    let charge = active_core::eval::ChargeReport {
        total: 7,
        limit: active_core::eval::UNLIMITED,
    };
    let with_charge = viz::budget_data_with_seeds_demand(
        &module,
        &value,
        &Budget::unlimited(),
        &diags,
        viz::SeedContext {
            profile_override: None,
            env: None,
            seeds: None,
        },
        &active_core::Demand::All,
        Some(charge),
    );
    assert_eq!(
        with_charge["charge"]["limit"],
        active_core::eval::UNLIMITED,
        "disabled total-value ceiling must surface as the sentinel: {with_charge}"
    );

    // A finite ceiling still reports a numeric headroom.
    let finite = viz::budget_data(&module, &value, &Budget::strict(), &diags);
    assert!(finite["headroom"]["nodes"].is_u64(), "{finite}");
}

#[test]
fn budget_data_resolves_a_zeroed_budget_like_every_other_surface() {
    // LANG-C-015: a zero field means "shipped default" at every consumer,
    // so a library embedder handing `viz` a zeroed `Budget` reads the
    // shipped limits and a shipped headroom, never `limits.nodes = 0`.
    // Negative control: without the `resolve` inside
    // `budget_data_with_redactor`, every assertion below fails (limits and
    // headroom read 0).
    let (module, value, diags) = fixture();
    let shipped = Budget::default();
    let zeroed = Budget {
        max_nodes: 0,
        max_depth: 0,
        max_array_len: 0,
        max_keys: 0,
        max_wall_ms: 0,
        max_value_size: 0,
        max_total_value_size: 0,
    };
    let data = viz::budget_data(&module, &value, &zeroed, &diags);
    assert_eq!(data["limits"]["nodes"], shipped.max_nodes, "{data}");
    assert_eq!(data["limits"]["depth"], shipped.max_depth, "{data}");
    assert_eq!(
        data["limits"]["total_size"], shipped.max_total_value_size,
        "{data}"
    );
    let used = data["usage"]["nodes"].as_u64().expect("usage.nodes");
    assert_eq!(
        data["headroom"]["nodes"],
        shipped.max_nodes - used,
        "{data}"
    );
    assert!(
        data["headroom"]["nodes"].as_u64().expect("headroom") > 0,
        "a resolved ceiling must leave headroom: {data}"
    );
}

#[test]
fn schema_data_matches_introspection() {
    let (module, _, _) = fixture();
    let data = viz::schema_data(&module).expect("schema");
    assert_eq!(data["version"], 2);
    let fields = data["fields"].as_array().expect("fields");
    assert!(fields.iter().any(|f| f["path"] == "port"));
    assert_eq!(data["mode"], "config");
}

#[test]
fn schema_data_carries_compatibility_and_migration_hints() {
    let source = "schema = {\n  version = \"1.2.0\",\n  min_reader = \"1.0.0\",\n  min_writer = \"1.1.0\",\n  old = String(deprecated = \"use new instead\"),\n  legacy = Int(removed_in = \"2.0.0\"),\n  current = Bool,\n}\n";
    let module = active_core::parse(source, "viz").expect("parses");
    let data = viz::schema_data(&module).expect("schema");
    let compatibility = &data["compatibility"];
    assert_eq!(compatibility["schema_version"], "1.2.0", "{data}");
    assert_eq!(compatibility["min_reader"], "1.0.0", "{data}");
    assert_eq!(compatibility["min_writer"], "1.1.0", "{data}");
    let deprecated = compatibility["deprecated"]
        .as_array()
        .expect("deprecated list");
    assert_eq!(deprecated.len(), 1, "{data}");
    assert_eq!(deprecated[0]["path"], "old");
    assert_eq!(deprecated[0]["hint"], "use new instead");
    let removals = compatibility["removals"].as_array().expect("removals");
    assert_eq!(removals.len(), 1, "{data}");
    assert_eq!(removals[0]["path"], "legacy");
    assert_eq!(removals[0]["removed_in"], "2.0.0");
    assert!(removals[0]["hint"]
        .as_str()
        .unwrap_or_default()
        .contains("2.0.0"));
}

#[test]
fn repair_data_exposes_the_plan() {
    let source = "schema = { port = Int(default = 8080) }\n";
    let data = viz::repair_data(source, active_core::diag::RepairGoal::Validate, &[], 8);
    let steps = data["steps"].as_array().expect("steps");
    assert!(!steps.is_empty(), "{data}");
    assert_eq!(steps[0]["kind"], "fill");
    assert!(
        data.get("repaired_source").is_none(),
        "display plan must not carry source: {data}"
    );
    let secret_source = "schema = { token = Int }\ntoken = secret(\"hunter2-viz-repair\")\n";
    let secret_data = viz::repair_data(
        secret_source,
        active_core::diag::RepairGoal::Validate,
        &[],
        8,
    );
    assert!(!secret_data.to_string().contains("hunter2"));
}

#[test]
fn taint_data_surfaces_secret_bearing_blobs() {
    let src = "f = blob(\"text/plain\", secret(\"hunter2-taint\"))\n";
    let module = parse(src, "viz-blob").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluate");
    let data = viz::taint_data(&module, &value);
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(sources.contains(&"f"), "{sources:?}");
    assert!(!data.to_string().contains("hunter2"));
}

#[test]
fn budget_data_with_profile_hides_selected_material_length() {
    // F13: the dashboard reports redacted sizes. An embedder that
    // evaluated under a profile override must pass the same selector,
    // or a long material literal leaks through `usage.value_size`.
    let src = "active_profile = \"a\"\nprofile \"a\" { n = \"benign\" }\nprofile \"b\" { n = env.ACTIVE_VIZ_BUDGET_MATERIAL }\nn = \"default\"\nk = secret(n)\n";
    let module = parse(src, "viz-budget-profile").expect("parse");
    let size_of = |material: &str| {
        std::env::set_var("ACTIVE_VIZ_BUDGET_MATERIAL", material);
        let mut engine = Engine::new();
        engine.profile_override = Some("b".to_string());
        let (value, diags) = engine.evaluate(&module).expect("evaluate");
        let data =
            viz::budget_data_with_profile(&module, &value, &engine.budget, &diags, Some("b"));
        data["usage"]["value_size"].as_u64().expect("value_size")
    };
    assert_eq!(
        size_of("abcde"),
        size_of("0123456789012345678901234567890123456789"),
        "material length must not change the reported size"
    );
}

#[test]
fn taint_data_with_seeds_reports_a_runtime_copy_and_payload_secret() {
    // Review 49: value-dependent taint needs the evaluator seeds and a
    // materialised payload to see a copy or payload-resident secret.
    let module = parse(
        "let v = \"PIN-${1234}\"\nlet k = secret(v)\ncopy = \"PIN-1234\"\nf = blob(\"text/plain\", \"PIN-1234\")\n",
        "viz-seeds",
    )
    .expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (value, _, seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let data = viz::taint_data_with_seeds(&module, &value, None, None, Some(&seeds));
    let sources: Vec<&str> = data["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .filter_map(|item| item.as_str())
        .collect();
    assert!(sources.contains(&"copy"), "{sources:?}");
    assert!(sources.contains(&"f"), "{sources:?}");
}

#[test]
fn budget_data_with_seeds_ignores_a_runtime_copy_length() {
    let module = parse(
        "let a = \"PIN-${12}\"\nlet k = secret(a)\ncopy = \"PIN-12\"\n",
        "viz-budget-short",
    )
    .expect("parse");
    let module_long = parse(
        "let a = \"PIN-${123456789012}\"\nlet k = secret(a)\ncopy = \"PIN-123456789012\"\n",
        "viz-budget-long",
    )
    .expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let (short, _, short_seeds) = engine.evaluate_with_seeds(&module).expect("evaluate");
    let (long, _, long_seeds) = engine.evaluate_with_seeds(&module_long).expect("evaluate");
    let size = |module: &active_core::Module,
                value: &active_core::Value,
                seeds: &active_core::secret::SecretRedactor| {
        viz::budget_data_with_seeds(
            module,
            value,
            &Budget::trusted(),
            &[],
            None,
            None,
            Some(seeds),
        )["usage"]["value_size"]
            .as_u64()
            .expect("value_size")
    };
    assert_eq!(
        size(&module, &short, &short_seeds),
        size(&module_long, &long, &long_seeds),
        "secret length must not change the reported size"
    );
}

#[test]
fn budget_data_redacts_the_cost_report_and_ignores_a_secret_length() {
    // L-P40 §3.2 / LANG-C-010: the dashboard's `cost_model` sizes a
    // material node at the redacted marker and redacts a path segment
    // whose text is the secret, so neither the secret's length nor its
    // cleartext reaches the report.
    let module = parse(
        "let tok = secret(\"secretshort1\")\ncopy = \"secretshort1\"\ntbl = { secretshort1 = 1 }\n",
        "viz-cost-short",
    )
    .expect("parse");
    let module_long = parse(
        "let tok = secret(\"secretmuchlongervalue1\")\ncopy = \"secretmuchlongervalue1\"\ntbl = { secretmuchlongervalue1 = 1 }\n",
        "viz-cost-long",
    )
    .expect("parse");
    let mut engine = Engine::hermetic();
    engine.materialise_blobs = true;
    let cost = |module: &active_core::Module| {
        let (value, _, seeds) = engine.evaluate_with_seeds(module).expect("evaluate");
        viz::budget_data_with_seeds(
            module,
            &value,
            &Budget::default(),
            &[],
            None,
            None,
            Some(&seeds),
        )["cost_model"]
            .clone()
    };
    let a = cost(&module);
    let b = cost(&module_long);
    assert_eq!(
        a["classes"], b["classes"],
        "the secret's length must not change the serialized class totals"
    );
    let paths: Vec<String> = a["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .filter_map(|n| n["path"].as_str().map(str::to_string))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.contains(active_core::secret::MARKER)),
        "the secret key path must render redacted: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.contains("secretshort1")),
        "no cleartext secret may appear in a path: {paths:?}"
    );
}

#[test]
fn budget_data_reports_a_bounded_deterministic_cost_model() {
    // L-P40 §3.2: the dashboard carries the whole-document cost report.
    let module = parse(
        "let base = 5\nname: \"cfg\"\nkeep:\n  n: base\n  s: \"${name}-x\"\n",
        "viz-cost",
    )
    .expect("parse");
    let (value, diags) = Engine::hermetic().evaluate(&module).expect("evaluate");
    let data = viz::budget_data_with_seeds(
        &module,
        &value,
        &Budget::default(),
        &diags,
        None,
        None,
        None,
    );
    let cost = &data["cost_model"];
    assert_eq!(cost["demand"], "all", "{cost}");
    assert_eq!(cost["truncated"], false, "{cost}");
    let classes = cost["classes"].as_array().expect("classes");
    assert_eq!(classes.len(), 3, "one entry per class: {cost}");
    assert_eq!(classes[0]["class"], "literal");
    assert_eq!(classes[1]["class"], "alias");
    assert_eq!(classes[2]["class"], "computed");
    assert!(classes[0]["nodes"].as_u64().unwrap() >= 1, "{cost}");
    assert!(classes[2]["nodes"].as_u64().unwrap() >= 1, "{cost}");
    let nodes = cost["nodes"].as_array().expect("nodes");
    assert!(!nodes.is_empty(), "{cost}");
    for node in nodes {
        for key in ["path", "node", "class", "depth", "charged_units", "reason"] {
            assert!(node.get(key).is_some(), "missing {key} in {node}");
        }
    }
    // The interpolation is the most charged node, and the ranking is
    // stable across identical runs.
    let again = viz::budget_data_with_seeds(
        &module,
        &value,
        &Budget::default(),
        &diags,
        None,
        None,
        None,
    );
    assert_eq!(data["cost_model"], again["cost_model"], "deterministic");
}

#[test]
fn budget_data_cost_model_names_a_cone_demand() {
    // L-P40 §3.2: a cone-scoped dashboard describes its cone and ranks
    // only closure nodes. The value and diagnostics are the cone's own,
    // so `usage` (cone-shaped) and `cost_model` (closure-scoped) are
    // exercised together, not a whole-document value with a cone label.
    let module = parse("keep:\n  n: 1\nother:\n  m: 2\n", "viz-cost-cone").expect("parse");
    let engine = Engine::hermetic();
    let cone = active_core::Demand::Cone("keep".to_string());
    let outcome = engine
        .evaluate_with_seeds_demanded(&module, &cone)
        .expect("evaluate");
    let data = viz::budget_data_with_seeds_demand(
        &module,
        &outcome.value,
        &engine.budget,
        &outcome.diagnostics,
        viz::SeedContext::default(),
        &cone,
        Some(outcome.charge),
    );
    let cost = &data["cost_model"];
    assert_eq!(cost["demand"], "keep", "{cost}");
    let nodes = cost["nodes"].as_array().expect("nodes");
    assert!(
        nodes.iter().all(|n| {
            n["path"]
                .as_str()
                .is_none_or(|p| p == "keep" || p.starts_with("keep."))
        }),
        "the cone must not rank an undemanded sibling: {nodes:?}"
    );
}

#[test]
fn budget_data_reports_the_run_charge_and_scope() {
    // L-P40 §3.4: the dashboard reports what the run spent, and
    // `cost_model.scope_nodes` names the demanded node count.
    let module = parse("keep:\n  n: 1\nother:\n  m: 2\n", "viz-charge").expect("parse");
    let engine = Engine::hermetic();
    let all = active_core::Demand::All;
    let outcome = engine
        .evaluate_with_seeds_demanded(&module, &all)
        .expect("evaluate");
    let seeds = viz::SeedContext {
        seeds: Some(&outcome.seeds),
        ..viz::SeedContext::default()
    };
    let data = viz::budget_data_with_seeds_demand(
        &module,
        &outcome.value,
        &engine.budget,
        &outcome.diagnostics,
        seeds,
        &all,
        Some(outcome.charge),
    );
    assert_eq!(data["charge"]["total"], outcome.charge.total, "{data}");
    assert_eq!(data["charge"]["limit"], outcome.charge.limit, "{data}");
    // The whole-document demand materialises the whole arena.
    let arena = module.arena.len() as u64;
    assert_eq!(data["cost_model"]["scope_nodes"], arena, "{data}");

    // A cone scopes the dashboard to its closure, a strict subset.
    let cone = active_core::Demand::Cone("keep".to_string());
    let cone_outcome = engine
        .evaluate_with_seeds_demanded(&module, &cone)
        .expect("evaluate");
    let cone_seeds = viz::SeedContext {
        seeds: Some(&cone_outcome.seeds),
        ..viz::SeedContext::default()
    };
    let cone_data = viz::budget_data_with_seeds_demand(
        &module,
        &cone_outcome.value,
        &engine.budget,
        &cone_outcome.diagnostics,
        cone_seeds,
        &cone,
        Some(cone_outcome.charge),
    );
    let scope = cone_data["cost_model"]["scope_nodes"]
        .as_u64()
        .expect("scope_nodes");
    assert!(
        scope < arena,
        "cone scope {scope} must be smaller than arena {arena}"
    );

    // A structural caller that ran no accountant reports a `null` charge.
    let structural = viz::budget_data_with_seeds_demand(
        &module,
        &outcome.value,
        &engine.budget,
        &outcome.diagnostics,
        viz::SeedContext::default(),
        &all,
        None,
    );
    assert!(structural["charge"].is_null(), "{structural}");
}
