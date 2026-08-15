use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lql"))
        .args(args)
        .output()
        .expect("LQL CLI should start")
}

#[test]
fn compile_json_returns_target_and_sql() {
    let output = run(&[
        "compile",
        "--json",
        r#"from events | where level = "error" | take 2"#,
    ]);
    assert!(output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["target"], "duckdb");
    assert!(body["sql"].as_str().unwrap().contains("LIMIT 2"));
}

#[test]
fn invalid_json_query_returns_structured_error() {
    let output = run(&["check", "--json", "from events | where missing = 1"]);
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("unknown field"));
}
