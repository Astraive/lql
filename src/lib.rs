//! Loza Query Language (LQL)
//!
//! Public compilation follows one explicit sequence:
//! `parse` -> `analyze` -> `compiler::render`.

pub mod ast;
pub mod compiler;
pub mod error;
pub mod escape;
pub mod functions;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod protocol;
pub mod schema;
pub mod validate;

use crate::ast::Literal;
use std::collections::HashSet;

pub use ast::{AggExpr, Expr, Pipeline, Source, Statement};
pub use compiler::{BoundValue, CompiledQuery, Target};
pub use error::{Diagnostic, DiagnosticBundle, LqlError, Severity, Span};
pub use ir::{
    analyze, AnalysisOptions, QueryPolicy, RelationNode, TypedColumn, TypedExpr, TypedPipeline,
    ValueType,
};
pub use schema::Schema;

/// Default maximum expression nesting depth (see [`parser::DEFAULT_MAX_DEPTH`]).
pub use parser::DEFAULT_MAX_DEPTH;

/// Parse LQL input into an AST pipeline using the default expression nesting
/// budget of [`DEFAULT_MAX_DEPTH`].
pub fn parse(input: &str) -> Result<Pipeline, LqlError> {
    parse_with_max_depth(input, DEFAULT_MAX_DEPTH)
}

/// Parse LQL input into an AST pipeline with an explicit expression nesting
/// budget. Exceeding `max_depth` returns [`LqlError::RecursionLimitExceeded`]
/// instead of overflowing the stack.
pub fn parse_with_max_depth(input: &str, max_depth: usize) -> Result<Pipeline, LqlError> {
    let tokens = lexer::Lexer::new(input).tokenize()?;
    parser::Parser::with_max_depth(tokens, max_depth).parse()
}

/// Parse and return structured diagnostics with source-relative byte spans,
/// using the default expression nesting budget.
pub fn parse_diagnostics(input: &str) -> Result<Pipeline, DiagnosticBundle> {
    parse_diagnostics_with_max_depth(input, DEFAULT_MAX_DEPTH)
}

/// Parse into structured diagnostics with an explicit expression nesting budget.
pub fn parse_diagnostics_with_max_depth(
    input: &str,
    max_depth: usize,
) -> Result<Pipeline, DiagnosticBundle> {
    parse_with_max_depth(input, max_depth).map_err(|error| structured_error(input, error))
}

/// Analyze an AST against explicit schema, target and policy options.
pub fn analyze_pipeline(
    pipeline: &Pipeline,
    options: &AnalysisOptions,
) -> Result<TypedPipeline, DiagnosticBundle> {
    ir::analyze(pipeline, options)
}

/// Parse and analyze source while relocating diagnostics to UTF-8 byte spans.
pub fn analyze_source(
    input: &str,
    options: &AnalysisOptions,
) -> Result<TypedPipeline, DiagnosticBundle> {
    let pipeline = parse_diagnostics(input)?;
    ir::analyze(&pipeline, options).map_err(|bundle| relocate_diagnostics(input, bundle))
}

/// Parse, analyze and render a parameterized query plan.
pub fn render_query(input: &str, target: Target) -> Result<CompiledQuery, DiagnosticBundle> {
    let options = match target {
        Target::DuckDB => AnalysisOptions::duckdb(),
        Target::ClickHouse => AnalysisOptions::clickhouse(),
        Target::PostgreSQL => AnalysisOptions::postgres(),
    };
    let typed = analyze_source(input, &options)?;
    compiler::render(&typed, target)
}
/// Parse, bind named `$name` parameters, and render with trusted scope.
pub fn render_query_with_parameters(
    input: &str,
    target: Target,
    parameters: &serde_json::Value,
) -> Result<CompiledQuery, DiagnosticBundle> {
    render_query_with_context(input, target, parameters, &serde_json::Value::Null)
}

pub fn render_query_with_context(
    input: &str,
    target: Target,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
) -> Result<CompiledQuery, DiagnosticBundle> {
    render_query_with_options(
        input,
        target,
        parameters,
        scope,
        Schema::default_for(target),
        QueryPolicy::default(),
    )
}

pub fn render_query_with_options(
    input: &str,
    target: Target,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
    schema: Schema,
    policy: QueryPolicy,
) -> Result<CompiledQuery, DiagnosticBundle> {
    let mut pipeline = parse_diagnostics(input)?;
    prepare_pipeline(&mut pipeline, parameters, scope, &policy)
        .map_err(|bundle| relocate_diagnostics(input, bundle))?;
    let options = AnalysisOptions {
        schema,
        target,
        policy,
        language_version: "0.1".to_string(),
    };
    let typed = analyze_pipeline(&pipeline, &options)
        .map_err(|bundle| relocate_diagnostics(input, bundle))?;
    compiler::render(&typed, target).map_err(|bundle| relocate_diagnostics(input, bundle))
}

/// Apply the parameter, scope and policy pre-analysis transformations to a
/// parsed pipeline. This is the single shared policy-application step used by
/// both the structured (`render_query_with_options`) and compatibility
/// (`compile_with_policy`) entry points.
fn prepare_pipeline(
    pipeline: &mut Pipeline,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
    policy: &QueryPolicy,
) -> Result<(), DiagnosticBundle> {
    if let Some(max_parameters) = policy.max_parameters {
        let count = parameters
            .as_object()
            .map(|values| values.len())
            .unwrap_or(0);
        if count > max_parameters {
            return Err(DiagnosticBundle::one(Diagnostic::error(
                "LQL122",
                "parameter count exceeds policy limit",
                Some(Span::empty(0)),
            )));
        }
    }
    bind_pipeline(pipeline, parameters)?;
    apply_scope_filter(pipeline, scope)?;
    Ok(())
}

/// Apply trusted scope predicates at the source stage of the pipeline.
///
/// The filter is inserted immediately after the `from` source, before any
/// `where`, `summarize`, `project`, `sort` or other transformation. This
/// guarantees the predicates always run against the raw source columns
/// (`collector` and `environment`), which:
/// - keeps scoping effective for every pipeline shape, and
/// - avoids analysis failures when those columns are removed or renamed by a
///   later projection or aggregation.
///
/// Appending the filter after aggregation/projection (the previous behavior)
/// could either fail closed or silently apply tenant scope to the wrong
/// relation.
fn apply_scope_filter(
    pipeline: &mut Pipeline,
    scope: &serde_json::Value,
) -> Result<(), DiagnosticBundle> {
    let Some(scope) = scope.as_object() else {
        return Ok(());
    };
    let mut predicates = Vec::new();
    for (field, value) in [("collector", "collector"), ("environment", "environment")] {
        if let Some(value) = scope.get(value).and_then(serde_json::Value::as_str) {
            predicates.push(Expr::BinaryOp {
                left: Box::new(Expr::Column(field.to_string())),
                op: ast::BinOp::Eq,
                right: Box::new(Expr::Literal(Literal::String(value.to_string()))),
            });
        }
    }
    if let Some(predicate) = predicates.into_iter().reduce(|left, right| Expr::BinaryOp {
        left: Box::new(left),
        op: ast::BinOp::And,
        right: Box::new(right),
    }) {
        // The scope predicate references base-table columns (`collector`,
        // `environment`). Transforms such as Project/Summarize/Timeseries
        // replace the column set, so a trailing `Where` would either fail
        // type checking with "unknown field 'collector'" or filter after
        // aggregation. Insert it immediately after `From` so it filters at
        // scan time, before any transform runs.
        let insert_at = pipeline
            .statements
            .iter()
            .position(|statement| matches!(statement, Statement::From(_)))
            .map(|index| index + 1)
            .unwrap_or(pipeline.statements.len());
        pipeline
            .statements
            .insert(insert_at, Statement::Where(predicate));
    }
    Ok(())
}

fn bind_pipeline(
    pipeline: &mut Pipeline,
    parameters: &serde_json::Value,
) -> Result<(), DiagnosticBundle> {
    let mut used = HashSet::new();
    for statement in &pipeline.statements {
        collect_statement_parameters(statement, &mut used);
    }
    if let Some(values) = parameters.as_object() {
        if let Some(unknown) = values.keys().find(|name| !used.contains(*name)) {
            return Err(DiagnosticBundle::one(Diagnostic::error(
                "LQL104",
                format!("unknown parameter '${unknown}'"),
                Some(Span::empty(0)),
            )));
        }
    }
    for statement in &mut pipeline.statements {
        match statement {
            Statement::Where(expr) => bind_expr(expr, parameters)?,
            Statement::Summarize { aggregations, by } => {
                for aggregation in aggregations {
                    if let Some(expr) = &mut aggregation.arg {
                        bind_expr(expr, parameters)?;
                    }
                }
                for expr in by {
                    bind_expr(expr, parameters)?;
                }
            }
            Statement::Sort { field, .. } => bind_expr(field, parameters)?,
            Statement::Distinct(exprs) | Statement::Project(exprs) => {
                for expr in exprs {
                    bind_expr(expr, parameters)?;
                }
            }
            Statement::Extend { expr, .. } => bind_expr(expr, parameters)?,
            Statement::Top { by, .. } => {
                for expr in by {
                    bind_expr(expr, parameters)?;
                }
            }
            Statement::From(_)
            | Statement::Limit(_)
            | Statement::Offset(_)
            | Statement::Timeseries { .. } => {}
        }
    }
    Ok(())
}

fn collect_statement_parameters(statement: &Statement, used: &mut HashSet<String>) {
    match statement {
        Statement::Where(expr) | Statement::Sort { field: expr, .. } => {
            collect_expr_parameters(expr, used)
        }
        Statement::Summarize { aggregations, by } => {
            for aggregation in aggregations {
                if let Some(expr) = &aggregation.arg {
                    collect_expr_parameters(expr, used);
                }
            }
            for expr in by {
                collect_expr_parameters(expr, used);
            }
        }
        Statement::Distinct(exprs) | Statement::Project(exprs) => {
            for expr in exprs {
                collect_expr_parameters(expr, used);
            }
        }
        Statement::Extend { expr, .. } => collect_expr_parameters(expr, used),
        Statement::Top { by, .. } => {
            for expr in by {
                collect_expr_parameters(expr, used);
            }
        }
        Statement::From(_)
        | Statement::Limit(_)
        | Statement::Offset(_)
        | Statement::Timeseries { .. } => {}
    }
}

fn collect_expr_parameters(expr: &Expr, used: &mut HashSet<String>) {
    match expr {
        Expr::Parameter(name) => {
            used.insert(name.clone());
        }
        Expr::BinaryOp { left, right, .. } => {
            collect_expr_parameters(left, used);
            collect_expr_parameters(right, used);
        }
        Expr::UnaryOp { expr, .. } => collect_expr_parameters(expr, used),
        Expr::Function { args, .. } => {
            for arg in args {
                collect_expr_parameters(arg, used);
            }
        }
        Expr::InList { expr, values, .. } => {
            collect_expr_parameters(expr, used);
            for value in values {
                collect_expr_parameters(value, used);
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            collect_expr_parameters(expr, used);
            collect_expr_parameters(low, used);
            collect_expr_parameters(high, used);
        }
        Expr::Column(_) | Expr::Literal(_) | Expr::Wildcard => {}
    }
}

fn bind_expr(expr: &mut Expr, parameters: &serde_json::Value) -> Result<(), DiagnosticBundle> {
    match expr {
        Expr::Parameter(name) => {
            let Some(value) = parameters.get(name.as_str()) else {
                return Err(DiagnosticBundle::one(Diagnostic::error(
                    "LQL103",
                    format!("missing parameter '${name}'"),
                    Some(Span::empty(0)),
                )));
            };
            *expr = parameter_literal(name, value)?;
        }
        Expr::BinaryOp { left, right, .. } => {
            bind_expr(left, parameters)?;
            bind_expr(right, parameters)?;
        }
        Expr::UnaryOp { expr, .. } => bind_expr(expr, parameters)?,
        Expr::Function { args, .. } => {
            for arg in args {
                bind_expr(arg, parameters)?;
            }
        }
        Expr::InList {
            expr: value,
            values,
            ..
        } => {
            bind_expr(value, parameters)?;
            for arg in values {
                bind_expr(arg, parameters)?;
            }
        }
        Expr::Between {
            expr: value,
            low,
            high,
            ..
        } => {
            bind_expr(value, parameters)?;
            bind_expr(low, parameters)?;
            bind_expr(high, parameters)?;
        }
        Expr::Column(_) | Expr::Literal(_) | Expr::Wildcard => {}
    }
    Ok(())
}

fn parameter_literal(name: &str, value: &serde_json::Value) -> Result<Expr, DiagnosticBundle> {
    let typed = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let raw = value.get("value").unwrap_or(&serde_json::Value::Null);
    let literal = match typed {
        "string" => raw.as_str().map(|v| Literal::String(v.to_string())),
        "timestamp" => raw.as_str().map(|v| Literal::Timestamp(v.to_string())),
        "int" => raw.as_i64().map(Literal::Integer),
        "float" => raw.as_f64().map(Literal::Float),
        "bool" => raw.as_bool().map(Literal::Bool),
        "null" => Some(Literal::Null),
        "duration" => raw.as_u64().map(|v| {
            Literal::Duration(ast::Duration {
                value: v,
                unit: ast::DurationUnit::Milliseconds,
            })
        }),
        "dynamic" => Some(Literal::Dynamic(raw.clone())),
        _ => None,
    };
    literal.map(Expr::Literal).ok_or_else(|| {
        DiagnosticBundle::one(Diagnostic::error(
            "LQL103",
            format!("parameter '${name}' has an invalid or incompatible value"),
            Some(Span::empty(0)),
        ))
    })
}

/// Map a semantic diagnostic's placeholder span onto real source bytes.
///
/// Diagnostics produced by analysis do not yet carry spans from the AST, so
/// this resolves the token named in the message against the original query.
/// Diagnostics that already have a real span are left untouched.
fn relocate_diagnostics(input: &str, mut bundle: DiagnosticBundle) -> DiagnosticBundle {
    for diagnostic in &mut bundle.diagnostics {
        if diagnostic.primary_span != Some(Span::empty(0)) {
            continue;
        }
        let message = diagnostic.message.as_bytes();
        let Some(open) = diagnostic.message.find('\'') else {
            continue;
        };
        let Some(close_offset) = diagnostic.message[open + 1..].find('\'') else {
            continue;
        };
        let close = open + 1 + close_offset;
        // Include a leading `$` so named parameters map to `$name` in source.
        let needle_start = if open > 0 && message[open - 1] == b'$' {
            open - 1
        } else {
            open + 1
        };
        let needle = &diagnostic.message[needle_start..close];
        if let Some(start) = input.find(needle) {
            diagnostic.primary_span = Some(Span::new(start, start + needle.len()).clamp_to(input));
        }
    }
    bundle
}

/// Compile with an explicit policy, scope and parameters.
///
/// This is the policy-aware counterpart of the compatibility `compile_to_*`
/// wrappers. It shares the full `render_query_with_options` pipeline
/// (parameter binding, scope filtering, and policy-aware analysis) and returns
/// a SQL string with bind values inlined. Use this for untrusted input:
/// `allowed_sources`, `allowed_fields`, `sensitive_fields`, `max_result_rows`,
/// `max_parameters` and the supplied scope are all enforced before rendering.
pub fn compile_with_policy(
    input: &str,
    target: Target,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
    schema: Schema,
    policy: QueryPolicy,
) -> Result<String, LqlError> {
    let mut pipeline = parse(input)?;
    prepare_pipeline(&mut pipeline, parameters, scope, &policy).map_err(|bundle| {
        LqlError::Compile {
            message: bundle.to_string(),
            span: bundle.diagnostics.first().and_then(|d| d.primary_span),
        }
    })?;
    compiler::compile_with_policy(&pipeline, target, &schema, policy)
}

/// Compile a query through the same analyze + policy + parameter-binding path
/// used by `render_query*`, then inline the bind values for the legacy
/// string-returning API. This is the single path behind the WASM and legacy
/// compile wrappers, so neither can bypass policy or use a different schema.
fn compile_compat(input: &str, target: Target) -> Result<String, LqlError> {
    let plan = render_query_with_options(
        input,
        target,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
        Schema::default_for(target),
        QueryPolicy::default(),
    )
    .map_err(|bundle| LqlError::Compile {
        message: bundle.to_string(),
        span: bundle.diagnostics.first().and_then(|d| d.primary_span),
    })?;
    Ok(compiler::inline_parameters(&plan))
}

/// Compile a query using the compatibility SQL-only wrapper.
///
/// Uses [`QueryPolicy::default()`], which is permissive. For untrusted input,
/// use [`compile_with_policy`] with an explicit policy and scope.
/// Compile a query using the legacy compatibility SQL-only wrapper.
///
/// # Security
/// This wrapper inlines values directly into the SQL string. It escapes string
/// literals for the target dialect, but it is intended for **trusted tooling
/// and static queries only**. For untrusted input, use [`render_query`] (or
/// [`render_query_with_parameters`]) and pass the returned bind parameters to
/// your driver instead of inlining them.
pub fn compile_to_duckdb(input: &str) -> Result<String, LqlError> {
    compile_compat(input, Target::DuckDB)
}

/// Compile a query using the compatibility SQL-only wrapper.
///
/// Uses [`QueryPolicy::default()`], which is permissive. For untrusted input,
/// use [`compile_with_policy`] with an explicit policy and scope.
/// Compile a query using the legacy compatibility SQL-only wrapper.
///
/// # Security
/// This wrapper inlines values directly into the SQL string. It escapes string
/// literals for the target dialect (including ClickHouse backslash escaping),
/// but it is intended for **trusted tooling and static queries only**. For
/// untrusted input, use [`render_query`] (or [`render_query_with_parameters`])
/// and pass the returned bind parameters to your driver instead of inlining.
pub fn compile_to_clickhouse(input: &str) -> Result<String, LqlError> {
    compile_compat(input, Target::ClickHouse)
}

/// Compile a query using the compatibility SQL-only wrapper.
///
/// Uses [`QueryPolicy::default()`], which is permissive. For untrusted input,
/// use [`compile_with_policy`] with an explicit policy and scope.
/// Compile a query using the legacy compatibility SQL-only wrapper.
///
/// # Security
/// This wrapper inlines values directly into the SQL string. It escapes string
/// literals for the target dialect, but it is intended for **trusted tooling
/// and static queries only**. For untrusted input, use [`render_query`] (or
/// [`render_query_with_parameters`]) and pass the returned bind parameters to
/// your driver instead of inlining them.
pub fn compile_to_postgres(input: &str) -> Result<String, LqlError> {
    compile_compat(input, Target::PostgreSQL)
}

/// Validate a query with explicit parameters, scope, schema and policy.
/// This is a dry run of `render_query_with_options` and retains every
/// diagnostic from the full analysis pipeline.
pub fn validate_query_with_options(
    input: &str,
    target: Target,
    parameters: &serde_json::Value,
    scope: &serde_json::Value,
    schema: Schema,
    policy: QueryPolicy,
) -> Result<(), DiagnosticBundle> {
    render_query_with_options(input, target, parameters, scope, schema, policy).map(|_| ())
}

/// Validate a query against the canonical DuckDB defaults, retaining all
/// diagnostics. This is the validation path shared by the WASM binding.
pub fn validate_query_diagnostics(input: &str) -> Result<(), DiagnosticBundle> {
    validate_query_with_options(
        input,
        Target::DuckDB,
        &serde_json::Value::Null,
        &serde_json::Value::Null,
        Schema::default_for(Target::DuckDB),
        QueryPolicy::default(),
    )
}

/// Validate a LQL query against the canonical default event schema.
pub fn validate_query(input: &str) -> Result<(), LqlError> {
    validate_query_diagnostics(input).map_err(|bundle| LqlError::Compile {
        message: bundle.to_string(),
        span: bundle.diagnostics.first().and_then(|d| d.primary_span),
    })
}

/// Compatibility compile entry point using [`QueryPolicy::default()`].
///
/// For untrusted input, use [`compile_with_policy`] with an explicit policy
/// and scope instead.
pub fn compile(input: &str, target: Target) -> Result<String, LqlError> {
    compile_compat(input, target)
}

/// Get all known field names for autocomplete.
pub fn known_fields() -> Vec<String> {
    let schema = Schema::default_for(Target::DuckDB);
    schema.columns.keys().cloned().collect()
}

fn structured_error(input: &str, error: LqlError) -> DiagnosticBundle {
    let mut diagnostic = error.diagnostic();
    let needle = match &error {
        LqlError::UnknownField { name, .. } => Some(name.as_str()),
        LqlError::UnknownFunction { name, .. } => Some(name.as_str()),
        _ => None,
    };
    if let Some(needle) = needle {
        if let Some(start) = input.find(needle) {
            diagnostic.primary_span = Some(Span::new(start, start + needle.len()));
        }
    }
    DiagnosticBundle::one(diagnostic)
}

// ── WASM bindings ──────────────────────────────────────────────────────────
#[cfg(target_arch = "wasm32")]
mod wasm {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub fn compile_to_duckdb(input: &str) -> Result<String, JsValue> {
        super::compile_to_duckdb(input).map_err(|e| JsValue::from_str(&e.to_string()))
    }
    #[wasm_bindgen]
    pub fn compile_to_clickhouse(input: &str) -> Result<String, JsValue> {
        super::compile_to_clickhouse(input).map_err(|e| JsValue::from_str(&e.to_string()))
    }
    #[wasm_bindgen]
    pub fn validate(input: &str) -> Result<String, JsValue> {
        super::validate_query_diagnostics(input)
            .map(|_| "{}".to_string())
            .map_err(|bundle| {
                let message = serde_json::to_string(&bundle.diagnostics)
                    .unwrap_or_else(|_| bundle.to_string());
                JsValue::from_str(&message)
            })
    }
    #[wasm_bindgen]
    pub fn known_fields() -> String {
        serde_json::to_string(&super::known_fields()).unwrap_or_else(|_| "[]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pipeline_duckdb() {
        let sql = compile_to_duckdb(
            r#"from events | where level = "error" and service = "checkout" | summarize count(), avg(duration_ms) by event | sort count desc | limit 20"#,
        ).unwrap();
        assert!(sql.contains("SELECT"));
        assert!(sql.contains("events"));
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("GROUP BY"));
        assert!(sql.contains("ORDER BY"));
        assert!(sql.contains("LIMIT 20"));
    }

    #[test]
    fn full_pipeline_clickhouse() {
        let sql = compile_to_clickhouse(
            r#"from events | where level = "error" | summarize p95(duration_ms) by service"#,
        )
        .unwrap();
        assert!(sql.contains("quantile(0.95)"));
    }

    #[test]
    fn parse_only() {
        let pipeline = parse(r#"from events | where level = "error" | limit 10"#).unwrap();
        assert_eq!(pipeline.statements.len(), 3);
    }

    #[test]
    fn timeseries_query() {
        let sql = compile_to_duckdb("from events | timeseries 5m").unwrap();
        assert!(sql.contains("time_bucket((5 * INTERVAL '1 minute')"));
        assert!(sql.contains("COUNT(*)"));
    }

    #[test]
    fn known_fields_returns_values() {
        let fields = known_fields();
        assert!(fields.contains(&"service".to_string()));
        assert!(fields.contains(&"level".to_string()));
        assert!(fields.contains(&"event_id".to_string()));
    }

    #[test]
    fn analysis_diagnostics_carry_real_source_spans() {
        let input = "from events | project missing_field";
        let error = analyze_source(input, &AnalysisOptions::duckdb()).unwrap_err();
        let span = error.diagnostics[0]
            .primary_span
            .expect("semantic diagnostic must have a real span");
        assert_eq!(&input[span.start..span.end], "missing_field");
    }

    #[test]
    fn parameter_diagnostics_carry_real_source_spans() {
        let input = "from events | where event_id = $id";
        let error = render_query_with_parameters(input, Target::DuckDB, &serde_json::json!({}))
            .unwrap_err();
        let span = error.diagnostics[0]
            .primary_span
            .expect("parameter diagnostic must have a real span");
        assert_eq!(&input[span.start..span.end], "$id");
    }

    #[test]
    fn custom_render_entry_points_relocate_semantic_diagnostics() {
        let input = "from events | project missing_field";
        let errors = [
            render_query_with_options(
                input,
                Target::DuckDB,
                &serde_json::Value::Null,
                &serde_json::Value::Null,
                Schema::loza_v1(Target::DuckDB),
                QueryPolicy::default(),
            )
            .unwrap_err(),
            render_query_with_parameters(input, Target::DuckDB, &serde_json::Value::Null)
                .unwrap_err(),
            render_query_with_context(
                input,
                Target::DuckDB,
                &serde_json::Value::Null,
                &serde_json::Value::Null,
            )
            .unwrap_err(),
        ];
        for error in errors {
            let span = error.diagnostics[0]
                .primary_span
                .expect("semantic diagnostic must have a source span");
            assert_eq!(&input[span.start..span.end], "missing_field");
        }
    }
}
