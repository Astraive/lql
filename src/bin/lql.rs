use std::io::{self, BufReader, Read};

use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_output = args.iter().skip(1).any(|arg| arg == "--json");
    let command_index = args
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, arg)| arg.as_str() != "--json")
        .map(|(index, _)| index);
    let Some(command_index) = command_index else {
        print_usage();
        std::process::exit(1);
    };
    let command = args[command_index].as_str();

    match command {
        "serve" => {
            if args.get(command_index + 1).map(String::as_str) != Some("--stdio") {
                emit_message_error(false, "serve requires --stdio");
            }
            if let Err(error) =
                lql::protocol::serve_stdio(BufReader::new(io::stdin()), io::stdout())
            {
                eprintln!("protocol error: {}", error);
                std::process::exit(1);
            }
        }
        "compile" | "compile-ch" | "check" => {
            let query_args: Vec<&str> = args
                .iter()
                .skip(command_index + 1)
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
                            let mut value =
                                serde_json::to_value(&plan).unwrap_or_else(|_| json!({}));
                            value["ok"] = json!(true);
                            emit_success(json_output, value, &plan.sql)
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
        "fields" => {
            let fields = lql::known_fields();
            if json_output {
                println!("{}", json!({"ok": true, "fields": fields}));
            } else {
                for field in fields {
                    println!("{}", field);
                }
            }
        }
        "--help" | "-h" => print_usage(),
        "--version" | "-v" => println!("lql {}", env!("CARGO_PKG_VERSION")),
        _ => emit_message_error(json_output, &format!("unknown command: {}", command)),
    }
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
        println!(
            "{}",
            json!({"ok": false, "diagnostics": [{"code": "LQL000", "severity": "error", "message": message, "primary_span": null, "labels": []}]})
        );
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
    println!("Loza Query Language (LQL) compiler");
    println!();
    println!("Usage:");
    println!("  lql compile [--json] <query>       Render a parameterized DuckDB plan");
    println!("  lql compile-ch [--json] <query>    Render a parameterized ClickHouse plan");
    println!("  lql check [--json] <query>         Validate LQL syntax and fields");
    println!("  lql serve --stdio                 Run the JSON-RPC compiler adapter");
    println!("  lql fields [--json]                List known event fields");
    println!();
    println!("Examples:");
    println!("  lql compile --json 'from events | where level = \"error\" | limit 10'");
    println!("  echo 'from events | limit 5' | lql compile");
}
