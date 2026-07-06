//! JSON values with CLI-grade fidelity.
//!
//! The standard-library-style JSON crates trade away details this program
//! must preserve: object key order, duplicate keys (legal in JSON text and
//! meaningful to servers), the distinction between integer and float
//! literals, and big integers beyond the f64/i64 range. This module keeps
//! all of them, and its serializer and parser reproduce the observable
//! conventions of the reference tooling this CLI is compatible with
//! (indentation, ASCII escaping, float rendering, error message shapes).

mod parse;
mod ser;

#[cfg(test)]
mod tests;

pub use parse::{ParseError, parse};
pub use ser::{DumpOptions, dumps};

/// A JSON value: objects preserve insertion order and duplicate keys.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Value::Object(_))
    }

    pub fn is_array(&self) -> bool {
        matches!(self, Value::Array(_))
    }

    /// Look up a key; with duplicate keys the last occurrence wins,
    /// matching what a last-wins consumer of the JSON text would see.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_index(&self, index: usize) -> Option<&Value> {
        match self {
            Value::Array(items) => items.get(index),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Set a key: with duplicate keys the last occurrence is replaced in
    /// place; a missing key is appended.
    pub fn object_set(&mut self, key: &str, value: Value) {
        if let Value::Object(pairs) = self {
            match pairs.iter().rposition(|(name, _)| name == key) {
                Some(i) => pairs[i].1 = value,
                None => pairs.push((key.to_string(), value)),
            }
        }
    }

    /// The JSON type name, as used in user-facing messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }
}

/// A JSON number: either an integer kept as its full decimal text (no
/// range limit) or a float.
#[derive(Debug, Clone, PartialEq)]
pub struct Number(pub(crate) NumberRepr);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NumberRepr {
    /// Canonical decimal text: optional `-`, no leading zeros.
    Int(String),
    Float(f64),
}

impl Number {
    pub fn from_i64(n: i64) -> Number {
        Number(NumberRepr::Int(n.to_string()))
    }

    pub fn from_f64(f: f64) -> Number {
        Number(NumberRepr::Float(f))
    }

    /// An integer from its canonical decimal text (as produced by the
    /// parser); arbitrary size.
    pub(crate) fn from_int_literal(text: String) -> Number {
        Number(NumberRepr::Int(text))
    }

    pub fn is_int(&self) -> bool {
        matches!(self.0, NumberRepr::Int(_))
    }

    pub fn as_i64(&self) -> Option<i64> {
        match &self.0 {
            NumberRepr::Int(text) => text.parse().ok(),
            NumberRepr::Float(_) => None,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match &self.0 {
            NumberRepr::Int(text) => text.parse().unwrap_or(f64::NAN),
            NumberRepr::Float(f) => *f,
        }
    }
}

impl std::fmt::Display for Value {
    /// Compact serialization with the default options.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&dumps(self, &DumpOptions::default()))
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Number(Number::from_i64(n))
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Value {
        Value::Number(Number::from_f64(f))
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Value {
        Value::Bool(b)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::String(s)
    }
}
