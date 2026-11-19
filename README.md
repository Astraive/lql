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
use lql::{render_query, Target};

let query = r#"from events | where level = "error" | limit 10"#;
let plan = render_query(query, Target::DuckDB)?;
// `plan.sql` contains bind placeholders and `plan.parameters` holds the values.
// Pass both to your driver so values are bound, never string-interpolated.
```

The `compile_to_*` wrappers and `compile` use the permissive
`QueryPolicy::default()` and are intended for trusted input. For untrusted
queries, use `compile_with_policy` (or `render_query_with_options`) with an
explicit `QueryPolicy` and scope so that source/field allowlists, sensitive
fields, row/parameter limits, and scope filters are enforced before SQL is
produced.
> [!WARNING]
> The legacy `compile_to_duckdb`, `compile_to_clickhouse`, and `compile_to_postgres`
> wrappers inline filter/literal values directly into the SQL string. They now
> escape string literals per dialect (including ClickHouse backslash escaping)
> and perform placeholder substitution in a single pass, but they remain
> **trusted-tooling-only** helpers best suited to static queries. When any part
> of a query comes from an untrusted source, use `render_query` /
> `render_query_with_parameters` and bind the returned parameters through your
> database driver instead.

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
## Parser recursion limit

Deeply nested parentheses and function calls are bounded by a parser nesting
budget so that a malicious query returns a diagnostic instead of exhausting the
stack. The default is `128` nested expressions
(`lql::DEFAULT_MAX_DEPTH`); exceeding it produces diagnostic `LQL004`
("expression nesting depth exceeds the configured limit").

The budget is configurable through the library API:

```rust
use lql::{parse, parse_with_max_depth, DEFAULT_MAX_DEPTH};

let query = r#"from events | where (((level = "error")))"#;
let pipeline = parse(query)?;                       // uses DEFAULT_MAX_DEPTH
let pipeline = parse_with_max_depth(query, 64)?;    // custom budget
```

`lql::parser::Parser::with_max_depth` provides the same control at the token
level, and `lql::parse_diagnostics_with_max_depth` returns the limit violation
as a structured diagnostic. All CLI, RPC, and WASM entry points use the safe
default.

## Development

```bash
cargo test --all-features
cargo run --bin lql -- --help
```

The enhancement roadmap and acceptance criteria are in [`PLAN.md`](PLAN.md).
