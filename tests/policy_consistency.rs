//! Cross-entry-point policy and schema consistency checks.
//!
//! The WASM `compile_to_*` bindings and the legacy wrappers share the same
//! `compile_compat` helper as the native calls below, so exercising the native
//! wrappers exercises the exact path WASM uses. RPC is driven through the real
//! stdio protocol handler.

use lql::protocol::serve_stdio;
use lql::{
    compile_to_clickhouse, compile_to_duckdb, compile_to_postgres, render_query_with_options,
    validate_query_with_options, QueryPolicy, Schema, Target,
};
use serde_json::{json, Value};
use std::io::Cursor;

fn rpc_compile(query: &str, target: &str, policy: Option<Value>) -> Value {
    let mut params = json!({"query": query, "target": target});
    if let Some(policy) = policy {
        params["policy"] = policy;
    }
    let input = format!(
        "{}\n",
        json!({
            "id": 1,
            "method": "compile",
            "protocol_version": 1,
            "language_version": "0.1",
            "params": params,
        })
    );
    let mut output = Vec::new();
    serve_stdio(Cursor::new(input), &mut output).unwrap();
    serde_json::from_slice(&output).unwrap()
}

fn rpc_accepted(response: &Value) -> bool {
    response
        .get("result")
        .map(|r| r.is_object())
        .unwrap_or(false)
}

fn compat_accepted(result: Result<String, lql::LqlError>) -> bool {
    result.is_ok()
}

#[test]
fn schema_defaults_are_identical_across_targets() {
    let duckdb = Schema::default_for(Target::DuckDB);
    let clickhouse = Schema::default_for(Target::ClickHouse);
    let postgres = Schema::default_for(Target::PostgreSQL);

    let mut duckdb_fields = duckdb.field_names();
    duckdb_fields.sort_unstable();
    let mut clickhouse_fields = clickhouse.field_names();
    clickhouse_fields.sort_unstable();
    let mut postgres_fields = postgres.field_names();
    postgres_fields.sort_unstable();

    assert_eq!(duckdb_fields, clickhouse_fields);
    assert_eq!(duckdb_fields, postgres_fields);
    // The canonical default is the revisioned LOZA schema, not the legacy
    // hand-written `duckdb_default` field set.
    let loza = Schema::loza_v1(Target::DuckDB);
    let mut loza_fields = loza.field_names();
    loza_fields.sort_unstable();
    assert_eq!(duckdb_fields, loza_fields);
}

#[test]
fn wasm_and_rpc_reject_legacy_only_field() {
    // `host` exists only in the legacy `duckdb_default()` schema. Both the
    // WASM/legacy wrapper and RPC must now use the canonical schema and reject.
    let query = r#"from events | where host = "prod""#;

    assert!(
        !compat_accepted(compile_to_duckdb(query)),
        "WASM/legacy DuckDB"
    );
    assert!(
        !compat_accepted(compile_to_clickhouse(query)),
        "WASM/legacy ClickHouse"
    );
    assert!(
        !compat_accepted(compile_to_postgres(query)),
        "WASM/legacy PostgreSQL"
    );

    // `render_query` is the exact function the CLI `compile` command calls.
    assert!(
        lql::render_query(query, Target::DuckDB).is_err(),
        "CLI DuckDB"
    );
    assert!(
        lql::render_query(query, Target::ClickHouse).is_err(),
        "CLI ClickHouse"
    );
    assert!(
        lql::render_query(query, Target::PostgreSQL).is_err(),
        "CLI PostgreSQL"
    );

    for target in ["duckdb", "clickhouse", "postgres"] {
        let response = rpc_compile(query, target, None);
        assert!(!rpc_accepted(&response), "RPC target {target} must reject");
        assert_eq!(response["error"]["code"], "compile_failed");
    }
}

#[test]
fn wasm_and_rpc_accept_canonical_only_field() {
    // `delivery_attempts` exists only in the revisioned LOZA schema. Before the
    // fix the WASM/legacy wrapper rejected it while RPC accepted it.
    let query = "from events | project delivery_attempts";

    assert!(
        compat_accepted(compile_to_duckdb(query)),
        "WASM/legacy DuckDB"
    );
    assert!(
        compat_accepted(compile_to_clickhouse(query)),
        "WASM/legacy ClickHouse"
    );
    assert!(
        compat_accepted(compile_to_postgres(query)),
        "WASM/legacy PostgreSQL"
    );

    // `render_query` is the exact function the CLI `compile` command calls.
    assert!(
        lql::render_query(query, Target::DuckDB).is_ok(),
        "CLI DuckDB"
    );
    assert!(
        lql::render_query(query, Target::ClickHouse).is_ok(),
        "CLI ClickHouse"
    );
    assert!(
        lql::render_query(query, Target::PostgreSQL).is_ok(),
        "CLI PostgreSQL"
    );

    for target in ["duckdb", "clickhouse", "postgres"] {
        let response = rpc_compile(query, target, None);
        assert!(rpc_accepted(&response), "RPC target {target} must accept");
    }
}

#[test]
fn policy_violation_is_rejected_by_every_analysis_entry_point() {
    let query = "from events | project message";
    let policy = QueryPolicy::from_value(&json!({
        "allowed_sources": ["events"],
        "sensitive_fields": ["message"],
    }))
    .expect("valid policy");

    // Direct typed path used by the compatibility/WASM wrapper.
    let direct = render_query_with_options(
        query,
        Target::DuckDB,
        &Value::Null,
        &Value::Null,
        Schema::default_for(Target::DuckDB),
        policy.clone(),
    );
    assert!(direct.is_err());
    assert!(direct.unwrap_err().to_string().contains("sensitive"));

    // Dry-run validation path used by WASM `validate`.
    let dry_run = validate_query_with_options(
        query,
        Target::DuckDB,
        &Value::Null,
        &Value::Null,
        Schema::default_for(Target::DuckDB),
        policy.clone(),
    );
    assert!(dry_run.is_err());
    assert!(dry_run.unwrap_err().to_string().contains("sensitive"));

    // RPC compile path.
    let response = rpc_compile(
        query,
        "duckdb",
        Some(json!({"sensitive_fields": ["message"]})),
    );
    assert!(!rpc_accepted(&response));
    assert_eq!(response["error"]["code"], "compile_failed");
    assert!(response["error"]["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("sensitive"));
}

#[test]
fn parameter_limit_is_enforced_on_shared_render_path() {
    let policy = QueryPolicy::from_value(&json!({
        "max_parameters": 1,
    }))
    .expect("valid policy");
    let result = render_query_with_options(
        "from events | where event_id = $a or service = $b",
        Target::DuckDB,
        &json!({
            "a": {"type": "string", "value": "e1"},
            "b": {"type": "string", "value": "api"},
        }),
        &Value::Null,
        Schema::default_for(Target::DuckDB),
        policy,
    );
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("parameter count exceeds policy limit"));
}
