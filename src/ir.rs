use crate::ast::*;
use crate::compiler::Target;
use crate::error::{Diagnostic, DiagnosticBundle, Span};
use crate::schema::{FieldType, Schema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Bool,
    Int,
    Float,
    Decimal,
    String,
    Timestamp,
    Duration,
    Json,
    Array,
    Object,
    Dynamic,
    Unknown,
}

impl ValueType {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Float | Self::Decimal)
    }
    fn from_field(t: &FieldType) -> Self {
        match t {
            FieldType::String => Self::String,
            FieldType::Integer => Self::Int,
            FieldType::Float => Self::Float,
            FieldType::Boolean => Self::Bool,
            FieldType::Timestamp => Self::Timestamp,
            FieldType::Duration => Self::Duration,
            FieldType::Array => Self::Array,
            FieldType::Object => Self::Object,
            FieldType::Any => Self::Dynamic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedColumn {
    pub name: String,
    pub field_type: ValueType,
    pub nullable: bool,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedExpr {
    pub expr: Expr,
    pub field_type: ValueType,
    pub nullable: bool,
    pub symbol: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RelationNode {
    Scan {
        source: String,
        columns: Vec<TypedColumn>,
    },
    Filter {
        input: Box<RelationNode>,
        predicate: TypedExpr,
        columns: Vec<TypedColumn>,
    },
    Extend {
        input: Box<RelationNode>,
        name: String,
        expr: TypedExpr,
        columns: Vec<TypedColumn>,
    },
    Project {
        input: Box<RelationNode>,
        expressions: Vec<TypedExpr>,
        columns: Vec<TypedColumn>,
    },
    Aggregate {
        input: Box<RelationNode>,
        aggregations: Vec<TypedAggregation>,
        groups: Vec<TypedExpr>,
        columns: Vec<TypedColumn>,
    },
    Distinct {
        input: Box<RelationNode>,
        expressions: Vec<TypedExpr>,
        columns: Vec<TypedColumn>,
    },
    Sort {
        input: Box<RelationNode>,
        keys: Vec<(TypedExpr, Order)>,
        columns: Vec<TypedColumn>,
    },
    Limit {
        input: Box<RelationNode>,
        count: usize,
        columns: Vec<TypedColumn>,
    },
    Offset {
        input: Box<RelationNode>,
        count: usize,
        columns: Vec<TypedColumn>,
    },
    TimeSeries {
        input: Box<RelationNode>,
        interval: Duration,
        columns: Vec<TypedColumn>,
    },
}

impl RelationNode {
    pub fn columns(&self) -> &[TypedColumn] {
        match self {
            Self::Scan { columns, .. }
            | Self::Filter { columns, .. }
            | Self::Extend { columns, .. }
            | Self::Project { columns, .. }
            | Self::Aggregate { columns, .. }
            | Self::Distinct { columns, .. }
            | Self::Sort { columns, .. }
            | Self::Limit { columns, .. }
            | Self::Offset { columns, .. }
            | Self::TimeSeries { columns, .. } => columns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedAggregation {
    pub function: AggFunction,
    pub arg: Option<TypedExpr>,
    pub alias: String,
    pub field_type: ValueType,
    pub nullable: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypedPipeline {
    pub root: RelationNode,
    pub output_schema: Vec<TypedColumn>,
    pub language_version: String,
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryPolicy {
    #[serde(default = "default_sources")]
    pub allowed_sources: Vec<String>,
    #[serde(default)]
    pub allowed_fields: Vec<String>,
    #[serde(default)]
    pub sensitive_fields: Vec<String>,
    #[serde(default)]
    pub max_result_rows: Option<usize>,
    #[serde(default)]
    pub max_complexity: Option<usize>,
    #[serde(default)]
    pub max_time_ms: Option<u64>,
    #[serde(default)]
    pub allowed_capabilities: Vec<String>,
    #[serde(default)]
    pub max_parameters: Option<usize>,
}
fn default_sources() -> Vec<String> {
    vec!["events".to_string()]
}
impl Default for QueryPolicy {
    fn default() -> Self {
        Self {
            allowed_sources: default_sources(),
            allowed_fields: Vec::new(),
            sensitive_fields: Vec::new(),
            max_result_rows: None,
            max_complexity: None,
            max_time_ms: None,
            allowed_capabilities: Vec::new(),
            max_parameters: None,
        }
    }
}

impl QueryPolicy {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let policy: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid policy: {error}"))?;
        if policy
            .allowed_sources
            .iter()
            .any(|source| source.trim().is_empty())
        {
            return Err("invalid policy: allowed_sources cannot contain empty values".to_string());
        }
        for (name, value) in [
            ("max_result_rows", policy.max_result_rows.map(|v| v as u64)),
            ("max_complexity", policy.max_complexity.map(|v| v as u64)),
            ("max_time_ms", policy.max_time_ms),
            ("max_parameters", policy.max_parameters.map(|v| v as u64)),
        ] {
            if value == Some(0) {
                return Err(format!("invalid policy: {name} must be greater than zero"));
            }
        }
        Ok(policy)
    }
}

#[derive(Debug, Clone)]
pub struct AnalysisOptions {
    pub schema: Schema,
    pub target: Target,
    pub policy: QueryPolicy,
    pub language_version: String,
    /// Optional unix milliseconds used by time-sensitive functions.
    pub clock: Option<i64>,
}

impl AnalysisOptions {
    pub fn duckdb() -> Self {
        Self {
            schema: Schema::loza_v1(Target::DuckDB),
            target: Target::DuckDB,
            policy: QueryPolicy::default(),
            language_version: "0.1".to_string(),
            clock: None,
        }
    }
    pub fn clickhouse() -> Self {
        Self {
            schema: Schema::loza_v1(Target::ClickHouse),
            target: Target::ClickHouse,
            policy: QueryPolicy::default(),
            language_version: "0.1".to_string(),
            clock: None,
        }
    }
    pub fn postgres() -> Self {
        Self {
            schema: Schema::loza_v1(Target::PostgreSQL),
            target: Target::PostgreSQL,
            policy: QueryPolicy::default(),
            language_version: "0.1".to_string(),
            clock: None,
        }
    }
}

pub fn analyze(
    pipeline: &Pipeline,
    options: &AnalysisOptions,
) -> Result<TypedPipeline, DiagnosticBundle> {
    let Some(Statement::From(source)) = pipeline.statements.first() else {
        return Err(DiagnosticBundle::one(Diagnostic::error(
            "LQL100",
            "query must start with a source",
            Some(Span::empty(0)),
        )));
    };
    let source_name = match source {
        Source::Events => "events",
        Source::Traces => "traces",
        Source::Incidents => "incidents",
        Source::Table(name) => name.as_str(),
    };
    if !options
        .policy
        .allowed_sources
        .iter()
        .any(|allowed| allowed == source_name)
        || options.schema.source != source_name
    {
        return Err(DiagnosticBundle::one(Diagnostic::error(
            "LQL110",
            format!("source '{source_name}' is not declared or allowed"),
            Some(Span::empty(0)),
        )));
    }
    let mut columns = schema_columns(&options.schema);
    validate_policy_fields(pipeline, &options.policy)?;
    let mut root = RelationNode::Scan {
        source: source_name.to_string(),
        columns: columns.clone(),
    };
    for statement in pipeline.statements.iter().skip(1) {
        root = match statement {
            Statement::Where(expr) => {
                let typed = type_expr(expr, &columns)?;
                if typed.field_type != ValueType::Bool {
                    return Err(type_error("where predicate must be bool"));
                }
                RelationNode::Filter {
                    input: Box::new(root),
                    predicate: typed,
                    columns: columns.clone(),
                }
            }
            Statement::Extend { name, expr } => {
                let typed = type_expr(expr, &columns)?;
                columns.retain(|c| c.name != *name);
                columns.push(TypedColumn {
                    name: name.clone(),
                    field_type: typed.field_type.clone(),
                    nullable: typed.nullable,
                    origin: "extend".to_string(),
                });
                RelationNode::Extend {
                    input: Box::new(root),
                    name: name.clone(),
                    expr: typed,
                    columns: columns.clone(),
                }
            }
            Statement::Project(exprs) => {
                let typed = exprs
                    .iter()
                    .map(|e| type_expr(e, &columns))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut projected = Vec::new();
                for expr in &typed {
                    let name = expr
                        .symbol
                        .clone()
                        .unwrap_or_else(|| expression_name(&expr.expr));
                    projected.push(TypedColumn {
                        name,
                        field_type: expr.field_type.clone(),
                        nullable: expr.nullable,
                        origin: "project".to_string(),
                    });
                }
                columns = projected.clone();
                RelationNode::Project {
                    input: Box::new(root),
                    expressions: typed,
                    columns: projected,
                }
            }
            Statement::Distinct(exprs) => {
                let typed = exprs
                    .iter()
                    .map(|e| type_expr(e, &columns))
                    .collect::<Result<Vec<_>, _>>()?;
                let output = typed
                    .iter()
                    .map(|e| TypedColumn {
                        name: e.symbol.clone().unwrap_or_else(|| expression_name(&e.expr)),
                        field_type: e.field_type.clone(),
                        nullable: e.nullable,
                        origin: "distinct".to_string(),
                    })
                    .collect::<Vec<_>>();
                columns = output.clone();
                RelationNode::Distinct {
                    input: Box::new(root),
                    expressions: typed,
                    columns: output,
                }
            }
            Statement::Summarize { aggregations, by } => {
                let groups = by
                    .iter()
                    .map(|e| type_expr(e, &columns))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut output = groups
                    .iter()
                    .map(|e| TypedColumn {
                        name: e.symbol.clone().unwrap_or_else(|| expression_name(&e.expr)),
                        field_type: e.field_type.clone(),
                        nullable: e.nullable,
                        origin: "group".to_string(),
                    })
                    .collect::<Vec<_>>();
                let mut typed_aggs = Vec::new();
                let mut seen = HashMap::new();
                for agg in aggregations {
                    let arg = agg
                        .arg
                        .as_ref()
                        .map(|e| type_expr(e, &columns))
                        .transpose()?;
                    let (ty, nullable) = aggregate_type(&agg.function, arg.as_ref())?;
                    let alias = agg
                        .alias
                        .clone()
                        .unwrap_or_else(|| aggregate_name(&agg.function, arg.as_ref()));
                    if seen.insert(alias.clone(), ()).is_some() {
                        return Err(type_error("duplicate aggregate output name"));
                    }
                    output.push(TypedColumn {
                        name: alias.clone(),
                        field_type: ty.clone(),
                        nullable,
                        origin: "aggregate".to_string(),
                    });
                    typed_aggs.push(TypedAggregation {
                        function: agg.function.clone(),
                        arg,
                        alias,
                        field_type: ty,
                        nullable,
                        span: Span::empty(0),
                    });
                }
                columns = output.clone();
                RelationNode::Aggregate {
                    input: Box::new(root),
                    aggregations: typed_aggs,
                    groups,
                    columns: output,
                }
            }
            Statement::Sort { field, order } => {
                let typed = type_expr(field, &columns)?;
                RelationNode::Sort {
                    input: Box::new(root),
                    keys: vec![(typed, *order)],
                    columns: columns.clone(),
                }
            }
            Statement::Top { count, by, order } => {
                let keys = by
                    .iter()
                    .map(|e| type_expr(e, &columns).map(|t| (t, *order)))
                    .collect::<Result<Vec<_>, _>>()?;
                let sorted = RelationNode::Sort {
                    input: Box::new(root),
                    keys,
                    columns: columns.clone(),
                };
                RelationNode::Limit {
                    input: Box::new(sorted),
                    count: *count,
                    columns: columns.clone(),
                }
            }
            Statement::Limit(count) => RelationNode::Limit {
                input: Box::new(root),
                count: *count,
                columns: columns.clone(),
            },
            Statement::Offset(count) => RelationNode::Offset {
                input: Box::new(root),
                count: *count,
                columns: columns.clone(),
            },
            Statement::Timeseries { interval } => {
                let output = vec![
                    TypedColumn {
                        name: "timestamp".to_string(),
                        field_type: ValueType::Timestamp,
                        nullable: false,
                        origin: "timeseries".to_string(),
                    },
                    TypedColumn {
                        name: "count".to_string(),
                        field_type: ValueType::Int,
                        nullable: false,
                        origin: "timeseries".to_string(),
                    },
                ];
                columns = output.clone();
                RelationNode::TimeSeries {
                    input: Box::new(root),
                    interval: interval.clone(),
                    columns: output,
                }
            }
            Statement::From(_) => return Err(type_error("source must be the first stage")),
        };
    }
    if let Some(max_complexity) = options.policy.max_complexity {
        if pipeline.statements.len() > max_complexity {
            return Err(type_error("query complexity exceeds policy limit"));
        }
    }
    if let Some(max_rows) = options.policy.max_result_rows {
        root = RelationNode::Limit {
            input: Box::new(root),
            count: max_rows,
            columns: columns.clone(),
        };
    }
    Ok(TypedPipeline {
        output_schema: columns,
        root,
        language_version: options.language_version.clone(),
        target: options.target,
    })
}
fn validate_policy_fields(
    pipeline: &Pipeline,
    policy: &QueryPolicy,
) -> Result<(), DiagnosticBundle> {
    for statement in &pipeline.statements {
        let expressions: Vec<&Expr> = match statement {
            Statement::Where(expr) | Statement::Sort { field: expr, .. } => vec![expr],
            Statement::Summarize { aggregations, by } => aggregations
                .iter()
                .filter_map(|aggregation| aggregation.arg.as_ref())
                .chain(by.iter())
                .collect(),
            Statement::Distinct(exprs) | Statement::Project(exprs) => exprs.iter().collect(),
            Statement::Extend { expr, .. } => vec![expr],
            Statement::Top { by, .. } => by.iter().collect(),
            Statement::From(_)
            | Statement::Limit(_)
            | Statement::Offset(_)
            | Statement::Timeseries { .. } => Vec::new(),
        };
        for expression in expressions {
            validate_policy_expr(expression, policy)?;
        }
    }
    Ok(())
}

fn validate_policy_expr(expression: &Expr, policy: &QueryPolicy) -> Result<(), DiagnosticBundle> {
    if let Expr::Column(name) = expression {
        if policy.sensitive_fields.iter().any(|field| field == name) {
            return Err(DiagnosticBundle::one(Diagnostic::error(
                "LQL120",
                format!("field '{name}' is sensitive and unavailable"),
                Some(Span::empty(0)),
            )));
        }
        if !policy.allowed_fields.is_empty()
            && !policy
                .allowed_fields
                .iter()
                .any(|field| name == field || name.starts_with(&format!("{field}.")))
        {
            return Err(DiagnosticBundle::one(Diagnostic::error(
                "LQL121",
                format!("field '{name}' is not allowed by policy"),
                Some(Span::empty(0)),
            )));
        }
        return Ok(());
    }
    match expression {
        Expr::BinaryOp { left, right, .. } => {
            validate_policy_expr(left, policy)?;
            validate_policy_expr(right, policy)?;
        }
        Expr::UnaryOp { expr, .. } => validate_policy_expr(expr, policy)?,
        Expr::Function { args, .. } => {
            for argument in args {
                validate_policy_expr(argument, policy)?;
            }
        }
        Expr::InList { expr, values, .. } => {
            validate_policy_expr(expr, policy)?;
            for value in values {
                validate_policy_expr(value, policy)?;
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            validate_policy_expr(expr, policy)?;
            validate_policy_expr(low, policy)?;
            validate_policy_expr(high, policy)?;
        }
        Expr::Column(_) | Expr::Parameter(_) | Expr::Literal(_) | Expr::Wildcard => {}
    }
    Ok(())
}

fn schema_columns(schema: &Schema) -> Vec<TypedColumn> {
    let mut result = schema
        .columns
        .values()
        .map(|f| TypedColumn {
            name: f.name.clone(),
            field_type: ValueType::from_field(&f.field_type),
            nullable: schema.nullable(&f.name),
            origin: schema.source.clone(),
        })
        .collect::<Vec<_>>();
    for (name, ty) in [
        (schema.ts_column.as_str(), ValueType::Timestamp),
        (schema.raw_column.as_str(), ValueType::Json),
    ] {
        if !result.iter().any(|c| c.name == name) {
            result.push(TypedColumn {
                name: name.to_string(),
                field_type: ty,
                nullable: true,
                origin: "events".to_string(),
            });
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

fn type_expr(expr: &Expr, columns: &[TypedColumn]) -> Result<TypedExpr, DiagnosticBundle> {
    let span = Span::empty(0);
    let fail = |message: String| {
        Err(DiagnosticBundle::one(Diagnostic::error(
            "LQL102",
            message,
            Some(span),
        )))
    };
    match expr {
        Expr::Column(name) => {
            if let Some(column) = columns.iter().find(|c| c.name == *name) {
                return Ok(TypedExpr {
                    expr: expr.clone(),
                    field_type: column.field_type.clone(),
                    nullable: column.nullable,
                    symbol: Some(column.name.clone()),
                    span,
                });
            }
            if let Some(root) = name.split('.').next() {
                if root == "attrs" {
                    return Ok(TypedExpr {
                        expr: expr.clone(),
                        field_type: ValueType::Dynamic,
                        nullable: true,
                        symbol: Some(name.clone()),
                        span,
                    });
                }
            }
            fail(format!(
                "unknown field '{}'; fields are resolved only from the current stage",
                name
            ))
        }
        Expr::Parameter(_) => Ok(TypedExpr {
            expr: expr.clone(),
            field_type: ValueType::Dynamic,
            nullable: true,
            symbol: None,
            span,
        }),
        Expr::Literal(lit) => {
            let ty = match lit {
                Literal::String(_) => ValueType::String,
                Literal::Timestamp(_) => ValueType::Timestamp,
                Literal::Dynamic(_) => ValueType::Dynamic,
                Literal::Integer(_) => ValueType::Int,
                Literal::Float(_) => ValueType::Float,
                Literal::Bool(_) => ValueType::Bool,
                Literal::Null => ValueType::Unknown,
                Literal::Duration(_) => ValueType::Duration,
            };
            Ok(TypedExpr {
                expr: expr.clone(),
                field_type: ty,
                nullable: matches!(lit, Literal::Null),
                symbol: None,
                span,
            })
        }
        Expr::Wildcard => Ok(TypedExpr {
            expr: expr.clone(),
            field_type: ValueType::Unknown,
            nullable: true,
            symbol: Some("*".to_string()),
            span,
        }),
        whole @ Expr::UnaryOp {
            op,
            expr: inner_expr,
        } => {
            let inner = type_expr(inner_expr, columns)?;
            if matches!(op, UnaryOp::Neg) && !inner.field_type.is_numeric() {
                return Err(type_error("unary minus requires a numeric operand"));
            }
            if matches!(op, UnaryOp::Not) && inner.field_type != ValueType::Bool {
                return Err(type_error("not requires a bool operand"));
            }
            Ok(TypedExpr {
                expr: whole.clone(),
                field_type: if matches!(op, UnaryOp::Not) {
                    ValueType::Bool
                } else {
                    inner.field_type
                },
                nullable: inner.nullable,
                symbol: None,
                span,
            })
        }
        Expr::BinaryOp { left, op, right } => {
            let l = type_expr(left, columns)?;
            let r = type_expr(right, columns)?;
            let result_type = match op {
                BinOp::Add
                    if (l.field_type == ValueType::Timestamp
                        && r.field_type == ValueType::Duration)
                        || (l.field_type == ValueType::Duration
                            && r.field_type == ValueType::Timestamp) =>
                {
                    ValueType::Timestamp
                }
                BinOp::Sub
                    if l.field_type == ValueType::Timestamp
                        && r.field_type == ValueType::Duration =>
                {
                    ValueType::Timestamp
                }
                BinOp::Sub
                    if l.field_type == ValueType::Timestamp
                        && r.field_type == ValueType::Timestamp =>
                {
                    ValueType::Duration
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod
                    if l.field_type.is_numeric() && r.field_type.is_numeric() =>
                {
                    l.field_type.clone()
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    return Err(type_error(
                        "arithmetic requires numeric operands or timestamp-duration operands",
                    ))
                }
                BinOp::And | BinOp::Or
                    if l.field_type != ValueType::Bool || r.field_type != ValueType::Bool =>
                {
                    return Err(type_error("logical operators require bool operands"))
                }
                BinOp::And
                | BinOp::Or
                | BinOp::Eq
                | BinOp::Neq
                | BinOp::Gt
                | BinOp::Lt
                | BinOp::Gte
                | BinOp::Lte
                | BinOp::Like
                | BinOp::NotLike
                | BinOp::Contains
                | BinOp::Has
                | BinOp::StartsWith
                | BinOp::EndsWith
                | BinOp::Matches
                | BinOp::NotMatches => ValueType::Bool,
            };
            Ok(TypedExpr {
                expr: expr.clone(),
                field_type: result_type,
                nullable: l.nullable || r.nullable,
                symbol: None,
                span,
            })
        }
        whole @ Expr::InList {
            expr: value,
            values,
            ..
        } => {
            let left = type_expr(value, columns)?;
            for item in values {
                let _ = type_expr(item, columns)?;
            }
            Ok(TypedExpr {
                expr: whole.clone(),
                field_type: ValueType::Bool,
                nullable: left.nullable,
                symbol: None,
                span,
            })
        }
        whole @ Expr::Between {
            expr: value,
            low,
            high,
            ..
        } => {
            let left = type_expr(value, columns)?;
            let _ = type_expr(low, columns)?;
            let _ = type_expr(high, columns)?;
            Ok(TypedExpr {
                expr: whole.clone(),
                field_type: ValueType::Bool,
                nullable: left.nullable,
                symbol: None,
                span,
            })
        }
        Expr::Function { name, args } => {
            let lower = name.to_ascii_lowercase();
            let valid_arity = match lower.as_str() {
                "now" => args.is_empty(),
                "ago" | "strlen" | "tolower" | "toupper" | "trim" | "typeof" | "to_string"
                | "to_int" | "to_float" | "abs" | "floor" | "ceil" | "log" | "sqrt" | "isempty"
                | "isnotempty" | "array_length" => args.len() == 1,
                "coalesce" => !args.is_empty(),
                "bin" | "split" => args.len() == 2,
                "if" | "substring" | "replace" => args.len() == 3,
                "round" => (1..=2).contains(&args.len()),
                _ => crate::functions::is_builtin_function(&lower),
            };
            if !valid_arity {
                return Err(DiagnosticBundle::one(Diagnostic::error(
                    "LQL104",
                    format!("invalid argument count for {}()", name),
                    Some(span),
                )));
            }
            let typed_args = args
                .iter()
                .map(|a| type_expr(a, columns))
                .collect::<Result<Vec<_>, _>>()?;
            let ty = match lower.as_str() {
                "now" | "ago" | "bin" => ValueType::Timestamp,
                "strlen" | "typeof" | "to_string" | "tolower" | "toupper" | "trim"
                | "substring" | "replace" => ValueType::String,
                "if" => typed_args
                    .get(1)
                    .map(|a| a.field_type.clone())
                    .unwrap_or(ValueType::Unknown),
                "to_int" => ValueType::Int,
                "to_float" => ValueType::Float,
                "isempty" | "isnotempty" => ValueType::Bool,
                _ => typed_args
                    .first()
                    .map(|a| a.field_type.clone())
                    .unwrap_or(ValueType::Unknown),
            };
            Ok(TypedExpr {
                expr: expr.clone(),
                field_type: ty,
                nullable: typed_args.iter().any(|a| a.nullable),
                symbol: None,
                span,
            })
        }
    }
}

fn type_error(message: &str) -> DiagnosticBundle {
    DiagnosticBundle::one(Diagnostic::error("LQL103", message, Some(Span::empty(0))))
}
fn expression_name(expr: &Expr) -> String {
    match expr {
        Expr::Column(name) => name.clone(),
        Expr::Function { name, .. } => name.clone(),
        _ => "expression".to_string(),
    }
}
fn aggregate_name(func: &AggFunction, arg: Option<&TypedExpr>) -> String {
    match func {
        AggFunction::Count => "count".to_string(),
        AggFunction::Sum => "sum".to_string(),
        AggFunction::Avg => "avg".to_string(),
        AggFunction::Min => "min".to_string(),
        AggFunction::Max => "max".to_string(),
        _ => arg
            .map(|a| expression_name(&a.expr))
            .unwrap_or_else(|| "aggregate".to_string()),
    }
}
fn aggregate_type(
    func: &AggFunction,
    arg: Option<&TypedExpr>,
) -> Result<(ValueType, bool), DiagnosticBundle> {
    match func {
        AggFunction::Count | AggFunction::DCount => Ok((ValueType::Int, false)),
        AggFunction::Sum | AggFunction::Avg | AggFunction::Min | AggFunction::Max => {
            let Some(arg) = arg else {
                return Err(type_error("aggregate requires an argument"));
            };
            if !arg.field_type.is_numeric() {
                return Err(type_error("numeric aggregate requires a numeric argument"));
            }
            Ok((arg.field_type.clone(), true))
        }
        _ => Ok((ValueType::Float, true)),
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn policy_rejects_sensitive_projection_and_result_limit() {
        let policy = QueryPolicy::from_value(&serde_json::json!({
            "allowed_sources": ["events"],
            "sensitive_fields": ["message"],
            "max_result_rows": 1
        }))
        .expect("policy");
        let mut options = AnalysisOptions::duckdb();
        options.policy = policy;
        let error = crate::analyze_source("from events | project message", &options)
            .expect_err("sensitive projection must be rejected");
        assert!(error.to_string().contains("sensitive"));
    }

    #[test]
    fn policy_rejects_unknown_fields() {
        let error = QueryPolicy::from_value(&serde_json::json!({
            "allowed_sources": ["events"],
            "not_a_policy_field": true
        }))
        .expect_err("unknown policy field must be rejected");
        assert!(error.contains("unknown field"));
    }
}
