use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lql"))
        .args(args)
        .output()
        .expect("LQL CLI should start")
}

/// Minimal collector that answers one request with `body` and returns the raw
/// request so tests can inspect headers such as `Authorization`.
fn spawn_collector(body: String) -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        for _ in 0..4 {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    request.extend_from_slice(&chunk[..n]);
                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (port, rx)
}

fn result_payload(name: &str, note: &str) -> String {
    serde_json::json!({
        "columns": [
            {"name": "name", "type": "string", "nullable": false},
            {"name": "note", "type": "string", "nullable": false}
        ],
        "rows": [{"name": name, "note": note}],
        "duration_ms": 0,
        "row_count": 1
    })
    .to_string()
}

#[test]
fn compile_json_returns_parameterized_plan() {
    let output = run(&[
        "compile",
        "--json",
        r#"from events | where level = "error" | take 2"#,
    ]);
    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["target"], "duckdb");
    assert!(body["sql"].as_str().unwrap().contains("LIMIT ?"));
    assert_eq!(body["parameters"][0]["value"], "error");
    assert_eq!(body["parameters"][1]["value"], 2);
}

#[test]
fn query_requires_connection_configuration() {
    let output = run(&["query", "--query", "from events"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid LQL connection configuration"));
    assert!(!stderr.contains("password"));
}

#[test]
fn query_rejects_plain_api_key() {
    let output = run(&[
        "query",
        "--endpoint",
        "https://remote.example",
        "--collector",
        "demo",
        "--api-key",
        "secret",
        "--query",
        "from events",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("plain --api-key is not supported"),
        "{}",
        stderr
    );
    assert!(stderr.contains("--api-key-env"), "{}", stderr);
}

#[test]
fn invalid_json_query_returns_structured_diagnostics() {
    let output = run(&["check", "--json", "from events | where missing = 1"]);
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("unknown field"));
    assert_eq!(body["diagnostics"][0]["code"], "LQL102");
}

#[test]
fn cli_uses_canonical_schema_consistent_with_rpc_and_wasm() {
    // `host` exists only in the legacy `duckdb_default` schema, so every entry
    // point must now reject it. CLI `compile` goes through `render_query`.
    let output = run(&["compile", "--json", r#"from events | where host = "prod""#]);
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("unknown field"));

    // `delivery_attempts` exists only in the revisioned LOZA schema and must
    // be accepted by every entry point.
    let output = run(&[
        "compile",
        "--json",
        "from events | project delivery_attempts",
    ]);
    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
}

#[test]
fn shell_quit_exits_without_running_following_commands() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lql"))
        .args(["shell", "loza://127.0.0.1:1/events"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("LQL CLI should start");
    {
        let stdin = child.stdin.as_mut().unwrap();
        write!(stdin, "\\q\n\\timing\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stdout.contains("timing on"), "\\q must end the shell loop");
    assert!(
        !stderr.contains("unknown shell command"),
        "\\q is a documented command"
    );
}

#[test]
fn shell_connect_preserves_api_key_across_reconnect() {
    let (port, requests) =
        spawn_collector(result_payload("ok", "done"));
    let first = format!(
        "loza://user:pass@127.0.0.1:{port}/first",
    );
    let second = format!(
        "loza://127.0.0.1:{port}/second",
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_lql"))
        .args([
            "shell",
            &first,
            "--api-key-env",
            "LQL_TEST_API_KEY",
        ])
        .env("LQL_TEST_API_KEY", "secret-key-123")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("LQL CLI should start");
    {
        let stdin = child.stdin.as_mut().unwrap();
        write!(stdin, "\\connect {second}\nfrom events;\n\\q\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("connected"));

    let request = requests
        .recv_timeout(Duration::from_secs(5))
        .expect("collector should receive the query");
    assert!(
        request.contains("/collectors/second/lql/query"),
        "query should target the reconnected collector: {request}"
    );
    assert!(
        request.contains("Bearer secret-key-123"),
        "api_key must survive \\connect: {request}"
    );
}

#[test]
fn query_csv_quotes_and_escapes_fields() {
    let (port, _) =
        spawn_collector(result_payload("a,b", "say \"hi\"\nnext"));
    let dsn = format!(
        "loza://127.0.0.1:{port}/events",
    );
    let output = run(&[
        "query",
        "--dsn",
        &dsn,
        "--query",
        "from events",
        "--format",
        "csv",
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("name,note"));
    assert!(
        stdout.contains("\"a,b\",\"say \"\"hi\"\"\nnext\"\n"),
        "CSV must quote commas and double embedded quotes/newlines: {stdout:?}"
    );
}
