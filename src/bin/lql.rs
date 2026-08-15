use std::io::{self, Read};

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
        "compile" | "compile-ch" | "check" => {
            let query_args: Vec<&str> = args
                .iter()
                .skip(command_index + 1)
                .filter(|arg| arg.as_str() != "--json")
                .map(String::as_str)
                .collect();
            let query = if query_args.is_empty() {
                let mut buf = String::new();
                if let Err(err) = io::stdin().read_to_string(&mut buf) {
                    emit_error(json_output, &format!("failed to read stdin: {}", err));
                }
                buf
            } else {
                query_args.join(" ")
            };

            let query = query.trim();
            if query.is_empty() {
                emit_error(json_output, "empty query");
            }

            match command {
                "compile" => match lql::compile_to_duckdb(query) {
                    Ok(sql) => emit_success(
                        json_output,
                        json!({"ok": true, "target": "duckdb", "sql": sql}),
                        &sql,
                    ),
                    Err(err) => emit_error(json_output, &err.to_string()),
                },
                "compile-ch" => match lql::compile_to_clickhouse(query) {
                    Ok(sql) => emit_success(
                        json_output,
                        json!({"ok": true, "target": "clickhouse", "sql": sql}),
                        &sql,
                    ),
                    Err(err) => emit_error(json_output, &err.to_string()),
                },
                "check" => match lql::validate_query(query) {
                    Ok(()) if json_output => println!("{}", json!({"ok": true, "valid": true})),
                    Ok(()) => println!("ok"),
                    Err(err) => emit_error(json_output, &err.to_string()),
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
        _ => emit_error(json_output, &format!("unknown command: {}", command)),
    }
}

fn emit_success(json_output: bool, value: serde_json::Value, text: &str) {
    if json_output {
        println!("{}", value);
    } else {
        println!("{}", text);
    }
}

fn emit_error(json_output: bool, message: &str) -> ! {
    if json_output {
        println!("{}", json!({"ok": false, "error": message}));
    } else {
        eprintln!("error: {}", message);
    }
    std::process::exit(1);
}

fn print_usage() {
    println!("Loza Query Language (LQL) compiler");
    println!();
    println!("Usage:");
    println!("  lql compile [--json] <query>       Compile LQL to DuckDB SQL");
    println!("  lql compile-ch [--json] <query>    Compile LQL to ClickHouse SQL");
    println!("  lql check [--json] <query>         Validate LQL syntax and fields");
    println!("  lql fields [--json]                List known event fields");
    println!();
    println!("Examples:");
    println!("  lql compile 'from events | where level = \"error\" | limit 10'");
    println!("  echo 'from events | limit 5' | lql compile");
}
