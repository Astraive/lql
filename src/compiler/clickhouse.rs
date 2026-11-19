//! Legacy ClickHouse compiler retained for API compatibility.
//!
//! Compilation is delegated to the single IR renderer in [`super`]; new
//! callers should use `parse -> analyze -> render` directly. Keeping this
//! shim avoids a second, drifting dialect implementation.

use crate::ast::Pipeline;
use crate::compiler::Target;
use crate::error::LqlError;
use crate::schema::Schema;

/// Compile a parsed LQL pipeline into ClickHouse SQL.
pub fn compile(pipeline: &Pipeline, schema: &Schema) -> Result<String, LqlError> {
    super::compile(pipeline, Target::ClickHouse, schema)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    #[test]
    fn wrapper_matches_ir_renderer() {
        let tokens = Lexer::new("from events | summarize p95(duration_ms) by service")
            .tokenize()
            .unwrap();
        let pipeline = Parser::new(tokens).parse().unwrap();
        let schema = Schema::clickhouse_default();
        assert_eq!(
            compile(&pipeline, &schema).unwrap(),
            super::super::compile(&pipeline, Target::ClickHouse, &schema).unwrap()
        );
    }
}
