use crate::ast::Pipeline;
use crate::error::{DiagnosticBundle, LqlError};
use crate::ir::{self, AnalysisOptions, QueryPolicy, TypedPipeline};
use crate::schema::Schema;

/// Typed, stage-correct analysis entry point retained under the historical module.
pub fn analyze(
    pipeline: &Pipeline,
    options: &AnalysisOptions,
) -> Result<TypedPipeline, DiagnosticBundle> {
    ir::analyze(pipeline, options)
}

/// Compatibility validator. New callers should use `analyze` and retain the typed plan.
pub fn validate(pipeline: &Pipeline, schema: &Schema) -> Result<(), LqlError> {
    // The caller supplies the schema, so the schema's declared source is the
    // authority for which sources are allowed. Hardcoding the default policy
    // (only `["events"]`) would reject valid traces/incidents/table queries
    // even when the supplied schema declares that source.
    let policy = QueryPolicy {
        allowed_sources: vec![schema.source.clone()],
        ..QueryPolicy::default()
    };
    let options = AnalysisOptions {
        schema: schema.clone(),
        target: crate::compiler::Target::DuckDB,
        policy,
        language_version: "0.1".to_string(),
    };
    analyze(pipeline, &options)
        .map(|_| ())
        .map_err(|bundle| LqlError::Compile {
            message: bundle.to_string(),
            span: bundle.diagnostics.first().and_then(|d| d.primary_span),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_for(source: &str) -> Schema {
        let mut schema = Schema::duckdb_default();
        schema.source = source.to_string();
        schema
    }

    #[test]
    fn validate_accepts_query_matching_supplied_schema_source() {
        for source in ["events", "traces", "incidents"] {
            let pipeline = crate::parse(&format!("from {source}")).unwrap();
            validate(&pipeline, &schema_for(source))
                .unwrap_or_else(|error| panic!("source '{source}' should validate: {error}"));
        }
    }

    #[test]
    fn validate_accepts_table_source_matching_schema() {
        let pipeline = crate::parse("from audit_rows").unwrap();
        validate(&pipeline, &schema_for("audit_rows"))
            .unwrap_or_else(|error| panic!("table source should validate: {error}"));
    }

    #[test]
    fn validate_still_rejects_source_not_declared_by_schema() {
        let pipeline = crate::parse("from incidents").unwrap();
        let error = validate(&pipeline, &schema_for("traces")).unwrap_err();
        assert!(
            error.to_string().contains("not declared or allowed"),
            "{error}"
        );
    }
}
