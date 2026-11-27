use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};

use lql_client::{Client, ConnectionConfig, QueryValue};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_output = args.iter().skip(1).any(|arg| arg == "--json");
    let command_index = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, arg)| !arg.starts_with('-'))
        .map(|(index, _)| index);
    let Some(command_index) = command_index else {
        print_usage();
        std::process::exit(1);
    };
    let command = args[command_index].as_str();
    match command {
        "serve" => run_serve(&args, command_index),
        "compile" | "compile-ch" | "check" => {
            run_offline(command, &args[command_index + 1..], json_output)
        }
        "fields" => run_fields(json_output),
        "query" => run_query(&args[command_index + 1..], json_output),
        "connect" | "shell" => run_shell(&args[command_index + 1..]),
        "--help" | "-h" => print_usage(),
        "--version" | "-v" => println!("lql {}", env!("CARGO_PKG_VERSION")),
        _ => emit_message_error(json_output, &format!("unknown command: {}", command)),
    }
}

fn run_serve(args: &[String], command_index: usize) {
    if args.get(command_index + 1).map(String::as_str) != Some("--stdio") {
        emit_message_error(false, "serve requires --stdio");
    }
    if let Err(error) = lql::protocol::serve_stdio(BufReader::new(io::stdin()), io::stdout()) {
        eprintln!("protocol error: {}", error);
        std::process::exit(1);
    }
}

fn run_offline(command: &str, command_args: &[String], json_output: bool) {
    let query_args: Vec<&str> = command_args
        .iter()
        .filter(|arg| arg.as_str() != "--json")
        .map(String::as_str)
        .collect();
    let mut input = if query_args.is_empty() {
        let mut buf = String::new();
        if let Err(err) = io::stdin().read_to_string(&mut buf) {
            emit_message_error(json_output, &format!("failed to read stdin: {}", err));
        }
        buf
    } else {
        query_args.join(" ")
    };
    input = input.trim().to_string();
    if input.is_empty() {
        emit_message_error(json_output, "empty query");
    }
    match command {
        "compile" | "compile-ch" => {
            let target = if command == "compile" {
                lql::Target::DuckDB
            } else {
                lql::Target::ClickHouse
            };
            match lql::render_query(&input, target) {
                Ok(plan) => {
                    let mut value = serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
                    value["ok"] = json!(true);
                    emit_success(json_output, value, &plan.sql);
                }
                Err(bundle) => emit_diagnostics(json_output, &bundle, &input),
            }
        }
        "check" => match lql::analyze_source(&input, &lql::AnalysisOptions::duckdb()) {
            Ok(_) => println!("ok"),
            Err(bundle) => emit_diagnostics(json_output, &bundle, &input),
        },
        _ => unreachable!(),
    }
}

fn run_fields(json_output: bool) {
    let fields = lql::known_fields();
    if json_output {
        println!("{}", json!({"ok": true, "fields": fields}));
    } else {
        for field in fields {
            println!("{}", field);
        }
    }
}

fn run_query(args: &[String], json_output: bool) {
    let config =
        connection_config(args).unwrap_or_else(|error| emit_message_error(json_output, &error));
    let source = query_source(args).unwrap_or_else(|error| emit_message_error(json_output, &error));
    let params =
        query_parameters(args).unwrap_or_else(|error| emit_message_error(json_output, &error));
    let client = Client::new(config)
        .unwrap_or_else(|error| emit_message_error(json_output, &error.to_string()));
    let result = client
        .query(&source, params, query_limit(args))
        .unwrap_or_else(|error| emit_message_error(json_output, &error.to_string()));
    let format = output_format(args, json_output);
    print_result(&result, &format);
}
fn connection_config(args: &[String]) -> Result<ConnectionConfig, String> {
    let value = flag_value(args, "--dsn").or_else(|| {
        args.first()
            .filter(|value| !value.starts_with('-'))
            .cloned()
    });
    let endpoint = flag_value(args, "--endpoint");
    let collector = flag_value(args, "--collector");
    let database_connection = flag_value(args, "--database-connection");
    let api_key = flag_value(args, "--api-key-env").and_then(|name| std::env::var(name).ok());
    let username = flag_value(args, "--username");
    let password = flag_value(args, "--password-env").and_then(|name| std::env::var(name).ok());
    let env = flag_value(args, "--env");
    let service = flag_value(args, "--service");
    if flag_value(args, "--password").is_some() {
        return Err("plain --password is not supported; use --password-env".into());
    }
    if flag_value(args, "--api-key").is_some() {
        return Err("plain --api-key is not supported; use --api-key-env".into());
    }
    let dsn = value.or_else(|| std::env::var("LOZA_DSN").ok());
    if dsn.is_none() && endpoint.is_none() {
        return Err(
            "invalid LQL connection configuration: --dsn, LOZA_DSN, or --endpoint is required"
                .into(),
        );
    }
    Ok(ConnectionConfig {
        dsn,
        endpoint,
        collector,
        api_key,
        username,
        password,
        env,
        service,
        database_connection,
        ..Default::default()
    })
}

fn query_source(args: &[String]) -> Result<String, String> {
    if let Some(value) = flag_value(args, "--query") {
        return Ok(value);
    }
    let mut source = String::new();
    io::stdin()
        .read_to_string(&mut source)
        .map_err(|_| "failed to read query from stdin".to_string())?;
    let source = source.trim().to_string();
    if source.is_empty() {
        Err("query source is required".into())
    } else {
        Ok(source)
    }
}

fn query_parameters(args: &[String]) -> Result<HashMap<String, QueryValue>, String> {
    let mut result = HashMap::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--param" {
            let raw = args
                .get(index + 1)
                .ok_or_else(|| "--param requires name=value".to_string())?;
            let (name, value) = raw
                .split_once('=')
                .ok_or_else(|| "--param requires name=value".to_string())?;
            if name.is_empty() {
                return Err("--param name cannot be empty".into());
            }
            result.insert(name.to_string(), typed_value(value)?);
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(result)
}

fn typed_value(value: &str) -> Result<QueryValue, String> {
    let result = if value == "true" || value == "false" {
        QueryValue::try_new("bool", value == "true")
    } else if let Ok(number) = value.parse::<i64>() {
        QueryValue::try_new("int", number)
    } else if let Ok(number) = value.parse::<f64>() {
        QueryValue::try_new("float", number)
    } else {
        QueryValue::try_new("string", value)
    };
    result.map_err(|error| format!("parameter serialization failed: {error}"))
}

/// Point a connection config at a new DSN while preserving every other
/// setting (endpoint, credentials, env, service, timeouts, ...).
///
/// The shell's `\connect` command must not rebuild the config from
/// `Default`, otherwise explicitly supplied credentials and endpoints are
/// silently dropped and subsequent queries lose authentication.
fn apply_connect(config: &mut ConnectionConfig, dsn: &str) {
    config.dsn = Some(dsn.trim().to_string());
}

fn run_shell(args: &[String]) {
    let mut config =
        connection_config(args).unwrap_or_else(|error| emit_message_error(false, &error));
    let mut client = Client::new(config.clone())
        .unwrap_or_else(|error| emit_message_error(false, &error.to_string()));
    let mut timing = false;
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut format = "table".to_string();
    let mut pending = String::new();
    loop {
        print!("lql> ");
        let _ = io::stdout().flush();
        let Some(Ok(line)) = lines.next() else {
            break;
        };
        let command = line.trim();
        if is_quit_command(command) {
            break;
        }
        if pending.is_empty() && command.starts_with('\\') {
            match command.split_once(' ') {
                Some(("\\connect", dsn)) => {
                    apply_connect(&mut config, dsn);
                    config = reconnect(&config, dsn);
                    match Client::new(config.clone()) {
                        Ok(next) => {
                            client = next;
                            println!("connected");
                        }
                        Err(error) => eprintln!("{}", error),
                    }
                }
                Some(("\\database", name)) => {
                    config.database_connection = Some(name.trim().to_string());
                    match Client::new(config.clone()) {
                        Ok(next) => {
                            client = next;
                            println!("database connection selected");
                        }
                        Err(error) => eprintln!("{}", error),
                    }
                }
                Some(("\\timing", _)) => {
                    timing = !timing;
                    println!("timing {}", if timing { "on" } else { "off" });
                }
                Some(("\\help", _)) => {
                    println!("\\connect <dsn>  \\database <name>  \\fields  \\timing  \\format table|json|csv  \\q")
                }
                Some(("\\format", value)) if matches!(value.trim(), "table" | "json" | "csv") => {
                    format = value.trim().to_string()
                }
                None if matches!(command, "\\help" | "\\timing") => {
                    if command == "\\timing" {
                        timing = !timing;
                        println!("timing {}", if timing { "on" } else { "off" });
                    } else {
                        println!(
                            "\\connect <dsn>  \\fields  \\timing  \\format table|json|csv  \\q"
                        );
                    }
                }
                _ => eprintln!("unknown shell command"),
            }
            continue;
        }
        pending.push_str(&line);
        pending.push('\n');
        if command == ";" || line.trim_end().ends_with(';') {
            let source = pending.trim().trim_end_matches(';').trim().to_string();
            pending.clear();
            let started = std::time::Instant::now();
            match client.query(&source, HashMap::new(), 1000) {
                Ok(result) => {
                    print_result(&result, &format);
                    if timing {
                        eprintln!("({} ms)", started.elapsed().as_millis());
                    }
                }
                Err(error) => eprintln!("{}", error),
            }
        }
    }
}

/// Escape a single CSV field according to RFC 4180 and neutralize spreadsheet
/// formula injection.
///
/// A value starting with a formula trigger (`'`, `=`, `+`, `-`, `@`) is prefixed
/// with an apostrophe so spreadsheet applications treat it as literal text.
/// Fields containing a comma, quote or line break are wrapped in double quotes,
/// with any embedded quotes doubled.
fn csv_field(value: &str) -> String {
    let neutralized = match value.chars().next() {
        Some('\'' | '=' | '+' | '-' | '@') => format!("'{}", value),
        _ => value.to_string(),
    };
    if neutralized
        .chars()
        .any(|c| matches!(c, ',' | '"' | '\r' | '\n'))
    {
        format!("\"{}\"", neutralized.replace('"', "\"\""))
    } else {
        neutralized
    }
}

/// Write a query result as CSV, escaping every header and cell value.
fn write_csv<W: Write>(result: &lql_client::QueryResult, mut out: W) -> io::Result<()> {
    let header = result
        .columns
        .iter()
        .map(|column| csv_field(&column.name))
        .collect::<Vec<_>>()
        .join(",");
    writeln!(out, "{}", header)?;
    for row in &result.rows {
        let line = result
            .columns
            .iter()
            .map(|column| {
                let value = row
                    .get(&column.name)
                    .map(display_value)
                    .unwrap_or_default();
                csv_field(&value)
            })
            .collect::<Vec<_>>()
            .join(",");
        writeln!(out, "{}", line)?;
    }
    Ok(())
}

/// Rebuild the connection for a new DSN without discarding credentials or
/// other settings the user configured at startup.
fn reconnect(config: &ConnectionConfig, dsn: &str) -> ConnectionConfig {
    let mut next = config.clone();
    next.dsn = Some(dsn.trim().to_string());
    next
}

fn is_quit_command(command: &str) -> bool {
    matches!(command, "\\q" | "\\quit" | "\\exit")
}

fn csv_record(fields: &[String]) -> String {
    fields
        .iter()
        .map(|field| csv_field(field))
        .collect::<Vec<_>>()
        .join(",")
}

fn display_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn print_result(result: &lql_client::QueryResult, format: &str) {
    match format {
        "json" => println!(
            "{}",
            serde_json::to_string(result).unwrap_or_else(|_| "{}".into())
        ),
        "csv" => {
            if let Err(error) = write_csv(result, io::stdout().lock()) {
                eprintln!("error writing CSV output: {}", error);
            }
        }
        _ => {
            println!(
                "{}",
                result
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            for row in &result.rows {
                println!(
                    "{}",
                    result
                        .columns
                        .iter()
                        .map(|column| row
                            .get(&column.name)
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "null".into()))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
        }
    }
}

fn query_limit(args: &[String]) -> usize {
    flag_value(args, "--limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000)
}
fn output_format(args: &[String], json_output: bool) -> String {
    if json_output {
        "json".into()
    } else {
        flag_value(args, "--format").unwrap_or_else(|| "table".into())
    }
}
fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn emit_success(json_output: bool, value: serde_json::Value, text: &str) {
    if json_output {
        println!("{}", value);
    } else {
        println!("{}", text);
    }
}
fn emit_message_error(json_output: bool, message: &str) -> ! {
    if json_output {
        println!("{}", json!({"ok": false, "error": message}));
    } else {
        eprintln!("error: {}", message);
    }
    std::process::exit(1);
}
fn emit_diagnostics(json_output: bool, bundle: &lql::DiagnosticBundle, source: &str) -> ! {
    if json_output {
        println!(
            "{}",
            json!({"ok": false, "diagnostics": bundle.diagnostics})
        );
    } else {
        eprintln!("{}", bundle.render(source));
    }
    std::process::exit(1);
}

fn print_usage() {
    println!("Loza Query Language (LQL) compiler and live client\n\nUsage:\n  lql compile [--json] <query>\n  lql compile-ch [--json] <query>\n  lql check [--json] <query>\n  lql query --dsn <loza://...> --query <source> [--database-connection NAME] [--param name=value]\n  lql connect [<dsn>] [--database-connection NAME]\n  lql shell [<dsn>] [--database-connection NAME]\n  lql serve --stdio\n  lql fields [--json]");
}

#[cfg(test)]
mod tests {
    use super::*;
    use lql_client::QueryColumn;
    use serde_json::{json, Value};
    use std::collections::HashMap;

    fn query_result(names: &[&str], rows: Vec<HashMap<String, Value>>) -> lql_client::QueryResult {
        let row_count = rows.len();
        lql_client::QueryResult {
            columns: names
                .iter()
                .map(|name| QueryColumn {
                    name: (*name).to_string(),
                    value_type: "string".to_string(),
                    nullable: true,
                })
                .collect(),
            rows,
            duration_ms: 0,
            row_count,
        }
    }

    #[test]
    fn connect_updates_dsn_without_discarding_existing_settings() {
        let mut config = ConnectionConfig {
            dsn: Some("loza://old".into()),
            endpoint: Some("https://old.example.com".into()),
            collector: Some("old-collector".into()),
            api_key: Some("old-api-key".into()),
            username: Some("old-user".into()),
            password: Some("old-password".into()),
            env: Some("old-env".into()),
            service: Some("old-service".into()),
            database_connection: Some("old-database".into()),
            timeout: Some(std::time::Duration::from_secs(7)),
            max_response_bytes: Some(4096),
        };

        apply_connect(&mut config, "  loza://new  ");

        assert_eq!(config.dsn.as_deref(), Some("loza://new"));
        assert_eq!(config.endpoint.as_deref(), Some("https://old.example.com"));
        assert_eq!(config.collector.as_deref(), Some("old-collector"));
        assert_eq!(config.api_key.as_deref(), Some("old-api-key"));
        assert_eq!(config.username.as_deref(), Some("old-user"));
        assert_eq!(config.password.as_deref(), Some("old-password"));
        assert_eq!(config.env.as_deref(), Some("old-env"));
        assert_eq!(config.service.as_deref(), Some("old-service"));
        assert_eq!(config.database_connection.as_deref(), Some("old-database"));
        assert_eq!(config.timeout, Some(std::time::Duration::from_secs(7)));
        assert_eq!(config.max_response_bytes, Some(4096));
    }

    #[test]
    fn reconnect_only_changes_the_dsn() {
        let config = ConnectionConfig {
            dsn: Some("loza://old-host/collector".into()),
            endpoint: Some("http://old-host:9308".into()),
            collector: Some("collector".into()),
            api_key: Some("secret-key".into()),
            username: Some("user".into()),
            password: Some("pass".into()),
            env: Some("staging".into()),
            service: Some("billing".into()),
            database_connection: Some("warehouse".into()),
            timeout: Some(std::time::Duration::from_secs(7)),
            max_response_bytes: Some(4096),
        };

        let next = reconnect(&config, "  loza://new-host/other  ");

        assert_eq!(next.dsn.as_deref(), Some("loza://new-host/other"));
        assert_eq!(next.endpoint, config.endpoint);
        assert_eq!(next.collector, config.collector);
        assert_eq!(next.api_key, config.api_key);
        assert_eq!(next.username, config.username);
        assert_eq!(next.password, config.password);
        assert_eq!(next.env, config.env);
        assert_eq!(next.service, config.service);
        assert_eq!(next.database_connection, config.database_connection);
        assert_eq!(next.timeout, config.timeout);
        assert_eq!(next.max_response_bytes, config.max_response_bytes);
    }

    #[test]
    fn connect_leaves_unset_fields_unset_for_dsn_derived_credentials() {
        let mut config = ConnectionConfig {
            dsn: Some("loza://old".into()),
            database_connection: Some("selected".into()),
            ..Default::default()
        };

        apply_connect(&mut config, "loza://new");

        assert_eq!(config.dsn.as_deref(), Some("loza://new"));
        assert_eq!(config.endpoint, None);
        assert_eq!(config.api_key, None);
        assert_eq!(config.username, None);
        assert_eq!(config.password, None);
        assert_eq!(config.database_connection.as_deref(), Some("selected"));
    }

    #[test]
    fn csv_field_quotes_commas_quotes_and_newlines() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(csv_field("a\r\nb"), "\"a\r\nb\"");
    }

    #[test]
    fn csv_field_neutralizes_leading_formula_characters() {
        for value in ["'leading", "=1+1", "+cmd", "-42", "@SUM(A1)"] {
            assert_eq!(csv_field(value), format!("'{}", value));
        }
        // Neutralization and quoting compose for a value that also has a comma.
        assert_eq!(csv_field("=a,b"), "\"'=a,b\"");
    }

    #[test]
    fn write_csv_escapes_headers_and_rows() {
        let mut row = HashMap::new();
        row.insert("last,name".to_string(), json!("Doe, John"));
        row.insert("net".to_string(), json!(-5));
        let result = query_result(&["last,name", "net"], vec![row]);

        let mut out = Vec::new();
        write_csv(&result, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\"last,name\",net\n\"Doe, John\",'-5\n"
        );
        assert_eq!(csv_field("first\nsecond"), "\"first\nsecond\"");
        assert_eq!(csv_field("first\rsecond"), "\"first\rsecond\"");
    }

    #[test]
    fn csv_record_escapes_every_field() {
        let fields = vec![
            "id".to_string(),
            "a,b".to_string(),
            "line1\nline2".to_string(),
        ];
        assert_eq!(csv_record(&fields), "id,\"a,b\",\"line1\nline2\"");
    }

    #[test]
    fn quit_aliases_are_recognized() {
        for command in ["\\q", "\\quit", "\\exit"] {
            assert!(is_quit_command(command), "{command} should quit");
        }
        for command in ["\\help", "\\timing", "\\connect dsn", "select 1;"] {
            assert!(!is_quit_command(command), "{command} should not quit");
        }
    }
}
