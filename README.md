# LOZA Query Language (LQL)

LQL is a small, typed-by-validation, Kusto-inspired query language that compiles to DuckDB and ClickHouse SQL. It is designed for LOZA wide-event data and can run as a Rust library, a native CLI, or a WASM module.

## Current release

- Crate: `lql`
- Version: `0.4.3`
- Targets: DuckDB and ClickHouse
- Inputs: pipeline queries beginning with `from`

## Quick start

```bash
cargo run --bin lql -- compile 'from events | where level = "error" | summarize count() by service | sort count desc | limit 10'
cargo run --bin lql -- compile-ch 'from events | where service contains "api" | limit 20'
cargo run --bin lql -- check 'from events | where duration_ms > 1000'
```

Library usage:

```rust
use lql::{compile_to_duckdb, compile_to_clickhouse};

let query = r#"from events | where level = "error" | limit 10"#;
let duckdb_sql = compile_to_duckdb(query)?;
let clickhouse_sql = compile_to_clickhouse(query)?;
# Ok::<(), lql::LqlError>(())
```

## Development

```bash
cargo test --all-features
cargo run --bin lql -- --help
```

The enhancement roadmap and acceptance criteria are in [`PLAN.md`](PLAN.md).
