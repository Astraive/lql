use lql::{
    compile_to_clickhouse, compile_to_duckdb, compile_to_postgres, compile_with_policy, parse,
    render_query, render_query_with_context, render_query_with_parameters, validate_query,
    QueryPolicy, Schema, Target, ValueType,
};

#[test]
fn every_catalog_builtin_renders_for_all_targets() {
    use lql::functions::{Arity, FUNCTION_CATALOG};

    for signature in FUNCTION_CATALOG {
        let count = match signature.arity {
            Arity::Exact(n) => n,
            Arity::Range(min, _) => min,
            Arity::AtLeast(min) => min,
        };
        let query = match signature.name {
            "ago" => "from events | project ago(1h)".to_string(),
            "bin" => "from events | project bin(timestamp, 1h)".to_string(),
            _ => format!(
                "from events | project {}({})",
                signature.name,
                vec!["1"; count].join(", ")
            ),
        };
        for target in [Target::DuckDB, Target::ClickHouse, Target::PostgreSQL] {
            let plan = render_query(&query, target)
                .unwrap_or_else(|error| panic!("{} failed for {:?}: {}", query, target, error));
            assert!(!plan.sql.is_empty(), "empty SQL for {}", query);
        }
    }
}

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
fn clickhouse_string_literals_escape_backslashes() {
    // A backslash followed by a quote must be escaped too. Without it, `\'` is
    // consumed as an escaped quote and the trailing text breaks out of the literal.
    let sql = compile_to_clickhouse(r#"from events | where service = "a\\' OR 1=1 --""#).unwrap();
    assert!(
        sql.contains(r#"'a\\\' OR 1=1 --'"#),
        "backslash must be doubled and the quote backslash-escaped so the payload stays inside the literal: {sql}"
    );
}

#[test]
fn clickhouse_compiler_literal_escapes_backslashes() {
    // Exercises compiler::clickhouse::compile directly (the location named in the
    // finding), alongside the public compile_to_clickhouse path above.
    let pipeline = parse(r#"from events | where service = "a\\' OR 1=1 --""#).unwrap();
    let sql =
        lql::compiler::clickhouse::compile(&pipeline, &lql::Schema::clickhouse_default()).unwrap();
    assert!(
        sql.contains(r#"q.`service` = 'a\\\' OR 1=1 --'"#),
        "direct compiler must double the backslash and escape the quote: {sql}"
    );
}

#[test]
fn clickhouse_string_literals_escape_single_quotes() {
    // ClickHouse escapes embedded single quotes with a backslash.
    let sql = compile_to_clickhouse(r#"from events | where service = "o'brien""#).unwrap();
    assert!(
        sql.contains(r"o\'brien"),
        "single quotes must be backslash-escaped: {sql}"
    );
}

#[test]
fn clickhouse_string_literals_escape_standalone_backslashes() {
    let sql = compile_to_clickhouse(r#"from events | where path = "C:\\temp\\new""#).unwrap();
    assert!(
        sql.contains("'C:\\\\temp\\\\new'"),
        "every backslash must be doubled: {sql}"
    );
}

#[test]
fn clickhouse_timeseries() {
    let sql = compile_to_clickhouse("from events | timeseries 1h").unwrap();
    assert!(sql.contains("toStartOfInterval"));
    assert!(sql.contains("toIntervalHour(1)"));
}

#[test]
fn inline_parameters_duckdb_value_containing_question_mark() {
    // The '?' inside the first literal is data, not a placeholder. Inlining must
    // not re-scan the already-substituted value and bind the second parameter
    // into it, leaving the real second placeholder unbound.
    let sql = compile_to_duckdb(r#"from events | where message = "what?" and duration_ms > 1000"#)
        .unwrap();
    assert!(
        sql.contains("'what?'"),
        "first literal must be preserved: {sql}"
    );
    assert!(
        sql.contains("> 1000"),
        "second parameter must be inlined: {sql}"
    );
    assert!(!sql.contains("> ?"), "no placeholder may remain: {sql}");
}

#[test]
fn inline_parameters_clickhouse_value_containing_later_marker() {
    // A literal containing the text of a later ClickHouse marker must survive
    // verbatim; replacement must happen in a single pass over the original SQL.
    let sql =
        compile_to_clickhouse(r#"from events | where message = "{p1:String}" and service = "api""#)
            .unwrap();
    assert!(
        sql.contains("'{p1:String}'"),
        "marker-like text in the first literal must be preserved: {sql}"
    );
    assert!(
        sql.contains("= 'api'"),
        "second parameter must be inlined: {sql}"
    );
}

#[test]
fn inline_parameters_postgres_value_containing_later_marker() {
    // Same hazard for PostgreSQL positional markers: '$2' inside an earlier
    // value must not consume the binding belonging to the second parameter.
    let sql =
        compile_to_postgres(r#"from events | where message = "$2" and service = "api""#).unwrap();
    assert!(
        sql.contains("'$2'"),
        "marker-like text must be preserved: {sql}"
    );
    assert!(
        sql.contains("= 'api'"),
        "second parameter must be inlined: {sql}"
    );
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
    assert!(duckdb.contains("LAST("));
    assert!(!duckdb.contains("LAST_VALUE("));
}

#[test]
fn first_and_last_aggregates_render_as_dialect_aggregates() {
    // FIRST_VALUE/LAST_VALUE are window functions requiring OVER(); they must
    // never be emitted for a summarize aggregation on any target.
    let duckdb_first = compile_to_duckdb("from events | summarize first(service)").unwrap();
    assert!(
        duckdb_first.contains("FIRST(q.\"service\")"),
        "{duckdb_first}"
    );
    assert!(!duckdb_first.contains("FIRST_VALUE"), "{duckdb_first}");

    let duckdb_last = compile_to_duckdb("from events | summarize last(service)").unwrap();
    assert!(duckdb_last.contains("LAST(q.\"service\")"), "{duckdb_last}");
    assert!(!duckdb_last.contains("LAST_VALUE"), "{duckdb_last}");

    let clickhouse_first = compile_to_clickhouse("from events | summarize first(service)").unwrap();
    assert!(
        clickhouse_first.contains("any(q.`service`)"),
        "{clickhouse_first}"
    );

    let clickhouse_last = compile_to_clickhouse("from events | summarize last(service)").unwrap();
    assert!(
        clickhouse_last.contains("anyLast(q.`service`)"),
        "{clickhouse_last}"
    );

    let postgres_first = compile_to_postgres("from events | summarize first(service)").unwrap();
    assert!(
        postgres_first.contains("(array_agg(q.\"service\"))[1]"),
        "{postgres_first}"
    );
    assert!(!postgres_first.contains("FIRST_VALUE"), "{postgres_first}");

    let postgres_last = compile_to_postgres("from events | summarize last(service)").unwrap();
    assert!(
        postgres_last.contains("array_agg(q.\"service\")"),
        "{postgres_last}"
    );
    assert!(!postgres_last.contains("LAST_VALUE"), "{postgres_last}");
}

#[test]
fn first_and_last_aggregates_preserve_argument_types_in_output_schema() {
    let document = serde_json::json!({
        "schema_version": "v1",
        "sources": {
            "events": {
                "physical": "events",
                "fields": [
                    {"name": "service", "field_type": "string", "physical": {"column": "service", "storage": "projection"}},
                    {"name": "duration_ms", "field_type": "float", "physical": {"column": "duration_ms", "storage": "projection"}}
                ]
            }
        }
    });
    let schema = Schema::from_document(&document, "events").unwrap();
    let plan = lql::render_query_with_options(
        "from events | summarize first_service = first(service), last_service = last(service), first_duration = first(duration_ms), last_duration = last(duration_ms)",
        Target::DuckDB,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
        schema,
        QueryPolicy::default(),
    )
    .unwrap();

    assert_eq!(
        plan.output_schema
            .iter()
            .map(|column| column.field_type.clone())
            .collect::<Vec<_>>(),
        vec![
            ValueType::String,
            ValueType::String,
            ValueType::Float,
            ValueType::Float
        ]
    );
    assert!(plan.output_schema.iter().all(|column| column.nullable));
    assert!(plan.sql.contains("FIRST(q.\"service\")"));
    assert!(plan.sql.contains("LAST(q.\"service\")"));
    assert!(plan.sql.contains("FIRST(q.\"duration_ms\")"));
    assert!(plan.sql.contains("LAST(q.\"duration_ms\")"));
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

/// Both scope predicates must be bound as parameters, in order, applied
/// directly to the source scan (not appended after aggregation/projection).
fn assert_source_scope(plan: &lql::CompiledQuery) {
    assert!(
        plan.sql.contains(
            r#"FROM "events") AS q WHERE ((q."collector" = ?) AND (q."environment" = ?))"#
        ),
        "scope predicate must filter the source scan directly: {}",
        plan.sql
    );
    let values: Vec<_> = plan.parameters.iter().map(|p| p.value.clone()).collect();
    assert_eq!(
        &values[..2],
        &[
            serde_json::json!("tenant-a"),
            serde_json::json!("production")
        ]
    );
    assert_eq!(plan.parameters[0].logical_type, ValueType::String);
    assert_eq!(plan.parameters[1].logical_type, ValueType::String);
}

#[test]
fn scoped_summarize_query_filters_at_source() {
    let plan = render_query_with_context(
        r#"from events | summarize count(), avg(duration_ms) by service | sort count desc"#,
        Target::DuckDB,
        &serde_json::json!({}),
        &serde_json::json!({"collector": "tenant-a", "environment": "production"}),
    )
    .unwrap();

    assert_source_scope(&plan);
    assert!(plan.sql.contains("GROUP BY"), "{}", plan.sql);
    assert!(plan.sql.contains("COUNT(*)"), "{}", plan.sql);
}

#[test]
fn scoped_project_query_dropping_scope_columns_still_filters_at_source() {
    // Regression: previously the scope `Where` was appended after `project`, so
    // `collector`/`environment` no longer existed and analysis failed.
    let plan = render_query_with_context(
        r#"from events | project service, duration_ms | sort duration_ms desc"#,
        Target::DuckDB,
        &serde_json::json!({}),
        &serde_json::json!({"collector": "tenant-a", "environment": "production"}),
    )
    .unwrap();

    assert_source_scope(&plan);
    assert!(plan.sql.contains("SELECT q.\"service\""), "{}", plan.sql);
}

#[test]
fn scoped_sort_query_filters_at_source() {
    let plan = render_query_with_context(
        r#"from events | sort timestamp desc | limit 10"#,
        Target::DuckDB,
        &serde_json::json!({}),
        &serde_json::json!({"collector": "tenant-a", "environment": "production"}),
    )
    .unwrap();

    assert_source_scope(&plan);
    assert!(plan.sql.contains("ORDER BY"), "{}", plan.sql);
    assert!(plan.sql.contains("LIMIT ?"), "{}", plan.sql);
}

#[test]
fn scoped_query_with_single_scope_field_filters_at_source() {
    let plan = render_query_with_context(
        r#"from events | summarize count() by service"#,
        Target::DuckDB,
        &serde_json::json!({}),
        &serde_json::json!({"collector": "tenant-a"}),
    )
    .unwrap();

    assert!(
        plan.sql
            .contains(r#"FROM "events") AS q WHERE (q."collector" = ?)"#),
        "single scope predicate must filter the source: {}",
        plan.sql
    );
    assert_eq!(plan.parameters.len(), 1);
    assert_eq!(plan.parameters[0].value, serde_json::json!("tenant-a"));
}

#[test]
fn postgres_uses_numbered_parameters_and_native_dialect() {
    let plan = render_query(
        "from events | where service startswith \"api\" and duration_ms >= 5 | take 2",
        Target::PostgreSQL,
    )
    .unwrap();
    assert!(plan.sql.contains("$1"));
    assert!(plan.sql.contains("$2"));
    let sql = compile_to_postgres("from events | where level = \"error\"").unwrap();
    assert!(sql.contains("'error'"));
}

// ── Policy-aware compatibility compile path (MASS-1001) ────────────────────

fn policy_query(
    input: &str,
    policy: QueryPolicy,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
) -> Result<String, lql::LqlError> {
    compile_with_policy(
        input,
        Target::DuckDB,
        parameters,
        scope,
        Schema::duckdb_default(),
        policy,
    )
}

#[test]
fn policy_compile_matches_legacy_wrapper_under_default_policy() {
    let input = r#"from events | where level = "error" | summarize count() by service | limit 3"#;
    let legacy = compile_to_duckdb(input).unwrap();
    let aware = policy_query(
        input,
        QueryPolicy::default(),
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    )
    .unwrap();
    assert_eq!(legacy, aware);
}

#[test]
fn policy_compile_rejects_sensitive_fields() {
    let policy = QueryPolicy {
        sensitive_fields: vec!["message".to_string()],
        ..QueryPolicy::default()
    };
    let error = policy_query(
        r#"from events | project message"#,
        policy,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    )
    .unwrap_err();
    assert!(error.to_string().contains("sensitive"), "{error}");

    // The legacy wrapper keeps its permissive default-policy behavior.
    assert!(compile_to_duckdb(r#"from events | project message"#).is_ok());
}

#[test]
fn policy_compile_rejects_fields_outside_allowlist() {
    let policy = QueryPolicy {
        allowed_fields: vec!["service".to_string()],
        ..QueryPolicy::default()
    };
    let error = policy_query(
        "from events | project duration_ms",
        policy,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("not allowed by policy"),
        "{error}"
    );
}

#[test]
fn policy_compile_enforces_allowed_sources() {
    let policy = QueryPolicy {
        allowed_sources: vec!["traces".to_string()],
        ..QueryPolicy::default()
    };
    let error = policy_query(
        "from events | limit 1",
        policy,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("not declared or allowed"),
        "{error}"
    );
}

#[test]
fn policy_compile_enforces_max_result_rows() {
    let policy = QueryPolicy {
        max_result_rows: Some(5),
        ..QueryPolicy::default()
    };
    let sql = policy_query(
        "from events | limit 100",
        policy,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
    )
    .unwrap();
    assert!(sql.contains("LIMIT 5"), "{sql}");
}

#[test]
fn policy_compile_enforces_max_parameters() {
    let policy = QueryPolicy {
        max_parameters: Some(1),
        ..QueryPolicy::default()
    };
    let parameters = serde_json::json!({
        "a": { "type": "int", "value": 1 },
        "b": { "type": "int", "value": 2 }
    });
    let error =
        policy_query("from events", policy, &parameters, &serde_json::Value::Null).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("parameter count exceeds policy limit"),
        "{error}"
    );
}

#[test]
fn policy_compile_applies_scope_filter() {
    let scope = serde_json::json!({ "collector": "edge", "environment": "prod" });
    let sql = policy_query(
        "from events | limit 10",
        QueryPolicy::default(),
        &serde_json::Value::Null,
        &scope,
    )
    .unwrap();
    assert!(sql.contains("collector"), "{sql}");
    assert!(sql.contains("'edge'"), "{sql}");
    assert!(sql.contains("environment"), "{sql}");
    assert!(sql.contains("'prod'"), "{sql}");
}

#[test]
fn compatibility_wrapper_still_enforces_default_source_policy() {
    assert!(compile_to_duckdb("from incidents | limit 1").is_err());
}

// ── Scope filter placement (MASS-1000 / issue #62) ──────────────────────────

#[test]
fn scope_filter_is_applied_before_column_replacing_transforms() {
    // The scope predicate references the base-table columns `collector` and
    // `environment`. `summarize` replaces the column set, so a scope `where`
    // appended at the end of the pipeline fails analysis with
    // "unknown field 'collector'". The filter belongs immediately after
    // `from`, before any transform, so it filters at scan time.
    let scope = serde_json::json!({ "collector": "edge", "environment": "prod" });

    let plan = render_query_with_context(
        r#"from events | summarize count() by service"#,
        Target::DuckDB,
        &serde_json::Value::Null,
        &scope,
    )
    .expect("scoped query with a column-replacing transform must compile");

    assert!(plan.sql.contains("WHERE"), "{sql}", sql = plan.sql);
    assert!(plan.sql.contains("collector"), "{sql}", sql = plan.sql);
    assert!(plan.sql.contains("environment"), "{sql}", sql = plan.sql);

    // The scope filter must run before aggregation, not after it.
    let where_pos = plan.sql.find("WHERE").expect("WHERE clause");
    let group_pos = plan.sql.find("GROUP BY").expect("GROUP BY clause");
    assert!(
        where_pos < group_pos,
        "scope filter must precede aggregation: {sql}",
        sql = plan.sql
    );

    // The trusted scope values are bound into the plan.
    let values: Vec<_> = plan
        .parameters
        .iter()
        .filter_map(|p| p.value.as_str())
        .collect();
    assert!(values.contains(&"edge"), "{values:?}");
    assert!(values.contains(&"prod"), "{values:?}");
}

// --- LQL-SEC-001: SQL injection regressions for the inlining wrappers -------

#[test]
fn single_quote_is_escaped_for_all_targets() {
    let source = r#"from events | where message = "it's""#;
    // Standard SQL dialects double the quote.
    assert!(compile_to_duckdb(source).unwrap().contains(r#"'it''s'"#));
    assert!(compile_to_postgres(source).unwrap().contains(r#"'it''s'"#));
    // ClickHouse uses backslash escaping inside string literals.
    assert!(compile_to_clickhouse(source)
        .unwrap()
        .contains(r#"'it\'s'"#));
}

#[test]
fn clickhouse_escapes_backslash_and_quote_breakout() {
    // A value ending in a single backslash must not escape the closing quote.
    let trailing = compile_to_clickhouse(r#"from events | where message = "\\""#).unwrap();
    assert!(trailing.contains(r#"'\\'"#), "{trailing}");

    // A backslash immediately before a quote is the classic ClickHouse
    // breakout; both characters must be escaped.
    let breakout = compile_to_clickhouse(r#"from events | where message = "\'""#).unwrap();
    assert!(breakout.contains(r#"'\\\''"#), "{breakout}");
}

#[test]
fn backslash_is_preserved_for_standard_sql_targets() {
    // Backslash is not an escape character in DuckDB/PostgreSQL string
    // literals; only the quote is doubled.
    let source = r#"from events | where message = "\'""#;
    assert!(compile_to_duckdb(source).unwrap().contains(r#"'\'''"#));
    assert!(compile_to_postgres(source).unwrap().contains(r#"'\'''"#));
}

#[test]
fn inlined_question_mark_is_not_resubstituted() {
    // The inlined `?` must not be consumed by the substitution of the second
    // parameter (the legacy sequential replacen corrupted it to ''SENTINEL'').
    let source = r#"from events | where message = "?" and service = "SENTINEL""#;
    for (sql, expected, sentinel) in [
        (
            compile_to_duckdb(source).unwrap(),
            r#"q."message" = '?'"#,
            r#"q."service" = 'SENTINEL'"#,
        ),
        (
            compile_to_clickhouse(source).unwrap(),
            "q.`message` = '?'",
            "q.`service` = 'SENTINEL'",
        ),
        (
            compile_to_postgres(source).unwrap(),
            r#"q."message" = '?'"#,
            r#"q."service" = 'SENTINEL'"#,
        ),
    ] {
        assert!(sql.contains(expected), "{sql}");
        assert!(sql.contains(sentinel), "{sql}");
    }
}

#[test]
fn inlined_numbered_placeholder_is_not_resubstituted() {
    // An inlined value containing `$2` must not be re-scanned when the `$2`
    // bind marker of a later parameter is substituted.
    let source = r#"from events | where message = "$2" and service = "SENTINEL""#;
    let expected = r#"q."message" = '$2'"#;
    let sentinel = r#"q."service" = 'SENTINEL'"#;
    let pg = compile_to_postgres(source).unwrap();
    assert!(pg.contains(expected), "{pg}");
    assert!(pg.contains(sentinel), "{pg}");

    let source = r#"from events | where message = "$1" and service = "SENTINEL""#;
    let expected = r#"q."message" = '$1'"#;
    let pg = compile_to_postgres(source).unwrap();
    assert!(pg.contains(expected), "{pg}");
    assert!(pg.contains(r#"q."service" = 'SENTINEL'"#), "{pg}");
}

#[test]
fn newline_payload_is_escaped_or_preserved_per_dialect() {
    let source = r#"from events | where message = "line1\nline2""#;
    // ClickHouse escapes the control character as `\n`.
    assert!(compile_to_clickhouse(source)
        .unwrap()
        .contains(r#"'line1\nline2'"#));
    // Standard SQL keeps the (harmless, still quoted) literal newline.
    assert!(compile_to_duckdb(source)
        .unwrap()
        .contains("q.\"message\" = 'line1\nline2'"));
    assert!(compile_to_postgres(source)
        .unwrap()
        .contains("q.\"message\" = 'line1\nline2'"));
}

#[test]
fn deeply_nested_input_returns_diagnostic_instead_of_crashing() {
    let input = format!(
        "from events | where {}1{}",
        "(".repeat(10_000),
        ")".repeat(10_000)
    );

    // The public parse API must return an error, not abort the process.
    assert!(parse(&input).is_err());

    // Structured diagnostics carry a dedicated, actionable code.
    let bundle = lql::parse_diagnostics(&input).unwrap_err();
    assert_eq!(bundle.diagnostics.len(), 1);
    assert_eq!(bundle.diagnostics[0].code, "LQL004");
    assert!(bundle.diagnostics[0].message.contains("nesting depth"));

    // The nesting budget is configurable through the public API.
    let shallow = "from events | where (((((1)))))";
    assert!(lql::parse_with_max_depth(shallow, 3).is_err());
    assert!(lql::parse_with_max_depth(shallow, 10).is_ok());
}
