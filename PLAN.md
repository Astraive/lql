# LQL 0.1 Product and Implementation Plan

## Product decision

LQL becomes LOZA's only public query language for event, trace, and incident
analytics. Users submit LQL; trusted LOZA services execute only a compiler-owned,
parameterized query plan. SQL remains a backend implementation detail.

LQL is a portable, typed, pipeline language. The compiler preserves the order of
pipeline stages, produces deterministic diagnostics and machine-readable analysis,
and lowers the same semantic plan to DuckDB or ClickHouse. It is a compiler and
toolchain, not a database, authorization engine, or arbitrary-SQL escape hatch.

## Starting point

The standalone Rust package currently parses pipeline queries and targets DuckDB
and ClickHouse. It has a Rust API, CLI, WASM bindings, source-schema validation,
JSON CLI envelopes, and these implemented stages:

- `from`, `where`, `summarize`, `sort`, `limit`/`take`, `distinct`, `project`,
  `extend`, `top`, and `timeseries`;
- scalar functions, aggregates, `between`, `in`, regex matching, and basic
  duration literals;
- a static default LOZA event schema.

The imported source still carries `0.3.0` package metadata. This roadmap names
the first standalone public LQL release `0.1.0`; release implementation will
align `Cargo.toml`, `lql.yaml`, artifacts, and compatibility fixtures at that
cutover. Planning work does not publish or relabel the current crate.

The existing enhancement checklist is obsolete: most listed syntax and JSON-CLI
work is already present. The remaining product gaps are semantic correctness and
complete platform contracts:

1. Both SQL backends flatten every stage into one final `SELECT`. A later
   `where`, `project`, `extend`, `distinct`, or `summarize` can therefore change
   the meaning of an earlier stage or reference an alias before it exists.
2. Validation is field-name based, rather than a typed, stage-aware analysis.
   Dynamic fields are accepted through special cases; aliases, nullability,
   aggregate context, and target capabilities are not modelled.
3. Tokens and AST nodes do not retain real spans. Most semantic errors report
   `0..0`, and diagnostics do not have stable codes, snippets, suggestions, or
   recovery.
4. Function metadata, validation, and backend SQL lowering are duplicated.
   That allows capability/arity/type drift and prevents a reliable target parity
   contract.
5. Strings are inlined into SQL. Compiler output must instead carry SQL
   placeholders and ordered bound values.
6. The schema is hard-coded and not an explicit LOZA schema contract. The
   Collector endpoint still accepts SQL rather than compiling trusted LQL
   server-side.
7. Tests primarily inspect fragments. They do not establish stage-order
   semantics, parameter binding, differential backend behavior, or an execution
   contract.

## 0.1 invariants

These are non-negotiable acceptance conditions for every milestone.

### One semantic source of truth

- Parsing produces a span-carrying syntax tree. Analysis resolves it into a
  typed relational IR; target emitters consume only that IR.
- A pipeline is left-to-right composition. Each stage receives exactly the
  relation emitted by its predecessor. A compiler may use nested subqueries or
  CTEs, but it must never reorder stages unless a proven-equivalent optimization
  records that rewrite.
- Identifier resolution is lexical and stage-aware. A `project` removes omitted
  fields; `extend` introduces an alias only after its stage; aggregate output
  replaces input fields; ambiguous names are errors.
- The same expression/function registry supplies parser recognition, signatures,
  type rules, documentation data, completion, capability checks, and backend
  lowering. No backend indexes arguments without first accepting a validated
  signature.

### Safe, portable execution

- Public compilation returns `CompiledQuery { sql, parameters, output_schema,
  required_capabilities, diagnostics, language_version }`. Literal values,
  including `limit`/`take` where supported by a target, are bind parameters;
  user-controlled text is never interpolated into SQL.
- Tables, columns, functions, and target dialect features are compiler-owned
  identifiers. A caller selects only allowed logical sources and fields through
  `Schema` and `QueryPolicy`.
- Compilation is deterministic for the same query, schema revision, target,
  policy, and language version. `now()` receives an explicit compilation clock
  in the analysis context so tests and saved-query replay are reproducible.
- SQL targets are capability profiles, not `if target` branches scattered
  through the AST. Unsupported but otherwise valid syntax fails before emission
  with a stable diagnostic.
- LQL itself has no arbitrary SQL stage, file/network functions, extension
  loading, DDL/DML, user-defined SQL functions, or implicit source discovery.

### Stable user and integration contracts

- Diagnostics have a stable `LQLxxxx` code, severity, UTF-8 byte span, line and
  column, labelled source snippet, explanatory note, and optional fix. JSON,
  WASM, Rust, CLI, and service adapters expose the same structured diagnostic.
- The public API distinguishes `parse`, `analyze`, `compile`, `format`,
  `complete`, and `explain`; `compile_to_duckdb` and `compile_to_clickhouse`
  remain compatibility wrappers that render the parameterized plan for trusted
  tooling only.
- Every API accepts `CompileOptions { target, schema, policy, language_version,
  clock }`. The 0.1 release defines the first public default language version.
- The LQL grammar, semantic behavior, built-in catalog, output schema, and
  diagnostics are versioned fixtures. Breaking grammar or semantic changes need
  a language-version gate and migration diagnostic.

## Language model

### Sources and schemas

`from` accepts a logical source declared in the supplied schema: `events`,
`traces`, `incidents`, and explicitly registered derived views. A source has:

- a stable logical name, physical target mapping, revision, and row identity;
- named typed columns, nullability, documentation, sensitivity tag, and
  pushdown/index metadata;
- declared structured columns (for example `attrs`, `resource`, `http`) whose
  paths are resolved through the schema rather than a permissive root-name list.

Dynamic event attributes remain useful, but must be explicit: `attrs["region"]`
or `get(attrs, "region")` has type `dynamic?`. Queries must cast it before
numeric, time, or boolean operations. This removes silent string comparisons
while preserving wide-event exploration.

### Types

The analyzer uses `bool`, `int`, `float`, `decimal`, `string`, `timestamp`,
`duration`, `json`, `array<T>`, `object`, `dynamic`, and `null`. Every type can
be nullable. Numeric promotion is explicit and deterministic; comparisons,
operators, aggregates, and functions publish their accepted and returned types.
SQL three-valued logic is exposed deliberately: `is_null`, `is_not_null`, and
`coalesce` are the portable ways to control null behavior.

### Core pipeline

The 0.1 core supports the existing source-compatible syntax plus the following
complete forms:

```lql
from events
| where timestamp >= ago(1h) and level in ("warn", "error")
| extend latency_s = duration_ms / 1000.0
| project timestamp, service, latency_s, status = http.status_code
| summarize errors = countif(level = "error"), p95(latency_s) by service
| sort by errors desc, service asc nulls last
| take 20
```

- `where` supports Boolean expressions, `in`, `between`, regex, null predicates,
  and explicit casts.
- `project` supports `name = expression`, `project-away`, and `project-rename`;
  it has exact output-column semantics.
- `extend` supports multiple ordered assignments and detects duplicate/shadowed
  aliases.
- `sort by` accepts multiple keys and explicit null placement. `top N by` is
  exactly `sort` followed by `take` in the semantic IR.
- `distinct` is a projection-plus-deduplication stage, not a mutable flag on the
  final `SELECT`.
- `summarize` supports `count`, `countif`, `sum`, `sumif`, `avg`, `min`, `max`,
  `dcount`, `percentile`, `arg_min`, `arg_max`, `first`, `last`, and histogram
  aggregates. Every aggregate has deterministic empty-input and null semantics.
- `let name = expression` and typed external parameters (`$start`, `$service`)
  support reusable saved queries without string substitution.

### Telemetry analytics

LOZA's primary use case requires first-class time analytics:

- `bin(timestamp, 1m)`, `ago`, `now`, `date_part`, timezone-aware parsing and
  formatting;
- `make-series`/`timeseries` with interval, explicit range, grouping, default
  fill policy, and ordered time buckets;
- `rate`, `delta`, moving average, cumulative sum, and percentile/histogram
  output as explicitly typed analytic operators;
- trace-oriented helpers such as `trace_duration`, `is_error`, and carefully
  defined status/outcome normalization only when their behavior is shared by
  the LOZA schema specification.

No helper is added because one target happens to provide it. Each must have a
target-neutral contract, a target capability entry, and execution fixtures.

### Relational and analytic composition

After the core is stable, 0.1 adds the minimum relational surface needed to
correlate LOZA data without exporting raw SQL:

- `union`/`union all` with schema alignment and explicit missing-column nulls;
- `join kind=inner|left|semi|anti (...) on key[, key...]`, with aliases,
  collision rules, and join-key type checks;
- window expressions with explicit `partition by`, `order by`, and frame:
  `row_number`, `rank`, `lag`, `lead`, running aggregates, and moving windows;
- named query definitions that form a DAG, reject cycles, and can be lowered to
  CTEs.

Cross joins, implicit joins, recursive queries, arbitrary subqueries, and
unbounded Cartesian expansion are excluded. The analyzer estimates cardinality
from source metadata and rejects or requires an explicit policy capability for
unsafe joins.

### Tooling surface

- `lql check`, `compile --target`, `format`, `ast`, `explain`, `fields`, and
  `functions` have documented text and JSON envelopes.
- `lql format --check` uses a canonical formatter, making saved queries and
  code review stable.
- `lql complete --position` returns schema-aware fields, aliases, functions,
  snippets, expected tokens, and deprecation notices.
- WASM exposes structured parse/analyze/compile/complete functions and returns
  serializable data; it never silently converts errors to strings.
- Explain returns logical plan, output schema, target capability decisions, and
  redacted parameter type information. It does not expose a database execution
  plan or sensitive values.

## Implementation architecture

### 1. Replace the token-only front end

Refactor `src/lexer.rs`, `src/parser.rs`, `src/ast.rs`, and `src/error.rs` into:

1. source text plus `Span`/`LineIndex`;
2. `Token { kind, span }`;
3. a concrete syntax tree that can recover at pipeline boundaries;
4. a typed public AST for integrations; and
5. `Diagnostic` / `DiagnosticCode` with renderer and serde representation.

The parser must have no sentinel token behavior and must make forward progress
on every recovery path. Parse recovery produces multiple diagnostics for editor
use; strict compile rejects any error severity diagnostic.

### 2. Introduce semantic analysis and relational IR

Add `src/analyze/` for scope construction, schema resolution, type inference,
function overload selection, nullability, aggregate/window context, stage
legality, policy checks, and output-schema calculation. Add `src/ir/` for
source, filter, map, aggregate, deduplicate, sort, limit, series, union, join,
window, and named-relation nodes.

The current `validate` module becomes a compatibility facade over analysis.
Unknown fields, untyped dynamic paths, use-after-project, aggregate misuse,
ambiguous identifiers, aliases in the same stage, and unavailable capabilities
must fail here before SQL generation.

### 3. Centralize catalog and schema contracts

Replace `src/functions.rs` boolean/arity checks with declarative function and
aggregate definitions: namespace, aliases, overloads, null behavior, volatility,
required capability, evaluator fixture, and DuckDB/ClickHouse lowering template.
The parser recognizes normal identifiers; catalog lookup classifies functions,
which prevents keyword conflicts and keeps one catalog authoritative.

Evolve `src/schema.rs` into serde-compatible schema documents and providers.
Ship a versioned LOZA schema fixture generated from the canonical LOZA event
contract. Do not independently hand-maintain duplicate field lists in LQL and
LOZA. Support caller-provided schemas for views, tests, and controlled custom
tables.

### 4. Emit prepared target plans

Create `src/compile/{duckdb,clickhouse}.rs`, consuming only analyzed IR.
Emit nested relations/CTEs to preserve stage order, then render a
`CompiledQuery` with positional placeholders and `QueryValue` bindings.
Quoting occurs only for schema-derived identifiers. Use one target capability
table and one SQL literal/parameter renderer per dialect.

Keep `compile_to_duckdb` and `compile_to_clickhouse` as legacy convenience APIs
through the 0.1 release, but document that integrations consume `CompiledQuery`.
Remove duplicate compiler logic only after differential tests prove semantic
parity.

### 5. Define the LOZA trust boundary

LQL must be compiled server-side before Collector execution. The Collector
`POST /lql/query` contract accepts `{ query, parameters, target? }`, applies a
server-owned `Schema` and `QueryPolicy`, receives a prepared plan from LQL, and
executes `sql` with `parameters` through the database driver. It does not accept
raw SQL at that route.

The integration adapter is a persistent, versioned JSON-RPC compiler process
initially: it avoids cgo and makes the standalone Rust package usable by Go,
containers, and local installations. The protocol exposes `compile`, `check`,
`complete`, and `version`; it includes schema/policy revision and returns only
structured success or diagnostics. A later FFI adapter is allowed only when it
passes the same protocol contract and packaging tests. Frontend WASM is for
editor feedback, never authorization or server trust.

`QueryPolicy` is caller-owned and has source allowlists, sensitive-field rules,
maximum result rows, maximum joins, time-range requirement, target capability
allowlist, and parameter-size/query-size limits. It is enforced in analysis,
not by string filtering emitted SQL.

## Delivery sequence

### Milestone 0 — freeze and specify (0.1.0-alpha.1)

- Replace this plan's old feature checklist with a versioned grammar,
  semantic-rules, function-catalog, schema, policy, and JSON-protocol
  specification under `docs/`.
- Create a compatibility corpus from every current public example and test.
  Record accepted syntax, AST/diagnostic behavior, and DuckDB/ClickHouse result
  expectations before refactoring.
- Establish public semver policy: the existing syntax is the `0.1` language
  baseline; any later incompatible grammar or semantic change requires a
  language-version gate and migration diagnostic.

**Exit:** specification review accepted; legacy corpus is executable; no behavior
is changed.

### Milestone 1 — diagnostics and semantic core (0.1.0-alpha.2)

- Implement spans, structured diagnostics, error recovery, formatter, and JSON
  serialization.
- Implement schema documents, type/nullability checking, catalog overload
  resolution, lexical stage scopes, and source/policy validation.
- Convert current parser and `validate` callers to the new analysis API while
  retaining source-compatible wrappers.

**Exit:** every parse/analysis failure has a stable code and real source span;
the legacy corpus is accepted or reports an explicit migration diagnostic; no
malformed user query panics.

### Milestone 2 — stage-correct prepared compilation (0.1.0-beta.1)

- Add relational IR and rewrite DuckDB lowering to use nested relations/CTEs.
- Replace inline literals with parameters and expose `CompiledQuery`.
- Port ClickHouse from the same IR and capability catalog.
- Add explain output and target capability diagnostics.

**Exit:** stage-order, alias lifetime, projection, aggregate, and parameter tests
execute correctly in DuckDB; every supported corpus query has equivalent
DuckDB/ClickHouse results or a documented capability rejection.

### Milestone 3 — telemetry analytics and query UX (0.1.0-beta.2)

- Complete multi-key sort, projection variants, aggregate catalog, typed
  parameters, time-series/range/fill semantics, and portable telemetry helpers.
- Ship formatter, completion, function/field metadata, CLI JSON protocol, and
  matching WASM data structures.
- Add bounded query complexity metrics to analysis for policy decisions.

**Exit:** Lozana can format, validate, complete, explain, and compile saved
queries without bespoke client SQL code; all public operations share diagnostic
and schema contracts.

### Milestone 4 — relational correlation (0.1.0-beta.3)

- Implement named relations, union, controlled joins, and windows in IR and both
  targets.
- Add semantic cardinality/policy enforcement and result-schema propagation.

**Exit:** event/trace/incident correlation fixtures execute identically on both
supported targets; disallowed joins fail at analysis, never at database runtime.

### Milestone 5 — LOZA server integration (0.1.0-rc.1)

- Ship the persistent compiler JSON-RPC adapter, container packaging, lifecycle,
  version negotiation, health checks, and observability contract.
- Change the Collector LQL endpoint to accept only LQL plus typed parameters,
  execute prepared SQL on a guarded connection, and return redacted structured
  errors and result schema.
- Migrate CLI and Lozana to the shared protocol; remove frontend-only
  compilation as an authority and remove SQL pass-through from `/lql/query`.
- Publish migration instructions and compatibility telemetry for any deprecated
  pre-0.1 syntax.

**Exit:** end-to-end LOZA tests prove an authenticated LQL request reaches
DuckDB through a prepared plan; raw SQL, unregistered sources, dangerous
functions, oversized parameters, and policy violations are rejected.

### Milestone 6 — 0.1 release gate

- Make 0.1 the default language version after a documented migration period.
- Freeze the public grammar, schema-document format, protocol envelope,
  diagnostic codes, and prepared-plan contract under semver.
- Publish crate, CLI binaries, WASM package, container, changelog, and signed
  compatibility fixtures.

**Exit:** release artifacts compile and execute the same conformance corpus;
LOZA integration is the supported production route; no raw SQL compatibility
path remains at the LQL endpoint.

## Verification matrix

Every milestone expands, never replaces, this matrix:

| Layer | Required evidence |
| --- | --- |
| Lexer/parser | unit, malformed-input recovery, corpus AST snapshots, `cargo fuzz`/property tests with no panic |
| Analyzer | type, scope, alias, null, aggregate/window, schema revision, and policy boundary tests |
| Compiler | golden prepared plans plus identifier/parameter injection tests for each target |
| Semantics | DuckDB execution fixtures; ClickHouse container differential fixtures for every portable feature |
| Compatibility | 0.1 query corpus and stable diagnostic/JSON protocol snapshots |
| CLI/WASM | JSON schema tests, stdin/stdout behavior, formatting idempotence, browser/WASM smoke |
| Adapter | JSON-RPC framing, version/schema negotiation, process restart, timeout, and malformed-request tests |
| Collector | authenticated endpoint, prepared binding, policy refusal, raw-SQL refusal, row cap, timeout, and redaction integration tests |
| Release | `cargo fmt --check`, Clippy with warnings denied, all tests/doctests, release build, WASM build, benchmark regression budget |

Target parity is semantic, not textual SQL equality. Fixtures insert the same
typed rows in DuckDB and ClickHouse, execute each supported LQL query, normalize
ordering/types according to the declared query semantics, and compare values.
Target-specific features are tested as explicit capability errors on the other
target.

## Non-goals and guardrails

- No arbitrary SQL, DDL/DML, database administration, user-defined SQL, or
  source discovery.
- No hidden type coercion, silent dynamic-field stringification, or fallback
  compilation after an unsupported feature.
- No query execution engine, storage layer, authentication, or authorization in
  the LQL crate.
- No joins or windows until the typed IR, prepared output, and target
  differential harness exist.
- No server integration that shells out once per query, accepts browser-produced
  SQL as trusted input, or duplicates the grammar in Go/TypeScript.
- Performance work follows correctness: benchmark parse, analyze, compile, and
  formatter separately; publish regression thresholds only after stable
  semantics.
