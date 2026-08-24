use lql::{
    compile_to_clickhouse, compile_to_duckdb, parse, render_query, render_query_with_parameters,
    validate_query, Target, ValueType,
};

#[test]
fn smoke_from_events_limit() {
    let sql = compile_to_duckdb("from events | limit 10").unwrap();
    assert!(sql.contains("LIMIT 10"));
}

#[test]
fn where_string_equality() {
    let sql = compile_to_duckdb(r#"from events | where service = "checkout""#).unwrap();
    assert!(sql.contains("WHERE"));
    assert!(sql.contains("'checkout'"));
}

#[test]
fn where_numeric_comparison() {
    let sql = compile_to_duckdb("from events | where duration_ms > 1000").unwrap();
    assert!(sql.contains("duration_ms"));
    assert!(sql.contains("> 1000"));
}

#[test]
fn where_and_or() {
    let sql = compile_to_duckdb(
        r#"from events | where level = "error" and service = "api" or outcome = "timeout""#,
    )
    .unwrap();
    assert!(sql.contains("AND"));
    assert!(sql.contains("OR"));
}

#[test]
fn summarize_count_by() {
    let sql = compile_to_duckdb(r#"from events | summarize count() by service"#).unwrap();
    assert!(sql.contains("COUNT(*)"));
    assert!(sql.contains("GROUP BY"));
}

#[test]
fn summarize_avg() {
    let sql = compile_to_duckdb("from events | summarize avg(duration_ms) by event").unwrap();
    assert!(sql.contains("AVG("));
}

#[test]
fn summarize_percentile() {
    let sql = compile_to_duckdb("from events | summarize p95(duration_ms)").unwrap();
    assert!(sql.contains("quantile_cont"));
}

#[test]
fn sort_desc_limit() {
    let sql = compile_to_duckdb("from events | sort duration_ms desc | limit 20").unwrap();
    assert!(sql.contains("ORDER BY"));
    assert!(sql.contains("DESC"));
    assert!(sql.contains("LIMIT 20"));
}

#[test]
fn project_specific_fields() {
    let sql = compile_to_duckdb(r#"from events | project service, event, level"#).unwrap();
    assert!(sql.contains("q.\"service\""));
    assert!(sql.contains("q.\"event\""));
    assert!(sql.contains("q.\"level\""));
}

#[test]
fn timeseries_5m() {
    let sql = compile_to_duckdb("from events | timeseries 5m").unwrap();
    assert!(sql.contains("time_bucket((5 * INTERVAL '1 minute')"));
    assert!(sql.contains("COUNT(*)"));
    assert!(sql.contains("GROUP BY"));
}

#[test]
fn clickhouse_output() {
    let sql = compile_to_clickhouse(r#"from events | where level = "error" | limit 5"#).unwrap();
    assert!(sql.contains("LIMIT 5"));
    assert!(sql.contains("'error'"));
}

#[test]
fn clickhouse_percentile() {
    let sql = compile_to_clickhouse("from events | summarize p95(duration_ms)").unwrap();
    assert!(sql.contains("quantile(0.95)"));
}

#[test]
fn clickhouse_timeseries() {
    let sql = compile_to_clickhouse("from events | timeseries 1h").unwrap();
    assert!(sql.contains("toStartOfInterval"));
    assert!(sql.contains("toIntervalHour(1)"));
}

#[test]
fn pipeline_parse_ast() {
    let pipeline = parse(r#"from events | where level = "error" | summarize count() by service | sort count desc | limit 10"#).unwrap();
    assert_eq!(pipeline.statements.len(), 5);
}

#[test]
fn validate_valid_query() {
    assert!(validate_query(r#"from events | where service = "checkout" | limit 10"#).is_ok());
}

#[test]
fn validate_unknown_field() {
    assert!(validate_query(r#"from events | where totally_fake_field = "x""#).is_err());
}

#[test]
fn where_has_string() {
    let sql = compile_to_duckdb(r#"from events | where message has "100%_done""#).unwrap();
    assert!(sql.contains("contains("));
    assert!(sql.contains("'100%_done'"));
}

#[test]
fn where_startswith() {
    let sql = compile_to_duckdb(r#"from events | where service startswith "check_""#).unwrap();
    assert!(sql.contains("starts_with("));
    assert!(sql.contains("'check_'"));
}

#[test]
fn summarize_with_alias() {
    let sql = compile_to_duckdb(r#"from events | summarize cnt = count() by service"#).unwrap();
    assert!(sql.contains("AS"));
}

#[test]
fn where_in_list() {
    let sql = compile_to_duckdb(r#"from events | where level in ("error", "fatal")"#).unwrap();
    assert!(sql.contains("IN ("));
}

#[test]
fn nested_field_access_requires_declared_dynamic_root() {
    assert!(validate_query(r#"from events | where not_declared.id = "u123""#).is_err());
}
#[test]
fn take_alias_emits_limit() {
    let sql = compile_to_duckdb("from events | take 7").unwrap();
    assert!(sql.contains("LIMIT 7"));
}

#[test]
fn distinct_projects_unique_values() {
    let sql = compile_to_duckdb("from events | distinct service").unwrap();
    assert!(sql.starts_with("SELECT DISTINCT"));
    assert!(sql.contains("\"service\""));
}

#[test]
fn between_is_inclusive_and_clickhouse_parity_exists() {
    let duckdb = compile_to_duckdb("from events | where duration_ms between 100 and 200").unwrap();
    assert!(duckdb.contains("\"duration_ms\" >= 100"));
    assert!(duckdb.contains("\"duration_ms\" <= 200"));

    let clickhouse =
        compile_to_clickhouse("from events | where duration_ms not between 100 and 200").unwrap();
    assert!(clickhouse.contains("NOT ("));
    assert!(clickhouse.contains(">= 100"));
    assert!(clickhouse.contains("<= 200"));
}

#[test]
fn not_in_and_regex_match_compile_for_both_targets() {
    let duckdb = compile_to_duckdb(
        r#"from events | where service not in ("api", "web") and message matches "^fail""#,
    )
    .unwrap();
    assert!(duckdb.contains("NOT IN"));
    assert!(duckdb.contains("regexp_matches("));

    let clickhouse =
        compile_to_clickhouse(r#"from events | where message not matches "timeout""#).unwrap();
    assert!(clickhouse.contains("NOT match("));
}

#[test]
fn compile_rejects_unknown_fields_before_sql_generation() {
    let error = compile_to_duckdb(r#"from events | where definitely_missing = "x""#).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn invalid_function_arity_returns_error_instead_of_panicking() {
    let error = compile_to_duckdb("from events | where isempty()").unwrap_err();
    assert!(error.to_string().contains("invalid argument count"));
}

#[test]
fn validate_rejects_invalid_function_arity() {
    assert!(validate_query("from events | where isempty()").is_err());
}

#[test]
fn string_and_array_builtins_compile_for_both_targets() {
    let duckdb = compile_to_duckdb(
        r#"from events | project replace(message, "old", "new"), split(service, "/"), array_length(attrs)"#,
    )
    .unwrap();
    assert!(duckdb.contains("REPLACE("));
    assert!(duckdb.contains("STRING_SPLIT("));
    assert!(duckdb.contains("ARRAY_LENGTH("));

    let clickhouse = compile_to_clickhouse(
        r#"from events | project replace(message, "old", "new"), split(service, "/"), array_length(attrs)"#,
    )
    .unwrap();
    assert!(clickhouse.contains("replaceAll("));
    assert!(clickhouse.contains("splitByString("));
    assert!(clickhouse.contains("length("));
    let duckdb_bin = compile_to_duckdb("from events | project bin(timestamp, 1h)").unwrap();
    assert!(duckdb_bin.contains("time_bucket("));
    let clickhouse_bin = compile_to_clickhouse("from events | project bin(timestamp, 1h)").unwrap();
    assert!(clickhouse_bin.contains("toStartOfInterval("));
}

#[test]
fn parameter_order_follows_generated_sql_text() {
    let plan = render_query(
        "from events | where timestamp >= ago(24h) | summarize p95(duration_ms) by service",
        Target::DuckDB,
    )
    .unwrap();
    assert!(plan.sql.find("quantile_cont(").unwrap() < plan.sql.find("NOW() -").unwrap());
    assert_eq!(plan.parameters[0].value, serde_json::json!(0.95));
    assert_eq!(plan.parameters[1].value, serde_json::json!(24));
}

#[test]
fn repeated_positional_expressions_repeat_their_bindings() {
    let empty = render_query(r#"from events | project isempty("x")"#, Target::DuckDB).unwrap();
    assert_eq!(empty.sql.matches('?').count(), 2);
    assert_eq!(empty.parameters.len(), 2);
    assert_eq!(empty.parameters[0], empty.parameters[1]);

    let between = render_query("from events | where 5 between 1 and 10", Target::DuckDB).unwrap();
    assert_eq!(between.sql.matches('?').count(), 4);
    assert_eq!(
        between
            .parameters
            .iter()
            .map(|parameter| parameter.value.clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!(5),
            serde_json::json!(1),
            serde_json::json!(5),
            serde_json::json!(10)
        ]
    );
}

#[test]
fn null_comparisons_use_sql_null_predicates() {
    let equals = render_query("from events | where error = null", Target::DuckDB).unwrap();
    assert!(equals.sql.contains("IS NULL"));
    assert!(equals.parameters.is_empty());

    let not_equals = render_query("from events | where null != error", Target::ClickHouse).unwrap();
    assert!(not_equals.sql.contains("IS NOT NULL"));
    assert!(not_equals.parameters.is_empty());
}

#[test]
fn duration_arithmetic_and_bucketing_use_native_intervals() {
    let duckdb = compile_to_duckdb(
        "from events | where timestamp >= now() - 24h | project bin(timestamp, 5m)",
    )
    .unwrap();
    assert!(duckdb.contains("NOW() - (24 * INTERVAL '1 hour')"));
    assert!(duckdb.contains("time_bucket((5 * INTERVAL '1 minute')"));

    let clickhouse = compile_to_clickhouse(
        "from events | where timestamp >= ago(24h) | project bin(timestamp, 5m)",
    )
    .unwrap();
    assert!(clickhouse.contains("NOW() - toIntervalHour(24)"));
    assert!(clickhouse.contains("toStartOfInterval"));
    assert!(clickhouse.contains("toIntervalMinute(5)"));
}

#[test]
fn typed_timestamp_and_dynamic_parameters_are_preserved() {
    let timestamp = render_query_with_parameters(
        "from events | where timestamp >= $from",
        Target::ClickHouse,
        &serde_json::json!({
            "from": {"type": "timestamp", "value": "2026-01-01T00:00:00Z"}
        }),
    )
    .unwrap();
    assert_eq!(timestamp.parameters[0].logical_type, ValueType::Timestamp);
    assert_eq!(
        timestamp.parameters[0].value,
        serde_json::json!("2026-01-01T00:00:00Z")
    );
    assert!(timestamp.sql.contains("DateTime64(3)"));

    let dynamic = render_query_with_parameters(
        "from events | project $payload",
        Target::DuckDB,
        &serde_json::json!({
            "payload": {"type": "dynamic", "value": {"attempt": 2, "ok": true}}
        }),
    )
    .unwrap();
    assert_eq!(dynamic.parameters[0].logical_type, ValueType::Dynamic);
    assert_eq!(
        dynamic.parameters[0].value,
        serde_json::json!({"attempt": 2, "ok": true})
    );
}

#[test]
fn last_aggregate_is_reachable() {
    let duckdb = compile_to_duckdb("from events | summarize last(service)").unwrap();
    assert!(duckdb.contains("LAST_VALUE("));
}

#[test]
fn log_has_natural_logarithm_semantics_on_both_targets() {
    let duckdb = compile_to_duckdb("from events | project log(duration_ms)").unwrap();
    assert!(duckdb.contains("LN("));
    let clickhouse = compile_to_clickhouse("from events | project log(duration_ms)").unwrap();
    assert!(clickhouse.contains("log("));
}

#[test]
fn semantic_corpus_v01_keeps_target_contracts() {
    let cases = [
        ("from events | where level = \"error\" | take 5", "LIMIT"),
        ("from events | summarize count() by service", "COUNT"),
        ("from events | project to_string(status_code)", "CAST"),
    ];
    for (source, duckdb_marker) in cases {
        let duckdb = compile_to_duckdb(source).unwrap();
        assert!(
            duckdb.contains(duckdb_marker),
            "DuckDB corpus case: {source}"
        );
        let clickhouse = compile_to_clickhouse(source).unwrap();
        assert!(!clickhouse.is_empty(), "ClickHouse corpus case: {source}");
    }
}
