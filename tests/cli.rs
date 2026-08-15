use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lql"))
        .args(args)
        .output()
        .expect("LQL CLI should start")
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
