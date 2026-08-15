use crate::ast::*;
use crate::error::LqlError;
use crate::schema::Schema;

/// Validate a pipeline against the schema.
/// Checks that referenced fields exist and function names are valid.
pub fn validate(pipeline: &Pipeline, schema: &Schema) -> Result<(), LqlError> {
    let mut aliases = Vec::new();
    for stmt in &pipeline.statements {
        match stmt {
            Statement::Where(expr) => validate_expr(expr, schema, &aliases)?,
            Statement::Summarize { aggregations, by } => {
                for agg in aggregations {
                    if let Some(arg) = &agg.arg {
                        validate_expr(arg, schema, &aliases)?;
                    }
                }
                for expr in by {
                    validate_expr(expr, schema, &aliases)?;
                }
            }
            Statement::Sort { field, .. } => validate_expr(field, schema, &aliases)?,
            Statement::Distinct(fields) | Statement::Project(fields) => {
                for expr in fields {
                    validate_expr(expr, schema, &aliases)?;
                }
            }
            Statement::Extend { name, expr } => {
                validate_expr(expr, schema, &aliases)?;
                aliases.push(name.clone());
            }
            Statement::Top { by, .. } => {
                for expr in by {
                    validate_expr(expr, schema, &aliases)?;
                }
            }
            Statement::Timeseries { .. } | Statement::From(_) | Statement::Limit(_) => {}
        }
    }
    Ok(())
}

fn validate_expr(expr: &Expr, schema: &Schema, aliases: &[String]) -> Result<(), LqlError> {
    match expr {
        Expr::Column(name) => {
            let base = name.split('.').next().unwrap_or(name);
            let virtual_fields = ["count", "timestamp", "ts", "raw"];
            let nested_roots = ["user", "tenant", "http", "resource", "attrs", "metadata"];
            if !schema.has_field(base)
                && !schema.has_field(name)
                && !virtual_fields.contains(&base)
                && !nested_roots.contains(&base)
                && !aliases.iter().any(|alias| alias == name)
            {
                return Err(LqlError::UnknownField {
                    name: name.clone(),
                    span: crate::error::Span::new(0, 0),
                });
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            validate_expr(left, schema, aliases)?;
            validate_expr(right, schema, aliases)?;
        }
        Expr::UnaryOp { expr, .. } => validate_expr(expr, schema, aliases)?,
        Expr::Function { name, args } => {
            if !crate::functions::is_builtin_function(name) {
                return Err(LqlError::UnknownFunction {
                    name: name.clone(),
                    span: crate::error::Span::new(0, 0),
                });
            }
            crate::functions::validate_function_arity(name, args)?;
            for arg in args {
                validate_expr(arg, schema, aliases)?;
            }
        }
        Expr::InList { expr, values, .. } => {
            validate_expr(expr, schema, aliases)?;
            for v in values {
                validate_expr(v, schema, aliases)?;
            }
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            validate_expr(expr, schema, aliases)?;
            validate_expr(low, schema, aliases)?;
            validate_expr(high, schema, aliases)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn validate_query(input: &str) -> Result<(), LqlError> {
        let tokens = Lexer::new(input).tokenize().unwrap();
        let pipeline = Parser::new(tokens).parse().unwrap();
        let schema = Schema::duckdb_default();
        validate(&pipeline, &schema)
    }

    #[test]
    fn valid_field_passes() {
        assert!(validate_query(r#"from events | where service = "checkout""#).is_ok());
    }

    #[test]
    fn unknown_field_fails() {
        assert!(validate_query(r#"from events | where nonexistent = "x""#).is_err());
    }

    #[test]
    fn nested_field_passes() {
        assert!(validate_query(r#"from events | where user.name = "x""#).is_ok());
    }
}
