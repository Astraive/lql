use crate::{Diagnostic, DiagnosticBundle, QueryPolicy, Schema, Source, Target};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const LANGUAGE_VERSION: &str = "0.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: Value,
    pub method: String,
    pub protocol_version: u32,
    pub language_version: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileParams {
    pub query: String,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub schema: Value,
    #[serde(default)]
    pub policy: Value,
    #[serde(default)]
    pub scope: Value,
}
fn default_target() -> String {
    "duckdb".to_string()
}

pub fn serve_stdio<R: BufRead, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => handle(request),
            Err(error) => RpcResponse {
                id: Value::Null,
                result: None,
                error: Some(RpcError {
                    code: "invalid_request".to_string(),
                    message: error.to_string(),
                    diagnostics: None,
                }),
            },
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn handle(request: RpcRequest) -> RpcResponse {
    if request.protocol_version != PROTOCOL_VERSION || request.language_version != LANGUAGE_VERSION
    {
        return failure(
            request.id,
            "version_mismatch",
            "protocol or language version mismatch",
            None,
        );
    }
    match request.method.as_str() {
        "handshake" => success(
            request.id,
            json!({
                "compiler_version": env!("CARGO_PKG_VERSION"),
                "language_version": LANGUAGE_VERSION,
                "protocol_version": PROTOCOL_VERSION,
                "targets": ["duckdb", "clickhouse"]
            }),
        ),
        "version" => success(
            request.id,
            json!({
                "compiler_version": env!("CARGO_PKG_VERSION"),
                "language_version": LANGUAGE_VERSION,
                "protocol_version": PROTOCOL_VERSION
            }),
        ),
        "compile" => compile(request.id, request.params),
        "check" => check(request.id, request.params),
        "complete" => success(request.id, json!({ "items": crate::known_fields() })),
        _ => failure(
            request.id,
            "method_not_found",
            "unsupported protocol method",
            None,
        ),
    }
}

fn compile(id: Value, params: Value) -> RpcResponse {
    let params = match serde_json::from_value::<CompileParams>(params) {
        Ok(params) => params,
        Err(error) => return failure(id, "invalid_params", &error.to_string(), None),
    };
    let target = match params.target.to_ascii_lowercase().as_str() {
        "duckdb" => Target::DuckDB,
        "clickhouse" => Target::ClickHouse,
        _ => return failure(id, "invalid_target", "unsupported target", None),
    };
    let policy = match QueryPolicy::from_value(&params.policy) {
        Ok(policy) => policy,
        Err(error) => return failure(id, "invalid_policy", &error, None),
    };
    let schema = match schema_for_query(&params.query, &params.schema, target) {
        Ok(schema) => schema,
        Err(error) => return failure(id, "invalid_schema", &error, None),
    };
    match crate::render_query_with_options(
        &params.query,
        target,
        &params.parameters,
        &params.scope,
        schema,
        policy,
    ) {
        Ok(plan) => success(id, json!({ "plan": plan })),
        Err(bundle) => failure(id, "compile_failed", "LQL compilation failed", Some(bundle)),
    }
}

fn schema_for_query(query: &str, document: &Value, target: Target) -> Result<Schema, String> {
    let default = || Schema::loza_v1(target);
    if document.is_null() {
        return Ok(default());
    }
    let pipeline = crate::parse_diagnostics(query).map_err(|error| error.to_string())?;
    let Some(crate::Statement::From(source)) = pipeline.statements.first() else {
        return Err("query must start with a source".to_string());
    };
    let source_name = match source {
        Source::Events => "events",
        Source::Traces => "traces",
        Source::Incidents => "incidents",
        Source::Table(name) => name,
    };
    Schema::from_document(document, source_name)
}
fn check(id: Value, params: Value) -> RpcResponse {
    let query = params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match crate::analyze_source(query, &crate::AnalysisOptions::duckdb()) {
        Ok(_) => success(id, json!({ "ok": true })),
        Err(bundle) => failure(id, "check_failed", "LQL check failed", Some(bundle)),
    }
}

fn success(id: Value, result: Value) -> RpcResponse {
    RpcResponse {
        id,
        result: Some(result),
        error: None,
    }
}
fn failure(
    id: Value,
    code: &str,
    message: &str,
    diagnostics: Option<DiagnosticBundle>,
) -> RpcResponse {
    RpcResponse {
        id,
        result: None,
        error: Some(RpcError {
            code: code.to_string(),
            message: message.to_string(),
            diagnostics: diagnostics.map(|b| b.diagnostics),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn stdio_compile_returns_plan_and_bindings() {
        let input = format!(
            "{}\n",
            json!({
                "id": 1, "method": "compile", "protocol_version": 1,
                "language_version": "0.1", "params": {"query": "from events | where level = \"error\" | take 2", "target": "duckdb"}
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(
            response["result"]["plan"]["parameters"][0]["value"],
            "error"
        );
    }

    #[test]
    fn stdio_compile_binds_named_parameter_value() {
        let input = format!(
            "{}\n",
            json!({
                "id": 2, "method": "compile", "protocol_version": 1,
                "language_version": "0.1",
                "params": {
                    "query": "from events | where event_id = $id",
                    "target": "duckdb",
                    "parameters": {"id": {"type": "string", "value": "evt-1"}}
                }
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            response["result"]["plan"]["parameters"][0]["value"],
            "evt-1"
        );
    }

    #[test]
    fn stdio_compile_applies_schema_and_policy() {
        let input = format!(
            "{}\n",
            json!({
                "id": 3, "method": "compile", "protocol_version": 1,
                "language_version": "0.1",
                "params": {
                    "query": "from events | project message",
                    "target": "duckdb",
                    "policy": {"allowed_sources": ["events"], "sensitive_fields": ["message"]}
                }
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], "compile_failed");
        assert!(response["error"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("sensitive"));
    }

    #[test]
    fn stdio_compile_rejects_unknown_policy_fields() {
        let input = format!(
            "{}\n",
            json!({
                "id": 4, "method": "compile", "protocol_version": 1,
                "language_version": "0.1",
                "params": {
                    "query": "from events",
                    "target": "duckdb",
                    "policy": {"unknown": true}
                }
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], "invalid_policy");
    }

    #[test]
    fn stdio_compile_uses_declared_revisioned_source() {
        let input = format!(
            "{}\n",
            json!({
                "id": 5, "method": "compile", "protocol_version": 1,
                "language_version": "0.1",
                "params": {
                    "query": "from audit | project message",
                    "target": "duckdb",
                    "schema": {
                        "schema_version": "v1",
                        "sources": {
                            "audit": {
                                "physical": "audit_rows",
                                "fields": [{
                                    "name": "message",
                                    "field_type": "string",
                                    "nullable": false,
                                    "physical": {
                                        "source": "audit_rows",
                                        "column": "payload",
                                        "storage": "raw"
                                    }
                                }]
                            }
                        }
                    },
                    "policy": {"allowed_sources": ["audit"]}
                }
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert!(response["result"]["plan"]["sql"]
            .as_str()
            .unwrap()
            .contains("\"audit\""));
    }
}
