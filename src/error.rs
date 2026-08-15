use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A UTF-8 byte-offset span into the original query source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
    pub const fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }

    pub fn clamp_to(&self, source: &str) -> Self {
        let mut start = self.start.min(source.len());
        let mut end = self.end.min(source.len());
        while start > 0 && !source.is_char_boundary(start) {
            start -= 1;
        }
        while end > 0 && !source.is_char_boundary(end) {
            end -= 1;
        }
        if end < start {
            end = start;
        }
        Self { start, end }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Error => "error",
                Self::Warning => "warning",
                Self::Info => "info",
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticLabel {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub primary_span: Option<Span>,
    #[serde(default)]
    pub labels: Vec<DiagnosticLabel>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            message: message.into(),
            primary_span: span,
            labels: Vec::new(),
        }
    }
    pub fn render(&self, source: &str) -> String {
        let Some(span) = self.primary_span.map(|s| s.clamp_to(source)) else {
            return format!("{}[{}]: {}", self.severity, self.code, self.message);
        };
        let (line, column) = line_column(source, span.start);
        let snippet = source.get(span.start..span.end).unwrap_or("");
        if snippet.is_empty() {
            format!(
                "{}[{}] at {}:{}: {}",
                self.severity, self.code, line, column, self.message
            )
        } else {
            format!(
                "{}[{}] at {}:{}: {} ({:?})",
                self.severity, self.code, line, column, self.message, snippet
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    pub diagnostics: Vec<Diagnostic>,
}

impl DiagnosticBundle {
    pub fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self { diagnostics }
    }
    pub fn one(diagnostic: Diagnostic) -> Self {
        Self {
            diagnostics: vec![diagnostic],
        }
    }
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
    pub fn render(&self, source: &str) -> String {
        self.diagnostics
            .iter()
            .map(|d| d.render(source))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl std::fmt::Display for DiagnosticBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, diagnostic) in self.diagnostics.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(
                f,
                "{}[{}]: {}",
                diagnostic.severity, diagnostic.code, diagnostic.message
            )?;
        }
        Ok(())
    }
}
impl std::error::Error for DiagnosticBundle {}
/// Legacy error type retained for source compatibility. New parse/analyze/render APIs
/// convert it to a serializable DiagnosticBundle.
#[derive(Debug, Clone, Error)]
pub enum LqlError {
    #[error("unexpected character '{char}' at position {pos}")]
    UnexpectedChar { char: char, pos: usize },
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("unexpected token '{token}' at position {pos}, expected {expected}")]
    UnexpectedToken {
        token: String,
        expected: String,
        pos: usize,
    },
    #[error("unknown function '{name}'")]
    UnknownFunction { name: String, span: Span },
    #[error("unknown field '{name}'")]
    UnknownField { name: String, span: Span },
    #[error("type mismatch: {message}")]
    TypeMismatch { message: String, span: Span },
    #[error("invalid duration: {input}")]
    InvalidDuration { input: String, span: Span },
    #[error("invalid aggregation: {message}")]
    InvalidAggregation { message: String, span: Span },
    #[error("{message}")]
    Compile { message: String, span: Option<Span> },
}

impl LqlError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnexpectedChar { .. } => "LQL001",
            Self::UnexpectedEof => "LQL002",
            Self::UnexpectedToken { .. } => "LQL003",
            Self::UnknownFunction { .. } => "LQL101",
            Self::UnknownField { .. } => "LQL102",
            Self::TypeMismatch { .. } => "LQL103",
            Self::InvalidDuration { .. } => "LQL104",
            Self::InvalidAggregation { .. } => "LQL105",
            Self::Compile { .. } => "LQL200",
        }
    }

    pub fn span(&self) -> Option<Span> {
        match self {
            Self::UnexpectedChar { pos, .. } | Self::UnexpectedToken { pos, .. } => {
                Some(Span::new(*pos, pos.saturating_add(1)))
            }
            Self::UnexpectedEof => None,
            Self::UnknownFunction { span, .. }
            | Self::UnknownField { span, .. }
            | Self::TypeMismatch { span, .. }
            | Self::InvalidDuration { span, .. }
            | Self::InvalidAggregation { span, .. } => Some(*span),
            Self::Compile { span, .. } => *span,
        }
    }

    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(self.code(), self.to_string(), self.span())
    }

    pub fn bundle(&self) -> DiagnosticBundle {
        DiagnosticBundle::one(self.diagnostic())
    }
}

pub fn line_column(source: &str, byte_offset: usize) -> (usize, usize) {
    let offset = byte_offset.min(source.len());
    let before = &source[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    (line, column)
}
