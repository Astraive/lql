# LQL Enhancement Plan

## Goal

Make LQL a dependable standalone query compiler for production LOZA dashboards, CLI usage, and WASM consumers. The work keeps the existing pipeline syntax source-compatible while adding high-value query expressiveness, deterministic diagnostics, target parity, and end-to-end confidence.

## Baseline

The imported 0.3.0 module already supports `from`, `where`, `summarize`, `sort`, `limit`, `project`, `extend`, `top`, `timeseries`, scalar functions, DuckDB output, ClickHouse output, schema validation, and WASM bindings.

Observed gaps:

- `compile()` does not validate field references before generating SQL.
- `distinct`, `take`, `between`, `not in`, and regex matching are unavailable.
- Function names are registered separately from compiler arity checks; malformed calls can panic through unchecked argument indexing.
- ClickHouse and DuckDB function/operator behavior is not covered by a shared parity matrix.
- CLI output is human-only, which makes editor and automation integration harder.
- Diagnostics expose token positions but not source spans/snippets suitable for editor errors.
- Existing tests mostly assert SQL fragments rather than executing a complete query against representative data.

## Scope and acceptance criteria

### 1. Standalone product surface

- Repository is independently buildable with Cargo.
- README documents library, CLI, WASM, development, and compatibility surfaces.
- No dependency on the former monorepo layout remains.

### 2. Language enhancements

- Add `take N` as a source-compatible alias for `limit N`.
- Add `distinct expr[, expr...]` as a pipeline statement.
- Add `between` and `not between` comparisons with inclusive bounds.
- Add `not in (...)` and `matches`/`not matches` regex comparisons.
- Preserve operator precedence and parenthesized expressions.
- Compile every new feature to both DuckDB and ClickHouse SQL.

### 3. Safety and diagnostics

- Public compile helpers validate the parsed pipeline against the selected target schema before SQL generation.
- Unknown functions and invalid arity return `LqlError`, never panic.
- Diagnostics include the original query position and actionable expected text.
- CLI supports `--json` for compile/check errors and successful output envelopes.

### 4. Verification

- Unit tests cover lexer, parser, validation, compiler, and function arity boundaries.
- Cross-target contract tests assert equivalent semantics, not incidental whitespace.
- End-to-end tests compile representative queries through the CLI, validate generated SQL, and execute DuckDB SQL against fixture data when DuckDB is available.
- Full Rust test, doctest, build, CLI smoke, and WASM compile checks pass.

## Implementation order

1. Extend AST/tokens/parser with the new statements and comparison forms.
2. Add validation and safe function-arity checks before changing SQL output.
3. Add DuckDB and ClickHouse compiler branches plus target parity tests.
4. Add CLI JSON envelopes and error diagnostics.
5. Add fixtures and end-to-end execution tests.
6. Run the complete verification matrix, package the crate, and publish the standalone repository.

## Non-goals for this increment

- Joins, user-defined functions, and arbitrary subqueries.
- A breaking AST redesign.
- A new server or query execution service inside the crate.
- GitHub Actions release automation; releases remain manually controlled.
