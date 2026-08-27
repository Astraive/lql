use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    InvalidConfiguration,
    Transport,
    Authentication,
    Scope,
    Diagnostics,
    CompilerUnavailable,
    Execution,
    Timeout,
    MalformedResponse,
}

#[derive(Debug)]
pub struct QueryError {
    pub category: ErrorCategory,
    pub status: Option<u16>,
    pub message: String,
    pub diagnostics: Vec<Value>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryValue {
    #[serde(rename = "type")]
    pub value_type: String,
    pub value: Value,
}
impl QueryValue {
    pub fn new(value_type: impl Into<String>, value: impl Serialize) -> Self {
        Self {
            value_type: value_type.into(),
            value: serde_json::to_value(value).unwrap_or(Value::Null),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryColumn {
    pub name: String,
    #[serde(default, rename = "type")]
    pub value_type: String,
    #[serde(default)]
    pub nullable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<HashMap<String, Value>>,
    #[serde(default)]
    pub duration_ms: i64,
    #[serde(default)]
    pub row_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionConfig {
    pub dsn: Option<String>,
    pub endpoint: Option<String>,
    pub collector: Option<String>,
    pub api_key: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub env: Option<String>,
    pub service: Option<String>,
    pub database_connection: Option<String>,
    pub timeout: Option<Duration>,
    pub max_response_bytes: Option<usize>,
}
pub struct Client {
    endpoint: String,
    collector: String,
    api_key: String,
    username: String,
    password: String,
    env: String,
    service: String,
    database_connection: Option<String>,
    max_response_bytes: usize,
    http: HttpClient,
}

impl Client {
    pub fn new(mut config: ConnectionConfig) -> Result<Self, QueryError> {
        let parsed = config.dsn.as_deref().map(parse_dsn).transpose()?;
        if let Some(dsn) = parsed {
            if config.endpoint.is_none() {
                config.endpoint = Some(dsn.endpoint);
            }
            if config.collector.is_none() {
                config.collector = Some(dsn.collector);
            }
            if config.env.is_none() {
                config.env = Some(dsn.env);
            }
            if config.service.is_none() {
                config.service = Some(dsn.service);
            }
            if config.username.is_none() {
                config.username = Some(dsn.username);
            }
            if config.password.is_none() {
                config.password = Some(dsn.password);
            }
        }
        let endpoint = config
            .endpoint
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        let endpoint_url =
            Url::parse(&endpoint).map_err(|_| config_error("endpoint must be an HTTP(S) URL"))?;
        if !matches!(endpoint_url.scheme(), "http" | "https")
            || endpoint_url.host_str().is_none()
            || endpoint_url.username() != ""
            || endpoint_url.password().is_some()
        {
            return Err(config_error(
                "endpoint must be an HTTP(S) URL without userinfo",
            ));
        }
        let collector = config.collector.unwrap_or_default();
        if !valid_collector(&collector) {
            return Err(config_error("collector slug is required"));
        }
        let api_key = config.api_key.unwrap_or_default();
        let username = config.username.unwrap_or_default();
        let password = config.password.unwrap_or_default();
        if !username.is_empty() && password.is_empty() && !username.starts_with("lz_pub_") {
            return Err(config_error("basic username requires a password"));
        }
        if !api_key.is_empty() && !username.is_empty() { /* Bearer precedence is intentional. */ }
        if api_key.is_empty()
            && !username.is_empty()
            && endpoint_url.scheme() == "http"
            && !is_localhost(endpoint_url.host_str().unwrap_or_default())
        {
            return Err(config_error("basic authentication requires TLS"));
        }
        let timeout = config.timeout.unwrap_or(Duration::from_secs(30));
        let http = HttpClient::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| transport_error("HTTP client initialization failed", error))?;
        Ok(Self {
            endpoint,
            collector,
            api_key,
            username,
            password,
            env: config.env.unwrap_or_default(),
            service: config.service.unwrap_or_default(),
            database_connection: config
                .database_connection
                .filter(|value| !value.trim().is_empty()),
            max_response_bytes: config.max_response_bytes.unwrap_or(8 << 20),
            http,
        })
    }

    pub fn query(
        &self,
        source: &str,
        parameters: HashMap<String, QueryValue>,
        limit: usize,
    ) -> Result<QueryResult, QueryError> {
        if source.trim().is_empty() {
            return Err(config_error("LQL query source is required"));
        }
        let limit = limit.clamp(1, 1000);
        let url = format!(
            "{}/collectors/{}/lql/query",
            self.endpoint,
            urlencoding(&self.collector)
        );
        let mut body = json!({ "query": source, "parameters": parameters, "limit": limit });
        if let Some(connection) = &self.database_connection {
            body["connection"] = json!(connection);
        }
        let mut request = self.http.post(url).json(&body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        } else if !self.username.is_empty() {
            request = request.basic_auth(&self.username, Some(&self.password));
        }
        if !self.env.is_empty() {
            request = request.header("X-Loza-Env", &self.env);
        }
        if !self.service.is_empty() {
            request = request.header("X-Loza-Service", &self.service);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                timeout_error("LQL query timed out", error)
            } else {
                transport_error("LQL query transport failed", error)
            }
        })?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .map_err(|error| transport_error("LQL response could not be read", error))?;
        if bytes.len() > self.max_response_bytes {
            return Err(QueryError {
                category: ErrorCategory::MalformedResponse,
                status: None,
                message: "LQL response exceeds the configured size limit".into(),
                diagnostics: vec![],
                source: None,
            });
        }
        if !(200..300).contains(&status) {
            return Err(decode_http_error(status, &bytes));
        }
        let payload: Value = serde_json::from_slice(&bytes).map_err(|error| {
            malformed_error("LQL response has an invalid result envelope", error)
        })?;
        let columns = payload
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| config_error("LQL response has no columns"))?
            .iter()
            .map(decode_column)
            .collect::<Result<Vec<_>, _>>()?;
        let rows: Vec<HashMap<String, Value>> =
            serde_json::from_value(payload.get("rows").cloned().unwrap_or(Value::Null))
                .map_err(|error| malformed_error("LQL response has invalid rows", error))?;
        let duration_ms = payload
            .get("duration_ms")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let row_count = payload
            .get("row_count")
            .and_then(Value::as_u64)
            .unwrap_or(rows.len() as u64) as usize;
        Ok(QueryResult {
            columns,
            rows,
            duration_ms,
            row_count,
        })
    }
}

fn parse_dsn(raw: &str) -> Result<DsnParts, QueryError> {
    let normalized = raw
        .strip_prefix("loza://")
        .map(|rest| format!("http://{rest}"))
        .unwrap_or_default();
    let parsed = Url::parse(&normalized).map_err(|_| config_error("invalid DSN"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| config_error("invalid DSN host"))?;
    let collector = parsed.path().trim_start_matches('/').to_string();
    if collector.is_empty() {
        return Err(config_error("invalid DSN collector"));
    }
    let local = is_localhost(host);
    let tls = parsed
        .query_pairs()
        .find(|(key, _)| key == "tls")
        .map(|(_, value)| value == "true")
        .unwrap_or(!local);
    let port = parsed.port().unwrap_or(if local {
        9308
    } else if tls {
        443
    } else {
        80
    });
    let username =
        percent_decode(parsed.username()).map_err(|_| config_error("invalid DSN credentials"))?;
    let password = percent_decode(parsed.password().unwrap_or_default())
        .map_err(|_| config_error("invalid DSN credentials"))?;
    if !username.is_empty() && password.is_empty() && !username.starts_with("lz_pub_") {
        return Err(config_error("invalid DSN credentials"));
    }
    Ok(DsnParts {
        endpoint: format!("{}://{}:{}", if tls { "https" } else { "http" }, host, port),
        collector,
        username,
        password,
        env: parsed
            .query_pairs()
            .find(|(key, _)| key == "env")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_else(|| "default".into()),
        service: parsed
            .query_pairs()
            .find(|(key, _)| key == "service")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_default(),
    })
}

struct DsnParts {
    endpoint: String,
    collector: String,
    username: String,
    password: String,
    env: String,
    service: String,
}
fn valid_collector(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}
fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}
fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect()
}
fn percent_decode(value: &str) -> Result<String, ()> {
    urlencoding::decode(value)
        .map(|value| value.into_owned())
        .map_err(|_| ())
}
fn config_error(message: &str) -> QueryError {
    QueryError {
        category: ErrorCategory::InvalidConfiguration,
        status: None,
        message: format!("invalid LQL connection configuration: {message}"),
        diagnostics: vec![],
        source: None,
    }
}
fn transport_error(
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> QueryError {
    QueryError {
        category: ErrorCategory::Transport,
        status: None,
        message: message.into(),
        diagnostics: vec![],
        source: Some(Box::new(source)),
    }
}
fn timeout_error(
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> QueryError {
    QueryError {
        category: ErrorCategory::Timeout,
        status: None,
        message: message.into(),
        diagnostics: vec![],
        source: Some(Box::new(source)),
    }
}
fn malformed_error(
    message: &str,
    source: impl std::error::Error + Send + Sync + 'static,
) -> QueryError {
    QueryError {
        category: ErrorCategory::MalformedResponse,
        status: None,
        message: message.into(),
        diagnostics: vec![],
        source: Some(Box::new(source)),
    }
}
fn decode_column(value: &Value) -> Result<QueryColumn, QueryError> {
    if let Some(name) = value.as_str() {
        return Ok(QueryColumn {
            name: name.into(),
            value_type: String::new(),
            nullable: false,
        });
    }
    serde_json::from_value(value.clone())
        .map_err(|_| config_error("LQL response has invalid columns"))
}
fn decode_http_error(status: u16, bytes: &[u8]) -> QueryError {
    let payload: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let message = payload
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .unwrap_or("LQL query failed")
        .to_string();
    let category = match status {
        400 => ErrorCategory::Diagnostics,
        401 => ErrorCategory::Authentication,
        403 => ErrorCategory::Scope,
        503 => ErrorCategory::CompilerUnavailable,
        _ => ErrorCategory::Execution,
    };
    QueryError {
        category,
        status: Some(status),
        message,
        diagnostics: payload
            .get("diagnostics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        source: None,
    }
}
