//! Legacy DuckDB compiler retained for API compatibility.
//!
//! Compilation is delegated to the single IR renderer in [`super`]; new
//! callers should use `parse -> analyze -> render` directly. Keeping this
//! shim avoids a second, drifting dialect implementation.

use crate::ast::Pipeline;
use crate::compiler::Target;
use crate::error::LqlError;
use crate::schema::Schema;

/// Compile a parsed LQL pipeline into DuckDB SQL.
pub fn compile(pipeline: &Pipeline, schema: &Schema) -> Result<String, LqlError> {
    super::compile(pipeline, Target::DuckDB, schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn compile_query(input: &str) -> String {
        let tokens = Lexer::new(input).tokenize().unwrap();
        let pipeline = Parser::new(tokens).parse().unwrap();
        let schema = Schema::duckdb_default();
        compile(&pipeline, &schema).unwrap()
    }

    #[test]
    fn simple_select() {
        let sql = compile_query(r#"from events | where service = "checkout" | limit 10"#);
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("LIMIT 10"));
    }

    #[test]
    fn summarize_count() {
        let sql = compile_query(r#"from events | summarize count() by service"#);
        assert!(sql.contains("COUNT(*)"));
        assert!(sql.contains("GROUP BY"));
    }

    #[test]
    fn where_level_error() {
        let sql = compile_query(r#"from events | where level = "error""#);
        assert!(sql.contains("WHERE"));
        assert!(sql.contains("'error'"));
    }

    #[test]
    fn sort_desc() {
        let sql = compile_query("from events | sort duration_ms desc | limit 5");
        assert!(sql.contains("ORDER BY"));
        assert!(sql.contains("DESC"));
        assert!(sql.contains("LIMIT 5"));
    }

    #[test]
    fn sort_multiple_keys() {
        let sql = compile_query("from events | sort timestamp asc, event_id asc");
        assert!(sql.contains(r#"ORDER BY q."timestamp" ASC, q."event_id" ASC"#));
    }

    #[test]
    fn separately_piped_sorts_compile_as_separate_stages() {
        let sql = compile_query("from events | sort timestamp asc | sort event_id asc");
        assert_eq!(sql.matches("ORDER BY").count(), 2);
    }

    #[test]
    fn offset_compiles() {
        let sql = compile_query("from events | sort timestamp asc | offset 10 | limit 5");
        assert!(sql.contains("OFFSET 10"));
        assert!(sql.contains("LIMIT 5"));
    }

    #[test]
    fn wrapper_matches_ir_renderer() {
        let tokens = Lexer::new("from events | where level = \"error\" | limit 3")
            .tokenize()
            .unwrap();
        let pipeline = Parser::new(tokens).parse().unwrap();
        let schema = Schema::duckdb_default();
        assert_eq!(
            compile(&pipeline, &schema).unwrap(),
            super::super::compile(&pipeline, Target::DuckDB, &schema).unwrap()
        );
    }
}
