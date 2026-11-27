use crate::{Diagnostic, DiagnosticBundle, QueryPolicy, Schema, Source, Target};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

pub const PROTOCOL_VERSION: u32 = 1;
pub const LANGUAGE_VERSION: &str = "0.1";

/// Maximum number of bytes in a single stdio JSON-RPC request line, excluding
/// the trailing newline. Lines longer than this are rejected before parsing so
/// an untrusted client cannot exhaust process memory with one request.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

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

#[derive(Debug, Clone, Deserialize)]
struct CheckParams {
    query: String,
    #[serde(default = "default_target")]
    target: String,
}
fn default_target() -> String {
    "duckdb".to_string()
}

fn parse_target(target: &str) -> Result<Target, ()> {
    match target.to_ascii_lowercase().as_str() {
        "duckdb" => Ok(Target::DuckDB),
        "clickhouse" => Ok(Target::ClickHouse),
        "postgres" | "postgresql" => Ok(Target::PostgreSQL),
        _ => Err(()),
    }
}

fn analysis_options(target: Target) -> crate::AnalysisOptions {
    match target {
        Target::DuckDB => crate::AnalysisOptions::duckdb(),
        Target::ClickHouse => crate::AnalysisOptions::clickhouse(),
        Target::PostgreSQL => crate::AnalysisOptions::postgres(),
    }
}

/// Outcome of reading one bounded line from the stdio stream.
enum BoundedLine {
    /// The underlying reader reached EOF with no remaining bytes.
    Eof,
    /// A complete line was read (newline stripped, `\r` parity with `lines()`).
    Line(Vec<u8>),
    /// The line exceeded `max` bytes; its remainder was drained through the
    /// newline so that the next line can still be processed.
    TooLong,
}

/// Read a single line while keeping allocation bounded to `max` bytes.
///
/// Unlike `BufRead::lines`, this never accumulates an unbounded line into a
/// `String`. Once the limit is reached, accumulation stops and the rest of the
/// line is discarded up to (and including) the next newline.
fn read_line_bounded<R: BufRead>(reader: &mut R, max: usize) -> io::Result<BoundedLine> {
    let mut buf = Vec::new();
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if too_long {
                Ok(BoundedLine::TooLong)
            } else if buf.is_empty() {
                Ok(BoundedLine::Eof)
            } else {
                Ok(BoundedLine::Line(buf))
            };
        }
        if let Some(pos) = available.iter().position(|&byte| byte == b'\n') {
            let (chunk, _) = available.split_at(pos);
            if !too_long && buf.len() + chunk.len() <= max {
                buf.extend_from_slice(chunk);
                reader.consume(pos + 1);
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                return Ok(BoundedLine::Line(buf));
            }
            // The line is oversized. Consume through the newline and report it
            // without ever copying the oversized payload into `buf`.
            reader.consume(pos + 1);
            return Ok(BoundedLine::TooLong);
        }
        // No newline in the buffered chunk: copy it only while within budget.
        let len = available.len();
        if !too_long && buf.len() + len <= max {
            buf.extend_from_slice(available);
        } else {
            too_long = true;
            buf.clear();
        }
        reader.consume(len);
    }
}

fn write_response<W: Write>(writer: &mut W, response: &RpcResponse) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, response)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub fn serve_stdio<R: BufRead, W: Write>(reader: R, mut writer: W) -> io::Result<()> {
    let mut reader = reader;
    loop {
        match read_line_bounded(&mut reader, MAX_LINE_BYTES)? {
            BoundedLine::Eof => break,
            BoundedLine::TooLong => {
                write_response(
                    &mut writer,
                    &failure(
                        Value::Null,
                        "line_too_long",
                        &format!("request line exceeds {MAX_LINE_BYTES} bytes"),
                        None,
                    ),
                )?;
            }
            BoundedLine::Line(bytes) => {
                let line = String::from_utf8_lossy(&bytes);
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
                write_response(&mut writer, &response)?;
            }
        }
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
    let target = match parse_target(&params.target) {
        Ok(target) => target,
        Err(()) => return failure(id, "invalid_target", "unsupported target", None),
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
    let params = match serde_json::from_value::<CheckParams>(params) {
        Ok(params) => params,
        Err(error) => return failure(id, "invalid_params", &error.to_string(), None),
    };
    let target = match parse_target(&params.target) {
        Ok(target) => target,
        Err(()) => return failure(id, "invalid_target", "unsupported target", None),
    };
    match crate::analyze_source(&params.query, &analysis_options(target)) {
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
    fn stdio_rejects_oversized_line_and_keeps_serving() {
        let oversized = "x".repeat(MAX_LINE_BYTES + 1);
        let valid = json!({
            "id": 42, "method": "version", "protocol_version": 1, "language_version": "0.1"
        });
        let input = format!("{oversized}\n{valid}\n");
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();

        let responses: Vec<Value> = output
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(
            responses.len(),
            2,
            "expected error response then normal response"
        );
        assert_eq!(responses[0]["error"]["code"], "line_too_long");
        assert!(responses[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exceeds"));
        assert_eq!(responses[1]["id"], 42);
        assert_eq!(
            responses[1]["result"]["compiler_version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn stdio_rejects_oversized_line_at_eof() {
        let oversized = "x".repeat(MAX_LINE_BYTES + 1);
        let mut output = Vec::new();
        serve_stdio(Cursor::new(oversized), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["error"]["code"], "line_too_long");
    }

    #[test]
    fn stdio_accepts_line_at_the_size_limit() {
        let base = json!({
            "id": "", "method": "version", "protocol_version": 1, "language_version": "0.1"
        });
        let base_len = serde_json::to_string(&base).unwrap().len();
        let id = "x".repeat(MAX_LINE_BYTES - base_len);
        let request = json!({
            "id": id, "method": "version", "protocol_version": 1, "language_version": "0.1"
        });
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(line.len(), MAX_LINE_BYTES);

        let mut output = Vec::new();
        serve_stdio(Cursor::new(format!("{line}\n")), &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], id);
        assert!(response["result"].is_object());
        assert!(response["error"].is_null());
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

    fn check_response(id: u32, target: &str) -> Value {
        let input = format!(
            "{}\n",
            json!({
                "id": id, "method": "check", "protocol_version": 1,
                "language_version": "0.1",
                "params": {
                    "query": "from events | where level = \"error\" | take 2",
                    "target": target
                }
            })
        );
        let mut output = Vec::new();
        serve_stdio(Cursor::new(input), &mut output).unwrap();
        serde_json::from_slice(&output).unwrap()
    }

    #[test]
    fn stdio_check_rejects_unknown_target() {
        let response = check_response(10, "bogus");
        assert_eq!(response["error"]["code"], "invalid_target");
    }

    #[test]
    fn stdio_check_accepts_supported_targets() {
        for (id, target) in [(11, "duckdb"), (12, "clickhouse"), (13, "postgres")] {
            let response = check_response(id, target);
            assert_eq!(response["id"], id);
            assert_eq!(response["result"]["ok"], true, "target {target} failed");
        }
    }

    #[test]
    fn check_rejects_malformed_parameters_and_keeps_defaults_and_target_options() {
        for params in [
            json!({}),
            json!({"query": 99}),
            json!({"query": "from events", "target": 99}),
        ] {
            assert_eq!(
                check(json!(14), params).error.unwrap().code,
                "invalid_params"
            );
        }
        assert!(check(json!(15), json!({"query": "from events"}))
            .error
            .is_none());
        assert!(check(json!(16), json!({"query": "from events", "target": "postgres"}))
            .error
            .is_none());
    }
}
