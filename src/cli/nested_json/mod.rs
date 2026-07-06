//! The nested-JSON data syntax.
//!
//! Request data items like `pet[name]=Hypatia` or `kids[0]:=1` address into
//! a JSON structure with a bracket path syntax. This module turns an ordered
//! sequence of `(key, value)` assignments into a single JSON body.
//!
//! The pipeline is: [`lexer`] splits a key into bracket/literal tokens,
//! [`parser`] shapes tokens into path segments, and [`interpreter`] folds
//! each assignment into the accumulated JSON context.

mod interpreter;
mod lexer;
mod parser;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod tests;

use crate::json::Value;

/// A half-open span of character positions within a data-item key.
///
/// Positions are counted in characters (not bytes) because they drive
/// caret alignment under the echoed key in error messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// Which family a nested-JSON error belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The key does not match the path grammar.
    Syntax,
    /// An access does not match the type of the existing value at that path.
    Type,
    /// The path is well-formed but its value is unusable (e.g. a negative index).
    Value,
}

impl ErrorKind {
    fn label(self) -> &'static str {
        match self {
            ErrorKind::Syntax => "Syntax",
            ErrorKind::Type => "Type",
            ErrorKind::Value => "Value",
        }
    }
}

/// An error raised while lexing, parsing, or interpreting a data-item key.
///
/// Rendered as up to three lines: the message, the offending key verbatim,
/// and a caret line highlighting the token the error is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestedJsonError {
    pub kind: ErrorKind,
    pub message: String,
    /// The key of the offending data item, verbatim.
    pub key: String,
    /// Character span to highlight with carets; `None` renders message-only.
    pub span: Option<Span>,
}

impl std::fmt::Display for NestedJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "furl {} Error: {}", self.kind.label(), self.message)?;
        if let Some(span) = self.span {
            let width = span.end.saturating_sub(span.start).max(1);
            write!(
                f,
                "\n{}\n{}{}",
                self.key,
                " ".repeat(span.start),
                "^".repeat(width)
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for NestedJsonError {}

/// Accumulates ordered `key → value` assignments into one JSON body.
#[derive(Debug, Default)]
pub struct NestedJson {
    context: Option<Value>,
}

impl NestedJson {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one assignment. Later assignments may overwrite or extend
    /// structures created by earlier ones.
    pub fn assign(&mut self, key: &str, value: Value) -> Result<(), NestedJsonError> {
        let segments = parser::parse(key)?;
        interpreter::assign(&mut self.context, key, &segments, value)
    }

    /// The finished JSON body: an object in the common case, or a bare
    /// array when every assignment addressed the top level as an array
    /// (`[]:=1`-style keys).
    pub fn finish(self) -> Value {
        match self.context {
            None => Value::Object(Vec::new()),
            Some(value) => value,
        }
    }
}
