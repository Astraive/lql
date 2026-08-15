use crate::ast::{Duration, DurationUnit, Expr};
use crate::error::LqlError;
use std::time::{SystemTime, UNIX_EPOCH};

/// Compute the timestamp for `ago(duration)` — current time minus the duration.
pub fn ago_millis(d: &Duration) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    now.saturating_sub(d.to_millis())
}

/// Format a duration as a DuckDB INTERVAL literal.
pub fn duration_to_duckdb_interval(d: &Duration) -> String {
    match d.unit {
        DurationUnit::Milliseconds => format!("INTERVAL '{}' MILLISECOND", d.value),
        DurationUnit::Seconds => format!("INTERVAL '{}' SECOND", d.value),
        DurationUnit::Minutes => format!("INTERVAL '{}' MINUTE", d.value),
        DurationUnit::Hours => format!("INTERVAL '{}' HOUR", d.value),
        DurationUnit::Days => format!("INTERVAL '{}' DAY", d.value),
        DurationUnit::Weeks => format!("INTERVAL '{}' DAY", d.value.saturating_mul(7)),
    }
}

/// Format a duration as a ClickHouse INTERVAL literal.
pub fn duration_to_clickhouse_interval(d: &Duration) -> String {
    match d.unit {
        DurationUnit::Milliseconds => format!("toIntervalMillisecond({})", d.value),
        DurationUnit::Seconds => format!("toIntervalSecond({})", d.value),
        DurationUnit::Minutes => format!("toIntervalMinute({})", d.value),
        DurationUnit::Hours => format!("toIntervalHour({})", d.value),
        DurationUnit::Days => format!("toIntervalDay({})", d.value),
        DurationUnit::Weeks => format!("toIntervalDay({})", d.value.saturating_mul(7)),
    }
}

/// Known built-in functions and their signatures.
pub fn is_builtin_function(name: &str) -> bool {
    matches!(
        name.to_lowercase().as_str(),
        "ago"
            | "bin"
            | "now"
            | "coalesce"
            | "if"
            | "isempty"
            | "isnotempty"
            | "strlen"
            | "tolower"
            | "toupper"
            | "trim"
            | "substring"
            | "replace"
            | "split"
            | "array_length"
            | "typeof"
            | "to_string"
            | "to_int"
            | "to_float"
            | "abs"
            | "round"
            | "floor"
            | "ceil"
            | "log"
            | "sqrt"
    )
}

/// Validate the argument count for a built-in scalar function.
pub fn validate_function_arity(name: &str, args: &[Expr]) -> Result<(), LqlError> {
    let name = name.to_lowercase();
    let valid = match name.as_str() {
        "ago" | "strlen" | "tolower" | "toupper" | "trim" | "typeof" | "to_string" | "to_int"
        | "to_float" | "abs" | "floor" | "ceil" | "log" | "sqrt" | "isempty" | "isnotempty"
        | "array_length" => args.len() == 1,
        "now" => args.is_empty(),
        "coalesce" => !args.is_empty(),
        "bin" => args.len() == 2,
        "substring" => args.len() == 3,
        "split" => args.len() == 2,
        "replace" => args.len() == 3,
        "round" => (1..=2).contains(&args.len()),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(LqlError::Compile {
            message: format!(
                "{}() requires a valid argument count; received {}",
                name,
                args.len()
            ),
            span: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_1h() {
        let d = Duration {
            value: 1,
            unit: DurationUnit::Hours,
        };
        let ts = ago_millis(&d);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(ts < now);
        assert!(now - ts >= 3_600_000 - 1000); // within 1s tolerance
    }

    #[test]
    fn duration_to_intervals() {
        let d = Duration {
            value: 5,
            unit: DurationUnit::Minutes,
        };
        assert_eq!(duration_to_duckdb_interval(&d), "INTERVAL '5' MINUTE");
        assert_eq!(duration_to_clickhouse_interval(&d), "toIntervalMinute(5)");
    }
}
