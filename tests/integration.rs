use loza_lql::{compile_to_clickhouse, compile_to_duckdb, parse, validate_query};

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
    assert!(sql.contains("PERCENTILE_CONT(0.95)"));
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
    assert!(sql.contains("SELECT"));
    // Should not be SELECT *
    assert!(!sql.contains("*"));
}

#[test]
fn timeseries_5m() {
    let sql = compile_to_duckdb("from events | timeseries 5m").unwrap();
    assert!(sql.contains("date_trunc"));
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
    assert!(sql.contains("toStartOfHour"));
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
    let sql = compile_to_duckdb(r#"from events | where message has "timeout""#).unwrap();
    assert!(sql.contains("LIKE '%timeout%'"));
}

#[test]
fn where_startswith() {
    let sql = compile_to_duckdb(r#"from events | where service startswith "check""#).unwrap();
    assert!(sql.contains("LIKE 'check%'"));
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
fn nested_field_access() {
    let sql = compile_to_duckdb(r#"from events | where user.id = "u123""#).unwrap();
    assert!(sql.contains("json_extract_string") || sql.contains("user.id"));
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
    assert!(duckdb.contains("regexp_matches"));

    let clickhouse =
        compile_to_clickhouse(r#"from events | where message not matches "timeout""#).unwrap();
    assert!(clickhouse.contains("NOT match"));
}

#[test]
fn compile_rejects_unknown_fields_before_sql_generation() {
    let error = compile_to_duckdb(r#"from events | where definitely_missing = "x""#).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn invalid_function_arity_returns_error_instead_of_panicking() {
    let error = compile_to_duckdb("from events | where isempty()").unwrap_err();
    assert!(error.to_string().contains("requires"));
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

    let clickhouse =
        compile_to_clickhouse(r#"from events | project replace(message, "old", "new")"#).unwrap();
    assert!(clickhouse.contains("replaceAll("));

    let duckdb_bin = compile_to_duckdb("from events | project bin(timestamp, 1h)").unwrap();
    assert!(duckdb_bin.contains("time_bucket("));
    let clickhouse_bin = compile_to_clickhouse("from events | project bin(timestamp, 1h)").unwrap();
    assert!(clickhouse_bin.contains("toStartOfInterval(\"timestamp\", toIntervalHour(1))"));
}
