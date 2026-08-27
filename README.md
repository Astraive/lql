# LOZA Query Language (LQL)

LQL is a small, typed-by-validation, Kusto-inspired query language that compiles to DuckDB, ClickHouse, and PostgreSQL SQL. It is designed for LOZA wide-event data and can run as a Rust library, a native CLI, or a WASM module.

## Current release

- Crate: `lql`
- Version: `0.5.1`
- Targets: DuckDB, ClickHouse, and PostgreSQL
- Inputs: pipeline queries beginning with `from`

## Quick start

```bash
cargo run --bin lql -- compile 'from events | where level = "error" | summarize count() by service | sort count desc | limit 10'
cargo run --bin lql -- compile-ch 'from events | where service contains "api" | limit 20'
cargo run --bin lql -- check 'from events | where duration_ms > 1000'
```

Library usage:

```rust
use lql::{compile_to_duckdb, compile_to_clickhouse, compile_to_postgres};

let query = r#"from events | where level = "error" | limit 10"#;
let duckdb_sql = compile_to_duckdb(query)?;
let clickhouse_sql = compile_to_clickhouse(query)?;
let postgres_sql = compile_to_postgres(query)?;
```

## Collector-managed database targets

The released client uses only `loza://` Collector DSNs for transport and
credentials. Pass `--database-connection NAME` to `query` or `shell` to select
a configured Collector connection. The selector is sent to the Collector; the
client never accepts or opens `postgres://`, `duckdb://`, or other direct
database DSNs. DuckDB is a server-side file path, while PostgreSQL and
ClickHouse use server-side host/port credentials.

## Development

```bash
cargo test --all-features
cargo run --bin lql -- --help
```

The enhancement roadmap and acceptance criteria are in [`PLAN.md`](PLAN.md).
