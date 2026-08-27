//! Loza Query Language (LQL)
//!
//! Public compilation follows one explicit sequence:
//! `parse` -> `analyze` -> `compiler::render`.

pub mod ast;
pub mod compiler;
pub mod error;
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

/// Parse LQL input into an AST pipeline.
pub fn parse(input: &str) -> Result<Pipeline, LqlError> {
    let tokens = lexer::Lexer::new(input).tokenize()?;
    parser::Parser::new(tokens).parse()
}

/// Parse and return structured diagnostics with source-relative byte spans.
pub fn parse_diagnostics(input: &str) -> Result<Pipeline, DiagnosticBundle> {
    parse(input).map_err(|error| structured_error(input, error))
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
        Schema::loza_v1(target),
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
    bind_pipeline(&mut pipeline, parameters)?;
    append_scope_filter(&mut pipeline, scope)?;
    let options = AnalysisOptions {
        schema,
        target,
        policy,
        language_version: "0.1".to_string(),
        clock: None,
    };
    let typed = analyze_pipeline(&pipeline, &options)?;
    compiler::render(&typed, target)
}

fn append_scope_filter(
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
        pipeline.statements.push(Statement::Where(predicate));
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

fn relocate_diagnostics(input: &str, mut bundle: DiagnosticBundle) -> DiagnosticBundle {
    for diagnostic in &mut bundle.diagnostics {
        if diagnostic.primary_span == Some(Span::empty(0)) {
            if let Some(start_quote) = diagnostic.message.find('\'') {
                let rest = &diagnostic.message[start_quote + 1..];
                if let Some(end_quote) = rest.find('\'') {
                    let needle = &rest[..end_quote];
                    if let Some(start) = input.find(needle) {
                        diagnostic.primary_span = Some(Span::new(start, start + needle.len()));
                    }
                }
            }
        }
    }
    bundle
}

/// Compile a query using the compatibility SQL-only wrapper.
pub fn compile_to_duckdb(input: &str) -> Result<String, LqlError> {
    let pipeline = parse(input)?;
    compiler::compile(&pipeline, Target::DuckDB, &Schema::duckdb_default())
}

/// Compile a query using the compatibility SQL-only wrapper.
pub fn compile_to_clickhouse(input: &str) -> Result<String, LqlError> {
    let pipeline = parse(input)?;
    compiler::compile(&pipeline, Target::ClickHouse, &Schema::clickhouse_default())
}
/// Compile a query using the compatibility SQL-only wrapper.
pub fn compile_to_postgres(input: &str) -> Result<String, LqlError> {
    let pipeline = parse(input)?;
    compiler::compile(
        &pipeline,
        Target::PostgreSQL,
        &Schema::loza_v1(Target::PostgreSQL),
    )
}

/// Validate a LQL query against the default generated event schema.
pub fn validate_query(input: &str) -> Result<(), LqlError> {
    let pipeline = parse(input)?;
    let options = AnalysisOptions::duckdb();
    ir::analyze(&pipeline, &options)
        .map(|_| ())
        .map_err(|d| LqlError::Compile {
            message: d.to_string(),
            span: d.diagnostics.first().and_then(|d| d.primary_span),
        })
}

/// Compatibility compile entry point.
pub fn compile(input: &str, target: Target) -> Result<String, LqlError> {
    match target {
        Target::DuckDB => compile_to_duckdb(input),
        Target::ClickHouse => compile_to_clickhouse(input),
        Target::PostgreSQL => compile_to_postgres(input),
    }
}

/// Get all known field names for autocomplete.
pub fn known_fields() -> Vec<String> {
    let schema = Schema::duckdb_default();
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
    use super::{ir, AnalysisOptions};
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
        super::parse_diagnostics(input)
            .and_then(|p| ir::analyze(&p, &AnalysisOptions::duckdb()).map(|_| ()))
            .map(|_| "{}".to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))
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
}
