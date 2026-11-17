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

The `compile_to_*` wrappers and `compile` use the permissive
`QueryPolicy::default()` and are intended for trusted input. For untrusted
queries, use `compile_with_policy` (or `render_query_with_options`) with an
explicit `QueryPolicy` and scope so that source/field allowlists, sensitive
fields, row/parameter limits, and scope filters are enforced before SQL is
produced.

## Collector-managed database targets

The released client uses only `loza://` Collector DSNs for transport and
credentials. Pass `--database-connection NAME` to `query` or `shell` to select
a configured Collector connection. The selector is sent to the Collector; the
client never accepts or opens `postgres://`, `duckdb://`, or other direct
database DSNs. DuckDB is a server-side file path, while PostgreSQL and
ClickHouse use server-side host/port credentials.

## Security model: trusted scope

Queries that carry a trusted `scope` (via `render_query_with_context` /
`render_query_with_options`, or the `compile` RPC) have their `collector` and
`environment` predicates injected as a `where` filter **immediately after the
source stage** — before any user `where`, `summarize`, `project`, `extend`,
`distinct`, `sort` or `top`. The scope values are always bound as SQL
parameters, never inlined into the query text.

Applying the filter at the source means every later stage only ever sees rows
inside the caller's tenant/environment, and the scoped columns need not survive
aggregation or projection. Scope enforcement therefore cannot be bypassed by
reshaping the pipeline, and cannot fail just because a projection drops the
`collector` or `environment` column.

## Development

```bash
cargo test --all-features
cargo run --bin lql -- --help
```

The enhancement roadmap and acceptance criteria are in [`PLAN.md`](PLAN.md).
