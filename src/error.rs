use std::error::Error as StdError;
use std::fmt;

/// A parse error with source location and optional source line context.
///
/// The `Display` implementation formats the error as `line N, column M: message` and,
/// when source context is available, appends the source line and a caret pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseError {
    line: usize,
    column: usize,
    message: String,
    source_line: Option<String>,
}

impl ParseError {
    pub(crate) fn new(line: usize, column: usize, message: impl Into<String>, source_line: Option<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
            source_line,
        }
    }

    /// 1-based line number where the error occurred.
    pub fn line(&self) -> usize { self.line }
    /// 1-based column number where the error occurred.
    pub fn column(&self) -> usize { self.column }
    /// Human-readable error message.
    pub fn message(&self) -> &str { &self.message }
    /// The source line text, if available, for display with a caret pointer.
    pub fn source_line(&self) -> Option<&str> { self.source_line.as_deref() }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)?;
        if let Some(src) = &self.source_line {
            write!(f, "\n  {}\n  {:>width$}", src, "^", width = self.column)?;
        }
        Ok(())
    }
}

impl StdError for ParseError {}

/// The error type for all TJSON operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// A parse error with source location.
    Parse(ParseError),
    /// A JSON serialization or deserialization error from serde_json.
    Json(serde_json::Error),
    /// A render error due to an internal invariant violation (indicates a bug in this library).
    Render(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Render(message) => write!(f, "{message}"),
        }
    }
}

impl StdError for Error {}

impl From<ParseError> for Error {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Convenience `Result` type with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
