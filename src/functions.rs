use crate::ast::{Duration, DurationUnit, Expr};
use crate::error::LqlError;
use crate::ir::ValueType;
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

/// Argument-count requirement for a shared LQL scalar function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// Exactly `n` arguments.
    Exact(usize),
    /// Any argument count in the inclusive range `min..=max`.
    Range(usize, usize),
    /// At least `n` arguments.
    AtLeast(usize),
}

impl Arity {
    /// Return whether `count` satisfies this requirement.
    pub fn accepts(self, count: usize) -> bool {
        match self {
            Self::Exact(n) => count == n,
            Self::Range(min, max) => (min..=max).contains(&count),
            Self::AtLeast(min) => count >= min,
        }
    }
}

/// How the result type of a shared LQL scalar function is resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum ReturnType {
    /// The same type for every call.
    Fixed(ValueType),
    /// The type of argument `index` (zero based).
    Arg(usize),
}

/// One entry in the shared LQL scalar function catalog.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub name: &'static str,
    pub arity: Arity,
    pub returns: ReturnType,
}

/// The single source of truth for LQL scalar functions. All built-in checks,
/// arity validation and result-type inference derive from this catalog.
pub static FUNCTION_CATALOG: &[FunctionSignature] = &[
    FunctionSignature {
        name: "ago",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::Timestamp),
    },
    FunctionSignature {
        name: "bin",
        arity: Arity::Exact(2),
        returns: ReturnType::Fixed(ValueType::Timestamp),
    },
    FunctionSignature {
        name: "now",
        arity: Arity::Exact(0),
        returns: ReturnType::Fixed(ValueType::Timestamp),
    },
    FunctionSignature {
        name: "coalesce",
        arity: Arity::AtLeast(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "if",
        arity: Arity::Exact(3),
        returns: ReturnType::Arg(1),
    },
    FunctionSignature {
        name: "isempty",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::Bool),
    },
    FunctionSignature {
        name: "isnotempty",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::Bool),
    },
    FunctionSignature {
        name: "strlen",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "tolower",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "toupper",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "trim",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "substring",
        arity: Arity::Exact(3),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "replace",
        arity: Arity::Exact(3),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "split",
        arity: Arity::Exact(2),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "array_length",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "typeof",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "to_string",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::String),
    },
    FunctionSignature {
        name: "to_int",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::Int),
    },
    FunctionSignature {
        name: "to_float",
        arity: Arity::Exact(1),
        returns: ReturnType::Fixed(ValueType::Float),
    },
    FunctionSignature {
        name: "abs",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "round",
        arity: Arity::Range(1, 2),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "floor",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "ceil",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "log",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
    FunctionSignature {
        name: "sqrt",
        arity: Arity::Exact(1),
        returns: ReturnType::Arg(0),
    },
];

/// Look up a built-in's signature by (case-insensitive) name.
pub fn function_signature(name: &str) -> Option<&'static FunctionSignature> {
    let lower = name.to_ascii_lowercase();
    FUNCTION_CATALOG
        .iter()
        .find(|signature| signature.name == lower)
}

/// Return whether a function is part of the shared LQL scalar catalog.
pub fn is_builtin_function(name: &str) -> bool {
    function_signature(name).is_some()
}

/// Validate the argument count for a shared LQL scalar function.
///
/// Adding a built-in now requires a single catalog entry, so validation sites
/// can no longer disagree about which functions exist or how many arguments
/// they accept. An unknown name is reported as an error so callers cannot
/// silently skip validation.
pub fn validate_function_arity(name: &str, args: &[Expr]) -> Result<(), LqlError> {
    match function_signature(name) {
        Some(signature) if signature.arity.accepts(args.len()) => Ok(()),
        Some(signature) => Err(LqlError::Compile {
            message: format!(
                "{}() requires a valid argument count; received {}",
                signature.name,
                args.len()
            ),
            span: None,
        }),
        None => Err(LqlError::Compile {
            message: format!("unknown function '{}'", name),
            span: None,
        }),
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

    #[test]
    fn shared_catalog_covers_if_and_rejects_bad_arity() {
        assert!(is_builtin_function("IF"));
        assert!(validate_function_arity("if", &[]).is_err());
        assert!(validate_function_arity(
            "if",
            &[
                Expr::Literal(crate::ast::Literal::Bool(true)),
                Expr::Literal(crate::ast::Literal::Integer(1)),
                Expr::Literal(crate::ast::Literal::Integer(0)),
            ],
        )
        .is_ok());
    }

    #[test]
    fn catalog_drives_every_validation_site() {
        let mut names = std::collections::HashSet::new();
        for signature in FUNCTION_CATALOG {
            assert!(
                names.insert(signature.name),
                "duplicate catalog entry: {}",
                signature.name
            );
            assert!(
                is_builtin_function(signature.name),
                "catalog entry missing from builtin lookup: {}",
                signature.name
            );
            assert!(
                is_builtin_function(&signature.name.to_ascii_uppercase()),
                "builtin lookup is not case-insensitive: {}",
                signature.name
            );

            let valid_count = (0..=4)
                .find(|count| signature.arity.accepts(*count))
                .expect("catalog arity must accept at least one argument count");
            let valid_args = vec![Expr::Literal(crate::ast::Literal::Integer(1)); valid_count];
            assert!(
                validate_function_arity(signature.name, &valid_args).is_ok(),
                "valid arity rejected: {}",
                signature.name
            );
            if let Some(invalid_count) = (0..=8).find(|count| !signature.arity.accepts(*count)) {
                let invalid_args =
                    vec![Expr::Literal(crate::ast::Literal::Integer(1)); invalid_count];
                assert!(
                    validate_function_arity(signature.name, &invalid_args).is_err(),
                    "invalid arity accepted: {}",
                    signature.name
                );
            }
        }
        assert!(!is_builtin_function("definitely_not_a_builtin"));
        assert!(validate_function_arity("definitely_not_a_builtin", &[]).is_err());
    }
}
