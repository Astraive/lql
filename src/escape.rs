//! Shared, dialect-aware SQL escaping helpers.
//!
//! Every value interpolated into generated SQL — identifiers as well as
//! string literals — must go through one of the helpers here. Keeping the
//! quoting rules in a single, audited place prevents ad-hoc `format!` calls
//! from turning into SQL-injection sinks (LQL-BUG-004).

use crate::compiler::Target;

/// Maximum number of fields accepted from a client-supplied schema document.
pub const MAX_SCHEMA_FIELDS: usize = 512;

/// Maximum length of a field, physical or table name in a schema document.
pub const MAX_SCHEMA_NAME_LEN: usize = 128;

/// Quote an SQL identifier for `target`.
///
/// DuckDB and PostgreSQL use ANSI double quotes, doubling embedded quotes.
/// ClickHouse also accepts double quotes, but backticks are its canonical
/// quoted-identifier form, so we emit those and double embedded backticks.
/// The special `*` token is passed through so `SELECT *` / `COUNT(*)` are
/// unchanged.
pub fn quote_identifier(name: &str, target: Target) -> String {
    if name == "*" {
        return "*".to_string();
    }
    match target {
        Target::ClickHouse => format!("`{}`", name.replace('`', "``")),
        Target::DuckDB | Target::PostgreSQL => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Escape the contents of a single-quoted SQL string literal for `target`.
/// The caller supplies the surrounding single quotes.
///
/// ClickHouse uses backslash escapes inside string literals, whereas DuckDB
/// and PostgreSQL (with `standard_conforming_strings`) double embedded quotes.
/// Escaping both quote and backslash here stops a value from ever breaking out
/// of its literal, regardless of dialect.
pub fn escape_string_literal(value: &str, target: Target) -> String {
    match target {
        Target::ClickHouse => value.replace('\\', "\\\\").replace('\'', "\\'"),
        Target::DuckDB | Target::PostgreSQL => value.replace('\'', "''"),
    }
}

/// Quote a single-quoted SQL string literal for `target`, including its quotes.
pub fn quote_string_literal(value: &str, target: Target) -> String {
    format!("'{}'", escape_string_literal(value, target))
}

/// Whether `name` is a safe identifier for a client-supplied schema document.
///
/// Field names may contain dots (JSON dot-paths); physical and table names use
/// underscores. Anything outside `[A-Za-z0-9_.]` is rejected so a schema
/// document cannot inject SQL or JSONPath syntax. Empty and overly long names
/// are rejected as well.
pub fn is_safe_schema_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SCHEMA_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_identifier_escapes_embedded_quotes() {
        assert_eq!(quote_identifier("a\"b", Target::DuckDB), "\"a\"\"b\"");
        assert_eq!(quote_identifier("a\"b", Target::PostgreSQL), "\"a\"\"b\"");
        assert_eq!(quote_identifier("a`b", Target::ClickHouse), "`a``b`");
        assert_eq!(quote_identifier("*", Target::DuckDB), "*");
    }

    #[test]
    fn escape_string_literal_escapes_quotes_and_backslashes() {
        assert_eq!(escape_string_literal("a'b", Target::DuckDB), "a''b");
        assert_eq!(escape_string_literal("a'b", Target::PostgreSQL), "a''b");
        assert_eq!(
            escape_string_literal("a'\\b", Target::ClickHouse),
            "a\\'\\\\b"
        );
        assert_eq!(quote_string_literal("a'b", Target::DuckDB), "'a''b'");
    }

    #[test]
    fn schema_name_allowlist_rejects_injection_payloads() {
        assert!(is_safe_schema_name("service"));
        assert!(is_safe_schema_name("error.code"));
        assert!(is_safe_schema_name("http_status"));
        assert!(!is_safe_schema_name(""));
        assert!(!is_safe_schema_name("a'b"));
        assert!(!is_safe_schema_name("a;b"));
        assert!(!is_safe_schema_name("a\\b"));
        assert!(!is_safe_schema_name("a\"b"));
        assert!(!is_safe_schema_name(&"a".repeat(MAX_SCHEMA_NAME_LEN + 1)));
    }
}
