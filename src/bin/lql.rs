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
    let api_key = flag_value(args, "--api-key")
        .or_else(|| flag_value(args, "--api-key-env").and_then(|name| std::env::var(name).ok()));
    let username = flag_value(args, "--username");
    let password = flag_value(args, "--password-env").and_then(|name| std::env::var(name).ok());
    let env = flag_value(args, "--env");
    let service = flag_value(args, "--service");
    if flag_value(args, "--password").is_some() {
        return Err("plain --password is not supported; use --password-env".into());
    }
    if value.is_none() && endpoint.is_none() && std::env::var("LOZA_DSN").is_err() {
        return Err(
            "invalid LQL connection configuration: --dsn, LOZA_DSN, or --endpoint is required"
                .into(),
        );
    }
    Ok(ConnectionConfig {
        dsn: value,
        endpoint,
        collector,
        api_key,
        username,
        password,
        env,
        service,
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
            result.insert(name.to_string(), typed_value(value));
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(result)
}

fn typed_value(value: &str) -> QueryValue {
    if value == "true" || value == "false" {
        QueryValue::new("bool", value == "true")
    } else if let Ok(number) = value.parse::<i64>() {
        QueryValue::new("int", number)
    } else if let Ok(number) = value.parse::<f64>() {
        QueryValue::new("float", number)
    } else {
        QueryValue::new("string", value)
    }
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
        if pending.is_empty() && command.starts_with('\\') {
            match command.split_once(' ') {
                Some(("\\connect", dsn)) => {
                    config = ConnectionConfig {
                        dsn: Some(dsn.trim().to_string()),
                        ..Default::default()
                    };
                    match Client::new(config.clone()) {
                        Ok(next) => {
                            client = next;
                            println!("connected");
                        }
                        Err(error) => eprintln!("{}", error),
                    }
                }
                Some(("\\timing", _)) => {
                    timing = !timing;
                    println!("timing {}", if timing { "on" } else { "off" });
                }
                Some(("\\help", _)) => {
                    println!("\\connect <dsn>  \\fields  \\timing  \\format table|json|csv  \\q")
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

fn print_result(result: &lql_client::QueryResult, format: &str) {
    match format {
        "json" => println!(
            "{}",
            serde_json::to_string(result).unwrap_or_else(|_| "{}".into())
        ),
        "csv" => {
            println!(
                "{}",
                result
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
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
                            .unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(",")
                );
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
    println!("Loza Query Language (LQL) compiler and live client\n\nUsage:\n  lql compile [--json] <query>\n  lql compile-ch [--json] <query>\n  lql check [--json] <query>\n  lql query --dsn <loza://...> --query <source> [--param name=value]\n  lql connect [<dsn>]\n  lql shell [<dsn>]\n  lql serve --stdio\n  lql fields [--json]");
}
