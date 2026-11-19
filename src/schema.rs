use crate::escape::{
    escape_string_literal, is_safe_schema_name, quote_identifier, MAX_SCHEMA_FIELDS,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
/// Metadata for a single field in the Loza event schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldInfo {
    pub name: String,
    pub field_type: FieldType,
    pub description: String,
    pub is_nested: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Integer,
    Float,
    Boolean,
    Timestamp,
    Duration,
    Array,
    Object,
    Any,
}

/// The full Loza event schema, used for validation and column mapping.
#[derive(Debug, Clone)]
pub struct Schema {
    pub source: String,
    pub table: String,
    pub columns: HashMap<String, FieldInfo>,
    pub raw_column: String,
    pub ts_column: String,
    nullability: HashMap<String, bool>,
    sensitivity: HashSet<String>,
    physical_columns: HashMap<String, (String, String)>,
}

impl Schema {
    /// Create the default DuckDB schema (used by the collector).
    pub fn duckdb_default() -> Self {
        let mut columns = HashMap::new();
        let fields = vec![
            ("event_id", FieldType::String, "Unique event identifier"),
            ("timestamp", FieldType::Timestamp, "Event timestamp"),
            ("service", FieldType::String, "Service name"),
            ("version", FieldType::String, "Service version"),
            ("environment", FieldType::String, "Deployment environment"),
            ("collector", FieldType::String, "Collector scope"),
            ("attrs", FieldType::Object, "Structured dynamic attributes"),
            ("event", FieldType::String, "Event name"),
            ("kind", FieldType::String, "Event kind (event, log, metric, span)"),
            ("level", FieldType::String, "Log level (debug, info, notice, warn, error, fatal)"),
            ("outcome", FieldType::String, "Event outcome (success, error, partial, abandoned, retried, cancelled, timeout, skipped, rejected, quarantined, unknown)"),
            ("message", FieldType::String, "Human-readable message"),
            ("error_code", FieldType::String, "Error code"),
            ("error_type", FieldType::String, "Error type"),
            ("error_message", FieldType::String, "Error message"),
            ("duration_ms", FieldType::Float, "Duration in milliseconds"),
            ("started_at", FieldType::Timestamp, "Event start time"),
            ("finished_at", FieldType::Timestamp, "Event finish time"),
            ("event_state", FieldType::String, "Lifecycle state"),
            ("request_id", FieldType::String, "Request identifier"),
            ("trace_id", FieldType::String, "Distributed trace identifier"),
            ("span_id", FieldType::String, "Span identifier"),
            ("incident_id", FieldType::String, "Incident identifier"),
            ("method", FieldType::String, "HTTP method"),
            ("path", FieldType::String, "HTTP path"),
            ("route", FieldType::String, "HTTP route"),
            ("status_code", FieldType::Integer, "HTTP status code"),
            ("user_agent", FieldType::String, "User agent string"),
            ("http_status", FieldType::Integer, "HTTP status"),
            ("region", FieldType::String, "Deployment region"),
            ("host", FieldType::String, "Host name"),
            ("release", FieldType::String, "Release identifier"),
            ("deployment_id", FieldType::String, "Deployment identifier"),
            ("sdk_name", FieldType::String, "SDK name"),
            ("sdk_version", FieldType::String, "SDK version"),
            ("schema_version", FieldType::String, "Event schema version"),
            ("event_version", FieldType::String, "Event version"),
        ];

        for (name, ft, desc) in fields {
            columns.insert(
                name.to_string(),
                FieldInfo {
                    name: name.to_string(),
                    field_type: ft,
                    description: desc.to_string(),
                    is_nested: false,
                },
            );
        }

        Schema {
            source: "events".to_string(),
            table: "events".to_string(),
            nullability: columns.keys().map(|name| (name.clone(), true)).collect(),
            sensitivity: HashSet::new(),
            physical_columns: HashMap::new(),
            columns,
            raw_column: "raw".to_string(),
            ts_column: "ts".to_string(),
        }
    }

    /// Load a versioned schema document for one declared logical source.
    pub fn from_document(document: &Value, source: &str) -> Result<Self, String> {
        let version = document
            .get("schema_version")
            .and_then(Value::as_str)
            .ok_or_else(|| "schema_version is required".to_string())?;
        if version != "v1" {
            return Err(format!("unsupported schema_version '{version}'"));
        }
        let source_document = document
            .get("sources")
            .and_then(Value::as_object)
            .and_then(|sources| sources.get(source))
            .and_then(Value::as_object)
            .ok_or_else(|| format!("source '{source}' is not declared"))?;
        let table = source_document
            .get("physical")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("source '{source}' physical table is required"))?;
        if !is_safe_schema_name(table) {
            return Err(format!(
                "source '{source}' has an unsafe physical table name"
            ));
        }
        let fields = source_document
            .get("fields")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("source '{source}' fields are required"))?;
        if fields.len() > MAX_SCHEMA_FIELDS {
            return Err(format!(
                "source '{source}' has too many fields (limit {MAX_SCHEMA_FIELDS})"
            ));
        }
        let mut columns = HashMap::new();
        let mut nullability = HashMap::new();
        let mut sensitivity = HashSet::new();
        let mut physical_columns = HashMap::new();
        let mut raw_column = "raw".to_string();
        for field in fields {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "schema field name is required".to_string())?;
            if !is_valid_field_name(name) {
                return Err(format!("invalid schema field name '{name}'"));
            }
            if !is_safe_schema_name(name) {
                return Err(format!("schema field name {name:?} is not allowed"));
            }
            if columns.contains_key(name) {
                return Err(format!("duplicate schema field '{name}'"));
            }
            let field_type_name = field
                .get("field_type")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("schema field '{name}' type is required"))?;
            let field_type = parse_field_type(field_type_name)?;
            let physical = field
                .get("physical")
                .and_then(Value::as_object)
                .ok_or_else(|| format!("schema field '{name}' physical mapping is required"))?;
            let physical_column = physical
                .get("column")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("schema field '{name}' physical column is required"))?;
            if !is_valid_ident(physical_column) {
                return Err(format!(
                    "invalid physical column '{physical_column}' for schema field '{name}'"
                ));
            }
            if !is_safe_schema_name(physical_column) {
                return Err(format!(
                    "schema field {name:?} has an unsafe physical column name"
                ));
            }
            let storage = physical
                .get("storage")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("schema field '{name}' storage is required"))?;
            if !matches!(storage, "raw" | "projection") {
                return Err(format!(
                    "schema field '{name}' has unsupported storage '{storage}'"
                ));
            }
            if storage == "raw" {
                raw_column = physical_column.to_string();
            }
            let nullable = field
                .get("nullable")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let field_sensitivity = field
                .get("sensitivity")
                .and_then(Value::as_str)
                .unwrap_or("public");
            if field_sensitivity != "public" {
                sensitivity.insert(name.to_string());
            }
            nullability.insert(name.to_string(), nullable);
            physical_columns.insert(
                name.to_string(),
                (physical_column.to_string(), storage.to_string()),
            );
            columns.insert(
                name.to_string(),
                FieldInfo {
                    name: name.to_string(),
                    field_type,
                    description: field
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    is_nested: field
                        .get("structured")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
            );
        }
        Ok(Self {
            source: source.to_string(),
            table: table.to_string(),
            columns,
            raw_column,
            ts_column: "ts".to_string(),
            nullability,
            sensitivity,
            physical_columns,
        })
    }

    /// Load the bundled revisioned LOZA schema for a target.
    pub fn loza_v1(target: crate::compiler::Target) -> Self {
        let document: Value = serde_json::from_str(include_str!("../schemas/loza-v1.json"))
            .expect("valid bundled LOZA schema");
        let mut schema =
            Self::from_document(&document, "events").expect("valid bundled LOZA events schema");
        if matches!(target, crate::compiler::Target::ClickHouse) {
            schema.table = "loza_events".to_string();
        }
        schema
    }

    /// Canonical default schema for a target.
    ///
    /// Every public entry point (WASM, CLI, RPC and the compatibility wrappers)
    /// must use this single default so query validity does not depend on which
    /// interface a caller happens to use.
    pub fn default_for(target: crate::compiler::Target) -> Self {
        Self::loza_v1(target)
    }

    /// Create the default ClickHouse schema.
    pub fn clickhouse_default() -> Self {
        let mut schema = Self::duckdb_default();
        schema.table = "loza_events".to_string();
        schema.ts_column = "ts".to_string();
        schema.raw_column = "raw".to_string();
        schema
    }

    /// Check if a field name exists in the schema.
    pub fn has_field(&self, name: &str) -> bool {
        self.columns.contains_key(name)
    }

    pub fn nullable(&self, name: &str) -> bool {
        self.nullability.get(name).copied().unwrap_or(true)
    }

    pub fn is_sensitive(&self, name: &str) -> bool {
        self.sensitivity.contains(name)
    }

    /// Get the SQL expression for a field. Identifiers are quoted and field
    /// names are escaped with the shared dialect-aware helpers so that a
    /// client-supplied name can never break out of its SQL literal.
    pub fn column_expr(&self, field: &str, target: crate::compiler::Target) -> String {
        use crate::compiler::Target;
        let (column, storage) = self
            .physical_columns
            .get(field)
            .map(|(column, storage)| (column.as_str(), storage.as_str()))
            .unwrap_or((self.raw_column.as_str(), "raw"));
        if storage == "projection" {
            return quote_identifier(column, target);
        }
        let column = quote_identifier(column, target);
        let field = escape_string_literal(field, target);
        match target {
            Target::DuckDB => format!("json_extract_string({column}, '$.{field}')"),
            Target::ClickHouse => format!("JSONExtractString({column}, '{field}')"),
            Target::PostgreSQL => format!("({column}::jsonb ->> '{field}')"),
        }
    }

    /// Return all known field names for autocomplete/validation.
    pub fn field_names(&self) -> Vec<&str> {
        self.columns.keys().map(|s| s.as_str()).collect()
    }
}

fn parse_field_type(name: &str) -> Result<FieldType, String> {
    match name {
        "string" => Ok(FieldType::String),
        "int" => Ok(FieldType::Integer),
        "float" => Ok(FieldType::Float),
        "bool" => Ok(FieldType::Boolean),
        "timestamp" => Ok(FieldType::Timestamp),
        "duration" => Ok(FieldType::Duration),
        "object" => Ok(FieldType::Object),
        "array<dynamic>" | "array" => Ok(FieldType::Array),
        "dynamic" | "any" => Ok(FieldType::Any),
        other => Err(format!("unsupported schema field type '{other}'")),
    }
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Escape a value for inclusion in a standard SQL single-quoted string
/// literal. Doubling single quotes works for DuckDB and PostgreSQL, where a
/// backslash is an ordinary character in a standard string literal.
fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// ClickHouse string literals use backslash escape sequences, so both
/// backslashes and single quotes must be escaped.
fn escape_clickhouse_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Strict SQL identifier: an ASCII letter or underscore followed by ASCII
/// letters, digits, or underscores. Rejects quotes, backslashes, whitespace
/// and anything else that could alter generated SQL.
fn is_valid_ident(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// JSON path field name: one or more dot-separated strict identifiers.
fn is_valid_field_name(value: &str) -> bool {
    !value.is_empty() && value.split('.').all(is_valid_ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schema_has_core_fields() {
        let schema = Schema::duckdb_default();
        assert!(schema.has_field("event_id"));
        assert!(schema.has_field("service"));
        assert!(schema.has_field("level"));
        assert!(schema.has_field("duration_ms"));
        assert!(schema.has_field("trace_id"));
        assert!(!schema.has_field("nonexistent"));
    }

    #[test]
    fn column_expr_duckdb() {
        let schema = Schema::duckdb_default();
        let expr = schema.column_expr("service", crate::compiler::Target::DuckDB);
        assert_eq!(expr, "json_extract_string(\"raw\", '$.service')");
    }

    #[test]
    fn column_expr_clickhouse() {
        let schema = Schema::clickhouse_default();
        let expr = schema.column_expr("service", crate::compiler::Target::ClickHouse);
        assert_eq!(expr, "JSONExtractString(`raw`, 'service')");
    }

    #[test]
    fn column_expr_escapes_field_payloads_for_every_dialect() {
        let schema = Schema::duckdb_default();
        let payload = "x' OR 1=1 --";
        assert_eq!(
            schema.column_expr(payload, crate::compiler::Target::DuckDB),
            "json_extract_string(\"raw\", '$.x'' OR 1=1 --')"
        );
        assert_eq!(
            schema.column_expr(payload, crate::compiler::Target::ClickHouse),
            "JSONExtractString(`raw`, 'x\\' OR 1=1 --')"
        );
        assert_eq!(
            schema.column_expr(payload, crate::compiler::Target::PostgreSQL),
            "(\"raw\"::jsonb ->> 'x'' OR 1=1 --')"
        );
    }

    #[test]
    fn column_expr_quotes_projection_identifiers() {
        let document = serde_json::json!({
            "schema_version": "v1",
            "sources": {
                "events": {
                    "physical": "events_rows",
                    "fields": [{
                        "name": "service",
                        "field_type": "string",
                        "physical": {
                            "column": "svc_name",
                            "storage": "projection"
                        }
                    }]
                }
            }
        });
        let schema = Schema::from_document(&document, "events").expect("schema document");
        assert_eq!(
            schema.column_expr("service", crate::compiler::Target::DuckDB),
            "\"svc_name\""
        );
        assert_eq!(
            schema.column_expr("service", crate::compiler::Target::ClickHouse),
            "`svc_name`"
        );
    }
}

#[cfg(test)]
mod revisioned_document_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn revisioned_document_loads_declared_source_and_mapping() {
        let document = json!({
            "schema_version": "v1",
            "sources": {
                "audit": {
                    "physical": "audit_rows",
                    "fields": [{
                        "name": "message",
                        "field_type": "string",
                        "nullable": false,
                        "sensitivity": "private",
                        "physical": {
                            "source": "audit_rows",
                            "column": "payload",
                            "storage": "raw"
                        }
                    }]
                }
            }
        });
        let schema = Schema::from_document(&document, "audit").expect("schema document");
        assert_eq!(schema.source, "audit");
        assert_eq!(schema.table, "audit_rows");
        assert_eq!(
            schema.column_expr("message", crate::compiler::Target::DuckDB),
            "json_extract_string(\"payload\", '$.message')"
        );
        assert!(!schema.nullable("message"));
        assert!(schema.is_sensitive("message"));
    }

    fn document_with_names(name: &str, column: &str, table: &str) -> serde_json::Value {
        json!({
            "schema_version": "v1",
            "sources": {
                "events": {
                    "physical": table,
                    "fields": [{
                        "name": name,
                        "field_type": "string",
                        "physical": {
                            "column": column,
                            "storage": "projection"
                        }
                    }]
                }
            }
        })
    }

    #[test]
    fn malicious_schema_names_are_rejected() {
        for payload in ["a'b", "a;b", "a\\b", "a\"b", "a) --", ""] {
            let document = document_with_names(payload, "raw", "events_rows");
            assert!(
                Schema::from_document(&document, "events").is_err(),
                "field name {payload:?} should be rejected"
            );

            let document = document_with_names("message", payload, "events_rows");
            assert!(
                Schema::from_document(&document, "events").is_err(),
                "physical column {payload:?} should be rejected"
            );

            let document = document_with_names("message", "raw", payload);
            assert!(
                Schema::from_document(&document, "events").is_err(),
                "table name {payload:?} should be rejected"
            );
        }
    }

    #[test]
    fn oversized_schema_documents_are_rejected() {
        let too_long = "a".repeat(crate::escape::MAX_SCHEMA_NAME_LEN + 1);
        let document = document_with_names(&too_long, "raw", "events_rows");
        assert!(Schema::from_document(&document, "events").is_err());

        let fields = (0..=crate::escape::MAX_SCHEMA_FIELDS)
            .map(|i| {
                json!({
                    "name": format!("field_{i}"),
                    "field_type": "string",
                    "physical": { "column": "raw", "storage": "raw" }
                })
            })
            .collect::<Vec<_>>();
        let document = json!({
            "schema_version": "v1",
            "sources": { "events": { "physical": "events_rows", "fields": fields } }
        });
        assert!(Schema::from_document(&document, "events").is_err());
    }
}

#[cfg(test)]
mod sql_injection_regression_tests {
    use super::*;
    use serde_json::json;

    fn document_with(column: &str, name: &str) -> serde_json::Value {
        json!({
            "schema_version": "v1",
            "sources": {
                "events": {
                    "physical": "events",
                    "fields": [{
                        "name": name,
                        "field_type": "string",
                        "physical": {
                            "column": column,
                            "storage": "raw"
                        }
                    }]
                }
            }
        })
    }

    #[test]
    fn rejects_field_name_that_breaks_out_of_sql_literal() {
        let document = document_with("raw", r#"service' OR '1'='1"#);
        let error = Schema::from_document(&document, "events").unwrap_err();
        assert!(error.contains("invalid schema field name"), "{error}");
    }

    #[test]
    fn rejects_physical_column_containing_sql() {
        let document = document_with("raw' || (SELECT 1) || '", "message");
        let error = Schema::from_document(&document, "events").unwrap_err();
        assert!(error.contains("invalid physical column"), "{error}");
    }

    #[test]
    fn column_expr_escapes_field_for_every_target() {
        let schema = Schema::duckdb_default();
        let field = r#"x' OR '1'='1"#;
        assert_eq!(
            schema.column_expr(field, crate::compiler::Target::DuckDB),
            r#"json_extract_string("raw", '$.x'' OR ''1''=''1')"#
        );
        assert_eq!(
            schema.column_expr(field, crate::compiler::Target::ClickHouse),
            r"JSONExtractString(`raw`, 'x\' OR \'1\'=\'1')"
        );
        assert_eq!(
            schema.column_expr(field, crate::compiler::Target::PostgreSQL),
            r#"("raw"::jsonb ->> 'x'' OR ''1''=''1')"#
        );
    }

    #[test]
    fn clickhouse_escapes_backslashes_in_field() {
        let schema = Schema::clickhouse_default();
        assert_eq!(
            schema.column_expr(r"a\b", crate::compiler::Target::ClickHouse),
            r"JSONExtractString(`raw`, 'a\\b')"
        );
    }

    #[test]
    fn valid_dotted_field_names_are_still_accepted() {
        let document = document_with("payload", "error.code");
        let schema = Schema::from_document(&document, "events").expect("valid dotted field name");
        assert_eq!(
            schema.column_expr("error.code", crate::compiler::Target::DuckDB),
            "json_extract_string(\"payload\", '$.error.code')"
        );
    }
}
