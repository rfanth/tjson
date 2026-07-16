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

/// Shared diagnostic presentation: `line N, column M: message` plus, when the source
/// line is available, the line text and a caret pointer. Parse errors and located
/// deserialize errors must render identically — a user should not be able to tell which
/// phase rejected their file.
fn fmt_located(
    f: &mut fmt::Formatter<'_>,
    line: usize,
    column: usize,
    message: &str,
    source_line: Option<&str>,
) -> fmt::Result {
    write!(f, "line {line}, column {column}: {message}")?;
    if let Some(src) = source_line {
        write!(f, "\n  {}\n  {:>width$}", src, "^", width = column)?;
    }
    Ok(())
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_located(f, self.line, self.column, &self.message, self.source_line.as_deref())
    }
}

impl StdError for ParseError {}

/// A source position resolved from a span. Internal: the public surface exposes it only
/// through `DeserializeError`'s accessor functions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Location {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) source_line: Option<String>,
}

/// A typed-deserialization error: the document parsed as valid TJSON but does not match
/// the target type.
///
/// Distinct from [`ParseError`] because their invariants differ: a parse error always
/// has a source location, while a deserialize error has one only when the data came
/// from source text ([`crate::from_str`]) rather than from a programmatically built
/// tree ([`crate::from_value`]). No sentinel coordinates, ever.
#[derive(Clone, Debug)]
pub struct DeserializeError {
    /// Field path from the root to the failing value, e.g. `servers[3].timeout`.
    /// Empty when the failure is at the root.
    path: String,
    message: String,
    location: Option<Location>,
}

impl DeserializeError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self { path: String::new(), message: message.into(), location: None }
    }

    /// Prepend a path segment while the error bubbles out of a container.
    /// `segment` is either a key (`timeout`) or an index (`[3]`). A dot joins the
    /// segment to an existing key path; an index path joins bare (`servers[3].timeout`).
    pub(crate) fn nest(mut self, segment: &str) -> Self {
        if self.path.is_empty() || self.path.starts_with('[') {
            self.path = format!("{segment}{}", self.path);
        } else {
            self.path = format!("{segment}.{}", self.path);
        }
        self
    }

    /// Stamp a location if none has been recorded yet. The deepest stamp wins: leaf
    /// deserializers stamp first, containers leave existing locations alone.
    pub(crate) fn locate(mut self, location: Location) -> Self {
        if self.location.is_none() {
            self.location = Some(location);
        }
        self
    }

    pub(crate) fn is_located(&self) -> bool {
        self.location.is_some()
    }

    /// Append an explanatory note to the message. Used for causes the serde protocol
    /// hides from the error's point of origin.
    pub(crate) fn with_note(mut self, note: &str) -> Self {
        self.message = format!("{} ({note})", self.message);
        self
    }

    /// Human-readable error message, without path or position.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Field path from the root to the failing value (e.g. `servers[3].timeout`),
    /// empty when the failure is at the root.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// 1-based line number, when the data came from source text.
    pub fn line(&self) -> Option<usize> {
        self.location.as_ref().map(|l| l.line)
    }

    /// 1-based column number, when the data came from source text.
    pub fn column(&self) -> Option<usize> {
        self.location.as_ref().map(|l| l.column)
    }

    /// The source line text, if available, for display with a caret pointer.
    pub fn source_line(&self) -> Option<&str> {
        self.location.as_ref().and_then(|l| l.source_line.as_deref())
    }

    fn message_with_path(&self) -> String {
        if self.path.is_empty() {
            self.message.clone()
        } else {
            format!("{}: {}", self.path, self.message)
        }
    }
}

impl fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Some(location) => fmt_located(
                f,
                location.line,
                location.column,
                &self.message_with_path(),
                location.source_line.as_deref(),
            ),
            None => f.write_str(&self.message_with_path()),
        }
    }
}

impl StdError for DeserializeError {}

impl serde::de::Error for DeserializeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::new(msg.to_string())
    }

    fn invalid_type(unexp: serde::de::Unexpected, exp: &dyn serde::de::Expected) -> Self {
        // serde's data-model name for null is "unit"; say "null" like serde_json does,
        // since that is the word the document actually contains.
        if let serde::de::Unexpected::Unit = unexp {
            Self::new(format!("invalid type: null, expected {exp}"))
        } else {
            Self::new(format!("invalid type: {unexp}, expected {exp}"))
        }
    }
}

/// The error type for all TJSON operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// A parse error with source location.
    Parse(ParseError),
    /// A typed-deserialization error: valid TJSON that does not match the target type.
    Deserialize(DeserializeError),
    /// A JSON serialization or deserialization error from serde_json.
    Json(serde_json::Error),
    /// A render error due to an internal invariant violation (indicates a bug in this library).
    Render(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "{error}"),
            Self::Deserialize(error) => write!(f, "{error}"),
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

impl From<DeserializeError> for Error {
    fn from(error: DeserializeError) -> Self {
        Self::Deserialize(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Convenience `Result` type with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;
