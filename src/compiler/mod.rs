pub mod clickhouse;
pub mod duckdb;

use crate::ast::{AggFunction, BinOp, Expr, Literal, UnaryOp};
use crate::error::{Diagnostic, DiagnosticBundle, Span};
use crate::functions::{is_builtin_function, validate_function_arity};
use crate::ir::{RelationNode, TypedAggregation, TypedColumn, TypedExpr, TypedPipeline, ValueType};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Target database dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    DuckDB,
    ClickHouse,
    PostgreSQL,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundValue {
    pub logical_type: ValueType,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledQuery {
    pub target: Target,
    pub sql: String,
    pub parameters: Vec<BoundValue>,
    pub output_schema: Vec<TypedColumn>,
    pub required_capabilities: Vec<String>,
    pub language_version: String,
}

/// Render a typed pipeline. All source literals become ordered bind parameters.
pub fn render(pipeline: &TypedPipeline, target: Target) -> Result<CompiledQuery, DiagnosticBundle> {
    let mut ctx = RenderContext {
        target,
        bindings: Vec::new(),
    };
    let marked_sql = render_relation(&pipeline.root, &mut ctx)?;
    let (sql, parameters) = finalize_parameters(&marked_sql, &ctx);
    Ok(CompiledQuery {
        target,
        sql,
        parameters,
        output_schema: pipeline.output_schema.clone(),
        required_capabilities: Vec::new(),
        language_version: pipeline.language_version.clone(),
    })
}

struct RenderContext {
    target: Target,
    bindings: Vec<BoundValue>,
}

fn render_relation(
    node: &RelationNode,
    ctx: &mut RenderContext,
) -> Result<String, DiagnosticBundle> {
    match node {
        RelationNode::Scan { source, .. } => Ok(format!("SELECT * FROM {}", quote_ident(source))),
        RelationNode::Filter {
            input, predicate, ..
        } => {
            let inner = render_relation(input, ctx)?;
            Ok(format!(
                "SELECT * FROM ({}) AS q WHERE {}",
                inner,
                render_expr(predicate, ctx, "q")?
            ))
        }
        RelationNode::Extend {
            input, name, expr, ..
        } => {
            let inner = render_relation(input, ctx)?;
            Ok(format!(
                "SELECT q.*, {} AS {} FROM ({}) AS q",
                render_expr(expr, ctx, "q")?,
                quote_ident(name),
                inner
            ))
        }
        RelationNode::Project {
            input, expressions, ..
        } => {
            let inner = render_relation(input, ctx)?;
            let values = expressions
                .iter()
                .map(|expr| render_expr(expr, ctx, "q"))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "SELECT {} FROM ({}) AS q",
                values.join(", "),
                inner
            ))
        }
        RelationNode::Distinct {
            input, expressions, ..
        } => {
            let inner = render_relation(input, ctx)?;
            let values = expressions
                .iter()
                .map(|expr| render_expr(expr, ctx, "q"))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "SELECT DISTINCT {} FROM ({}) AS q",
                values.join(", "),
                inner
            ))
        }
        RelationNode::Aggregate {
            input,
            aggregations,
            groups,
            ..
        } => {
            let inner = render_relation(input, ctx)?;
            let mut values = groups
                .iter()
                .map(|expr| render_expr(expr, ctx, "q"))
                .collect::<Result<Vec<_>, _>>()?;
            values.extend(
                aggregations
                    .iter()
                    .map(|agg| render_aggregate(agg, ctx, "q"))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            let mut sql = format!("SELECT {} FROM ({}) AS q", values.join(", "), inner);
            if !groups.is_empty() {
                let group_sql = groups
                    .iter()
                    .map(|expr| render_expr(expr, ctx, "q"))
                    .collect::<Result<Vec<_>, _>>()?;
                sql.push_str(&format!(" GROUP BY {}", group_sql.join(", ")));
            }
            Ok(sql)
        }
        RelationNode::Sort { input, keys, .. } => {
            let inner = render_relation(input, ctx)?;
            let values = keys
                .iter()
                .map(|(expr, order)| Ok(format!("{} {}", render_expr(expr, ctx, "q")?, order)))
                .collect::<Result<Vec<_>, DiagnosticBundle>>()?;
            Ok(format!(
                "SELECT * FROM ({}) AS q ORDER BY {}",
                inner,
                values.join(", ")
            ))
        }
        RelationNode::Limit { input, count, .. } => {
            let inner = render_relation(input, ctx)?;
            let placeholder = bind(ctx, ValueType::Int, json!(*count));
            Ok(format!(
                "SELECT * FROM ({}) AS q LIMIT {}",
                inner, placeholder
            ))
        }
        RelationNode::Offset { input, count, .. } => {
            let inner = render_relation(input, ctx)?;
            let placeholder = bind(ctx, ValueType::Int, json!(*count));
            Ok(format!(
                "SELECT * FROM ({}) AS q OFFSET {}",
                inner, placeholder
            ))
        }
        RelationNode::TimeSeries {
            input, interval, ..
        } => {
            let inner = render_relation(input, ctx)?;
            let duration = render_duration(interval, ctx);
            let bucket = match ctx.target {
                Target::DuckDB => {
                    format!("time_bucket({}, q.\"timestamp\")", duration)
                }
                Target::ClickHouse => {
                    format!("toStartOfInterval(q.\"timestamp\", {})", duration)
                }
                Target::PostgreSQL => {
                    format!("date_bin({}, q.\"timestamp\", TIMESTAMP 'epoch')", duration)
                }
            };
            Ok(format!(
                "SELECT {} AS \"timestamp\", COUNT(*) AS \"count\" FROM ({}) AS q GROUP BY {}",
                bucket, inner, bucket
            ))
        }
    }
}

fn render_aggregate(
    agg: &TypedAggregation,
    ctx: &mut RenderContext,
    alias: &str,
) -> Result<String, DiagnosticBundle> {
    let arg = agg
        .arg
        .as_ref()
        .map(|e| render_expr(e, ctx, alias))
        .transpose()?;
    let function = match (&agg.function, arg) {
        (AggFunction::Count, None) => "COUNT(*)".to_string(),
        (AggFunction::Count, Some(value)) => format!("COUNT({})", value),
        (AggFunction::DCount, Some(value)) => format!("COUNT(DISTINCT {})", value),
        (AggFunction::Sum, Some(value)) => format!("SUM({})", value),
        (AggFunction::Avg, Some(value)) => format!("AVG({})", value),
        (AggFunction::Min, Some(value)) => format!("MIN({})", value),
        (AggFunction::Max, Some(value)) => format!("MAX({})", value),
        (AggFunction::First, Some(value)) => format!("FIRST_VALUE({})", value),
        (AggFunction::Last, Some(value)) => format!("LAST_VALUE({})", value),
        (func, Some(value)) => {
            let pct = match func {
                AggFunction::P50 => 0.5,
                AggFunction::P95 => 0.95,
                AggFunction::P99 => 0.99,
                AggFunction::Percentile(p) => *p / 100.0,
                _ => 0.5,
            };
            let placeholder = bind(ctx, ValueType::Float, json!(pct));
            match ctx.target {
                Target::DuckDB => format!("quantile_cont({}, {})", placeholder, value),
                Target::ClickHouse => format!("quantile({})({})", placeholder, value),
                Target::PostgreSQL => format!(
                    "percentile_cont({}) WITHIN GROUP (ORDER BY {})",
                    placeholder, value
                ),
            }
        }
        (_, None) => {
            return Err(DiagnosticBundle::one(Diagnostic::error(
                "LQL201",
                "aggregate requires an argument",
                Some(Span::empty(0)),
            )))
        }
    };
    Ok(format!("{} AS {}", function, quote_ident(&agg.alias)))
}

fn render_expr(
    expr: &TypedExpr,
    ctx: &mut RenderContext,
    alias: &str,
) -> Result<String, DiagnosticBundle> {
    match &expr.expr {
        Expr::Column(name) => Ok(if name == "*" {
            "*".to_string()
        } else {
            format!("{}.{}", alias, quote_ident(name))
        }),
        Expr::Parameter(_) => Ok(bind(ctx, ValueType::Dynamic, Value::Null)),
        Expr::Literal(lit) => Ok(render_literal(lit, &expr.field_type, ctx)),
        Expr::Wildcard => Ok("*".to_string()),
        Expr::UnaryOp { op, expr: inner } => {
            let value = render_expr(&typed_child(inner), ctx, alias)?;
            Ok(match op {
                UnaryOp::Not => format!("NOT ({})", value),
                UnaryOp::Neg => format!("-({})", value),
            })
        }
        Expr::BinaryOp { left, op, right } => {
            if matches!(op, BinOp::Eq | BinOp::Neq) && (is_null_expr(left) || is_null_expr(right)) {
                let value = if is_null_expr(left) { right } else { left };
                let rendered = render_expr(&typed_child(value), ctx, alias)?;
                return Ok(format!(
                    "({} IS {}NULL)",
                    rendered,
                    if matches!(op, BinOp::Neq) { "NOT " } else { "" }
                ));
            }
            let left_value = render_expr(&typed_child(left), ctx, alias)?;
            let right_value = render_expr(&typed_child(right), ctx, alias)?;
            let rendered = match op {
                BinOp::Contains | BinOp::Has => match ctx.target {
                    Target::DuckDB => format!("contains({}, {})", left_value, right_value),
                    Target::ClickHouse => format!("position({}, {}) > 0", left_value, right_value),
                    Target::PostgreSQL => {
                        format!("POSITION({} IN {}) > 0", right_value, left_value)
                    }
                },
                BinOp::StartsWith => match ctx.target {
                    Target::DuckDB => format!("starts_with({}, {})", left_value, right_value),
                    Target::ClickHouse => format!("startsWith({}, {})", left_value, right_value),
                    Target::PostgreSQL => format!("{} LIKE ({} || '%')", left_value, right_value),
                },
                BinOp::EndsWith => match ctx.target {
                    Target::DuckDB => format!("ends_with({}, {})", left_value, right_value),
                    Target::ClickHouse => format!("endsWith({}, {})", left_value, right_value),
                    Target::PostgreSQL => format!("{} LIKE ('%' || {})", left_value, right_value),
                },
                BinOp::Matches | BinOp::NotMatches => match ctx.target {
                    Target::DuckDB => format!(
                        "{}regexp_matches({}, {})",
                        if matches!(op, BinOp::NotMatches) {
                            "NOT "
                        } else {
                            ""
                        },
                        left_value,
                        right_value
                    ),
                    Target::ClickHouse => format!(
                        "{}match({}, {})",
                        if matches!(op, BinOp::NotMatches) {
                            "NOT "
                        } else {
                            ""
                        },
                        left_value,
                        right_value
                    ),
                    Target::PostgreSQL => format!(
                        "{}({} {} {})",
                        if matches!(op, BinOp::NotMatches) {
                            "NOT "
                        } else {
                            ""
                        },
                        left_value,
                        if matches!(op, BinOp::NotMatches) {
                            "!~"
                        } else {
                            "~"
                        },
                        right_value
                    ),
                },
                _ => format!("{} {} {}", left_value, op, right_value),
            };
            Ok(format!("({})", rendered))
        }
        Expr::InList {
            expr: value,
            values,
            negated,
        } => {
            let left = render_expr(&typed_child(value), ctx, alias)?;
            let values = values
                .iter()
                .map(|v| render_expr(&typed_child(v), ctx, alias))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "{} {}IN ({})",
                left,
                if *negated { "NOT " } else { "" },
                values.join(", ")
            ))
        }
        Expr::Between {
            expr: value,
            low,
            high,
            negated,
        } => {
            let value = render_expr(&typed_child(value), ctx, alias)?;
            let low = render_expr(&typed_child(low), ctx, alias)?;
            let high = render_expr(&typed_child(high), ctx, alias)?;
            Ok(format!(
                "{}({} >= {} AND {} <= {})",
                if *negated { "NOT " } else { "" },
                value,
                low,
                value,
                high
            ))
        }
        Expr::Function { name, args } => {
            let lower = name.to_ascii_lowercase();
            if !is_builtin_function(&lower) {
                return Err(DiagnosticBundle::one(Diagnostic::error(
                    "LQL101",
                    format!("unknown function '{}'", name),
                    Some(expr.span),
                )));
            }
            if let Err(error) = validate_function_arity(&lower, args) {
                return Err(DiagnosticBundle::one(Diagnostic::error(
                    "LQL104",
                    error.to_string(),
                    Some(expr.span),
                )));
            }
            let values = args
                .iter()
                .map(|a| render_expr(&typed_child(a), ctx, alias))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match lower.as_str() {
                "if" => format!(
                    "CASE WHEN {} THEN {} ELSE {} END",
                    values[0], values[1], values[2]
                ),
                "ago" => format!("NOW() - {}", values[0]),
                "tolower" => format!("LOWER({})", values[0]),
                "toupper" => format!("UPPER({})", values[0]),
                "strlen" => format!("LENGTH({})", values[0]),
                "trim" => format!("TRIM({})", values[0]),
                "substring" => match ctx.target {
                    Target::DuckDB => {
                        format!("SUBSTRING({}, {}, {})", values[0], values[1], values[2])
                    }
                    Target::ClickHouse => {
                        format!("substring({}, {}, {})", values[0], values[1], values[2])
                    }
                    Target::PostgreSQL => {
                        format!("SUBSTRING({}, {}, {})", values[0], values[1], values[2])
                    }
                },
                "typeof" => match ctx.target {
                    Target::DuckDB => format!("TYPEOF({})", values[0]),
                    Target::ClickHouse => format!("toTypeName({})", values[0]),
                    Target::PostgreSQL => format!("pg_typeof({})", values[0]),
                },
                "replace" => match ctx.target {
                    Target::DuckDB => {
                        format!("REPLACE({}, {}, {})", values[0], values[1], values[2])
                    }
                    Target::ClickHouse => {
                        format!("replaceAll({}, {}, {})", values[0], values[1], values[2])
                    }
                    Target::PostgreSQL => {
                        format!("REPLACE({}, {}, {})", values[0], values[1], values[2])
                    }
                },
                "to_string" => match ctx.target {
                    Target::DuckDB => format!("CAST({} AS VARCHAR)", values[0]),
                    Target::ClickHouse => format!("toString({})", values[0]),
                    Target::PostgreSQL => format!("CAST({} AS TEXT)", values[0]),
                },
                "to_int" => match ctx.target {
                    Target::DuckDB => format!("CAST({} AS BIGINT)", values[0]),
                    Target::ClickHouse => format!("toInt64({})", values[0]),
                    Target::PostgreSQL => format!("CAST({} AS BIGINT)", values[0]),
                },
                "to_float" => match ctx.target {
                    Target::DuckDB => format!("CAST({} AS DOUBLE)", values[0]),
                    Target::ClickHouse => format!("toFloat64({})", values[0]),
                    Target::PostgreSQL => format!("CAST({} AS DOUBLE PRECISION)", values[0]),
                },
                "abs" => format!("ABS({})", values[0]),
                "round" => format!("ROUND({})", values.join(", ")),
                "floor" => format!("FLOOR({})", values[0]),
                "ceil" => format!("CEIL({})", values[0]),
                "sqrt" => format!("SQRT({})", values[0]),
                "log" => match ctx.target {
                    Target::DuckDB => format!("LN({})", values[0]),
                    Target::ClickHouse => format!("log({})", values[0]),
                    Target::PostgreSQL => format!("LN({})", values[0]),
                },
                "coalesce" => format!("COALESCE({})", values.join(", ")),
                "now" => "NOW()".to_string(),
                "isempty" => format!("({} IS NULL OR {} = '')", values[0], values[0]),
                "isnotempty" => format!("({} IS NOT NULL AND {} != '')", values[0], values[0]),
                "array_length" => match ctx.target {
                    Target::DuckDB => format!("ARRAY_LENGTH({})", values[0]),
                    Target::ClickHouse => format!("length({})", values[0]),
                    Target::PostgreSQL => format!("CARDINALITY({})", values[0]),
                },
                "split" => match ctx.target {
                    Target::DuckDB => format!("STRING_SPLIT({}, {})", values[0], values[1]),
                    Target::ClickHouse => format!("splitByString({}, {})", values[1], values[0]),
                    Target::PostgreSQL => format!("string_to_array({}, {})", values[0], values[1]),
                },
                "bin" => match ctx.target {
                    Target::DuckDB => format!("time_bucket({}, {})", values[1], values[0]),
                    Target::ClickHouse => {
                        format!("toStartOfInterval({}, {})", values[0], values[1])
                    }
                    Target::PostgreSQL => {
                        format!("date_bin({}, {}, TIMESTAMP 'epoch')", values[1], values[0])
                    }
                },
                _ => unreachable!("validated function catalog must cover renderer"),
            })
        }
    }
}

fn typed_child(expr: &Expr) -> TypedExpr {
    TypedExpr {
        expr: expr.clone(),
        field_type: match expr {
            Expr::Literal(Literal::String(_)) => ValueType::String,
            Expr::Literal(Literal::Timestamp(_)) => ValueType::Timestamp,
            Expr::Literal(Literal::Dynamic(_)) => ValueType::Dynamic,
            Expr::Literal(Literal::Integer(_)) => ValueType::Int,
            Expr::Literal(Literal::Float(_)) => ValueType::Float,
            Expr::Literal(Literal::Bool(_)) => ValueType::Bool,
            Expr::Literal(Literal::Duration(_)) => ValueType::Duration,
            Expr::Literal(Literal::Null) => ValueType::Unknown,
            _ => ValueType::Unknown,
        },
        nullable: true,
        symbol: None,
        span: Span::empty(0),
    }
}
fn render_literal(lit: &Literal, ty: &ValueType, ctx: &mut RenderContext) -> String {
    if let Literal::Duration(duration) = lit {
        return render_duration(duration, ctx);
    }
    let (value, inferred) = match lit {
        Literal::String(s) => (json!(s), ValueType::String),
        Literal::Timestamp(s) => (json!(s), ValueType::Timestamp),
        Literal::Dynamic(value) => (value.clone(), ValueType::Dynamic),
        Literal::Integer(n) => (json!(n), ValueType::Int),
        Literal::Float(n) => (json!(n), ValueType::Float),
        Literal::Bool(b) => (json!(b), ValueType::Bool),
        Literal::Null => (Value::Null, ValueType::Unknown),
        Literal::Duration(_) => unreachable!("duration handled above"),
    };
    bind(
        ctx,
        if *ty == ValueType::Unknown {
            inferred
        } else {
            ty.clone()
        },
        value,
    )
}

fn render_duration(duration: &crate::ast::Duration, ctx: &mut RenderContext) -> String {
    let placeholder = bind(ctx, ValueType::Duration, json!(duration.value));
    match ctx.target {
        Target::DuckDB => {
            let unit = match duration.unit {
                crate::ast::DurationUnit::Milliseconds => "millisecond",
                crate::ast::DurationUnit::Seconds => "second",
                crate::ast::DurationUnit::Minutes => "minute",
                crate::ast::DurationUnit::Hours => "hour",
                crate::ast::DurationUnit::Days => "day",
                crate::ast::DurationUnit::Weeks => "week",
            };
            format!("({placeholder} * INTERVAL '1 {unit}')")
        }
        Target::ClickHouse => {
            let function = match duration.unit {
                crate::ast::DurationUnit::Milliseconds => "toIntervalMillisecond",
                crate::ast::DurationUnit::Seconds => "toIntervalSecond",
                crate::ast::DurationUnit::Minutes => "toIntervalMinute",
                crate::ast::DurationUnit::Hours => "toIntervalHour",
                crate::ast::DurationUnit::Days => "toIntervalDay",
                crate::ast::DurationUnit::Weeks => "toIntervalWeek",
            };
            format!("{function}({placeholder})")
        }
        Target::PostgreSQL => {
            let unit = match duration.unit {
                crate::ast::DurationUnit::Milliseconds => "millisecond",
                crate::ast::DurationUnit::Seconds => "second",
                crate::ast::DurationUnit::Minutes => "minute",
                crate::ast::DurationUnit::Hours => "hour",
                crate::ast::DurationUnit::Days => "day",
                crate::ast::DurationUnit::Weeks => "week",
            };
            format!("({placeholder} * INTERVAL '1 {unit}')")
        }
    }
}

fn bind(ctx: &mut RenderContext, ty: ValueType, value: Value) -> String {
    let index = ctx.bindings.len();
    ctx.bindings.push(BoundValue {
        logical_type: ty,
        value,
    });
    format!("\u{1e}{index}\u{1f}")
}

fn finalize_parameters(sql: &str, ctx: &RenderContext) -> (String, Vec<BoundValue>) {
    let mut rendered = String::with_capacity(sql.len());
    let mut parameters = Vec::new();
    let mut rest = sql;
    while let Some(start) = rest.find('\u{1e}') {
        rendered.push_str(&rest[..start]);
        let marker = &rest[start + '\u{1e}'.len_utf8()..];
        let end = marker
            .find('\u{1f}')
            .expect("bind markers are created as complete internal pairs");
        let binding_index = marker[..end]
            .parse::<usize>()
            .expect("bind markers contain numeric indexes");
        let parameter = ctx.bindings[binding_index].clone();
        let output_index = parameters.len();
        match ctx.target {
            Target::DuckDB => rendered.push('?'),
            Target::ClickHouse => rendered.push_str(&format!(
                "{{p{}:{}}}",
                output_index,
                clickhouse_type(&parameter.logical_type)
            )),
            Target::PostgreSQL => rendered.push_str(&format!("${}", output_index + 1)),
        }
        parameters.push(parameter);
        rest = &marker[end + '\u{1f}'.len_utf8()..];
    }
    rendered.push_str(rest);
    (rendered, parameters)
}
fn clickhouse_type(ty: &ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "Bool",
        ValueType::Int | ValueType::Duration => "Int64",
        ValueType::Float | ValueType::Decimal => "Float64",
        ValueType::Timestamp => "DateTime64(3)",
        _ => "String",
    }
}
fn quote_ident(name: &str) -> String {
    if name == "*" {
        "*".to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// Compatibility wrapper. New callers should use parse -> analyze -> render.
pub fn compile(
    pipeline: &crate::ast::Pipeline,
    target: Target,
    schema: &crate::schema::Schema,
) -> Result<String, crate::error::LqlError> {
    let options = crate::ir::AnalysisOptions {
        schema: schema.clone(),
        target,
        policy: crate::ir::QueryPolicy::default(),
        language_version: "0.1".to_string(),
        clock: None,
    };
    let typed =
        crate::ir::analyze(pipeline, &options).map_err(|d| crate::error::LqlError::Compile {
            message: d.to_string(),
            span: d.diagnostics.first().and_then(|d| d.primary_span),
        })?;
    let plan = render(&typed, target).map_err(|d| crate::error::LqlError::Compile {
        message: d.to_string(),
        span: d.diagnostics.first().and_then(|d| d.primary_span),
    })?;
    Ok(inline_parameters(&plan))
}

fn inline_parameters(plan: &CompiledQuery) -> String {
    let mut sql = plan.sql.clone();
    for (index, parameter) in plan.parameters.iter().enumerate() {
        let value = match &parameter.value {
            Value::String(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Null => "NULL".to_string(),
            other => other.to_string(),
        };
        sql = match plan.target {
            Target::DuckDB => sql.replacen('?', &value, 1),
            Target::ClickHouse => {
                let marker = format!(
                    "{{p{}:{}}}",
                    index,
                    clickhouse_type(&parameter.logical_type)
                );
                sql.replace(&marker, &value)
            }
            Target::PostgreSQL => sql.replacen(&format!("${}", index + 1), &value, 1),
        };
    }
    sql
}

fn is_null_expr(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(Literal::Null))
}
