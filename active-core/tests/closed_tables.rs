//! L-P11: the closed-table audit. `envelope::closed_tables` records, for
//! every closed schema table instance, the key names present at it - the
//! receipt's "these keys were present when I closed it". Values never
//! appear; the walk is over the redacted document in the real paths.

use active_core::envelope::{closed_tables, ClosedTable};
use active_core::{parse, Engine};

fn facts(src: &str) -> Vec<ClosedTable> {
    let module = parse(src, "closed-tables").unwrap_or_else(|diags| panic!("parse: {diags:?}"));
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluation");
    assert!(diags.is_empty(), "{diags:?}");
    closed_tables(&module, &value)
}

fn fact(path: &str, keys: &[&str]) -> ClosedTable {
    ClosedTable {
        path: path.to_string(),
        keys: keys.iter().map(|key| key.to_string()).collect(),
    }
}

#[test]
fn root_and_nested_closed_tables_are_audited() {
    let found = facts(
        "schema = { closed = true, server = Table { closed = true, host = String, port = Int } }\n\
         server = { host = \"x\", port = 8080 }\n",
    );
    assert_eq!(
        found,
        vec![fact(".", &["server"]), fact("server", &["host", "port"])],
        "{found:?}"
    );
}

#[test]
fn open_tables_are_not_audited() {
    // Open tables keep ignoring extra keys and make no promise, so there
    // is nothing to record.
    let found = facts(
        "schema = { server = Table { host = String } }\n\
         server = { host = \"x\", extra = 1 }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn array_elements_are_audited_by_index() {
    let found = facts(
        "schema = { servers = Array<Table { closed = true, host = String }> }\n\
         servers = [{ host = \"a\" }, { host = \"b\" }]\n",
    );
    assert_eq!(
        found,
        vec![fact("servers[0]", &["host"]), fact("servers[1]", &["host"])],
        "{found:?}"
    );
}

#[test]
fn a_disjunction_records_the_closed_shape_the_value_satisfies() {
    // Only the branch whose shape accepts the value contributes. The
    // other closed branch declares `a`; recording it would claim a
    // promise the value does not satisfy.
    let found = facts(
        "schema = { v = OneOf(Table { closed = true, a = Int }, \
         Table { closed = true, b = String }) }\n\
         v = { b = \"x\" }\n",
    );
    assert_eq!(found, vec![fact("v", &["b"])], "{found:?}");
}

#[test]
fn a_disjunction_that_matches_more_than_one_branch_is_recorded_once() {
    let found = facts(
        "schema = { v = OneOf(Table { closed = true, a = Int }, \
         Table { closed = true, a = Int }) }\n\
         v = { a = 1 }\n",
    );
    assert_eq!(found, vec![fact("v", &["a"])], "{found:?}");
}

#[test]
fn entries_carry_keys_and_never_values() {
    let source = "schema = { closed = true, token = String, port = Int }\n\
                  token = \"hunter2\"\n\
                  port = 8080\n";
    let module = parse(source, "closed-values").expect("parse");
    let (value, _) = Engine::new().evaluate(&module).expect("evaluation");
    let found = closed_tables(&module, &value);
    assert_eq!(found, vec![fact(".", &["port", "token"])], "{found:?}");
    let rendered = serde_json::to_string(&found).expect("serialize");
    assert!(
        !rendered.contains("hunter2") && !rendered.contains("8080"),
        "the audit must never echo a value: {rendered}"
    );
}

#[test]
fn the_audit_is_deterministic_and_sorted() {
    let source = "schema = { z = Table { closed = true, b = Int, a = Int }, \
                  y = Table { closed = true, c = Int } }\n\
                  z = { b = 2, a = 1 }\n\
                  y = { c = 3 }\n";
    let first = facts(source);
    let second = facts(source);
    assert_eq!(first, second, "same input, same audit");
    assert_eq!(
        first,
        vec![fact("y", &["c"]), fact("z", &["a", "b"])],
        "entries sort by path and keys sort inside each entry: {first:?}"
    );
}

#[test]
fn a_closed_root_exempts_active_profile() {
    // G23 (Bud's adjudication): the root metadata exemption is three keys,
    // including `active_profile`. Before the fix, a closed-root document
    // that selected a profile failed T005 on its own metadata key.
    let source = "schema = { closed = true, x = Int }\n\
                  active_profile = \"prod\"\n\
                  x = 1\n\
                  profile \"prod\" { x = 2 }\n";
    let module = parse(source, "closed-profile").expect("parse");
    let (value, diags) = Engine::new().evaluate(&module).expect("evaluate");
    assert!(
        diags.iter().all(|d| d.code != "T005"),
        "active_profile must not be an unexpected key: {diags:?}"
    );
    let json = active_core::convert::value_to_json(&value);
    assert_eq!(json["x"], 2, "the selected profile must merge: {json}");
}
