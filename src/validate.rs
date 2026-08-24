use crate::ast::Pipeline;
use crate::error::{DiagnosticBundle, LqlError};
use crate::ir::{self, AnalysisOptions, TypedPipeline};
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
    let options = AnalysisOptions {
        schema: schema.clone(),
        target: crate::compiler::Target::DuckDB,
        policy: Default::default(),
        language_version: "0.1".to_string(),
        clock: None,
    };
    analyze(pipeline, &options)
        .map(|_| ())
        .map_err(|bundle| LqlError::Compile {
            message: bundle.to_string(),
            span: bundle.diagnostics.first().and_then(|d| d.primary_span),
        })
}
