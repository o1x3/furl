//! Folds parsed path assignments into the accumulated JSON context.

use serde_json::{Map, Value};

use super::parser::{Segment, SegmentKind};
use super::{ErrorKind, NestedJsonError};

/// Apply one `path = value` assignment to the shared context.
///
/// The cursor walks the existing structure segment by segment, creating
/// missing containers along the way; the final segment stores the value.
pub(crate) fn assign(
    context: &mut Option<Value>,
    key: &str,
    segments: &[Segment],
    value: Value,
) -> Result<(), NestedJsonError> {
    debug_assert!(!segments.is_empty(), "a parsed path has at least the root");
    if context.is_none() {
        *context = Some(fresh_container(&segments[0]));
    }
    let mut cursor = context.as_mut().expect("context initialized above");
    let mut value = Some(value);

    for (i, segment) in segments.iter().enumerate() {
        let is_last = i + 1 == segments.len();
        match &segment.kind {
            SegmentKind::Key(k) => {
                if !cursor.is_object() {
                    return Err(type_error("key", segments, i, cursor, "object", key));
                }
                let Value::Object(object) = cursor else {
                    unreachable!()
                };
                if is_last {
                    object.insert(k.clone(), value.take().expect("value set only once"));
                    return Ok(());
                }
                let slot = object
                    .entry(k.clone())
                    .or_insert_with(|| fresh_container(&segments[i + 1]));
                // An explicit null marks "no structure here yet": descending
                // through it replaces it with a fresh container.
                if slot.is_null() {
                    *slot = fresh_container(&segments[i + 1]);
                }
                cursor = slot;
            }
            SegmentKind::Index {
                value: index,
                number_span,
            } => {
                if !cursor.is_array() {
                    return Err(type_error("index", segments, i, cursor, "array", key));
                }
                if *index < 0 {
                    return Err(NestedJsonError {
                        kind: ErrorKind::Value,
                        message: "Negative indexes are not supported.".to_string(),
                        key: key.to_string(),
                        span: Some(*number_span),
                    });
                }
                let Value::Array(array) = cursor else {
                    unreachable!()
                };
                let index = usize::try_from(*index).unwrap_or(usize::MAX);
                if array.len() <= index {
                    // Sparse assignment: pad the gap with nulls. Absurd
                    // indexes fail cleanly instead of aborting on OOM.
                    let needed = index
                        .checked_sub(array.len())
                        .and_then(|gap| gap.checked_add(1));
                    let reserved = needed
                        .map(|n| array.try_reserve(n).is_ok())
                        .unwrap_or(false);
                    if !reserved {
                        return Err(NestedJsonError {
                            kind: ErrorKind::Value,
                            message: "Index is too large.".to_string(),
                            key: key.to_string(),
                            span: Some(*number_span),
                        });
                    }
                    array.resize(index + 1, Value::Null);
                }
                if is_last {
                    array[index] = value.take().expect("value set only once");
                    return Ok(());
                }
                let slot = &mut array[index];
                if slot.is_null() {
                    *slot = fresh_container(&segments[i + 1]);
                }
                cursor = slot;
            }
            SegmentKind::Append => {
                if !cursor.is_array() {
                    return Err(type_error("append", segments, i, cursor, "array", key));
                }
                let Value::Array(array) = cursor else {
                    unreachable!()
                };
                if is_last {
                    array.push(value.take().expect("value set only once"));
                    return Ok(());
                }
                array.push(fresh_container(&segments[i + 1]));
                cursor = array.last_mut().expect("just pushed");
            }
        }
    }
    unreachable!("the final segment always stores the value and returns")
}

/// The container a segment wants to address into: objects for keys,
/// arrays for indexes and appends.
fn fresh_container(segment: &Segment) -> Value {
    match segment.kind {
        SegmentKind::Key(_) => Value::Object(Map::new()),
        SegmentKind::Index { .. } | SegmentKind::Append => Value::Array(Vec::new()),
    }
}

fn type_error(
    operation: &str,
    segments: &[Segment],
    offending: usize,
    actual: &Value,
    required: &str,
    key: &str,
) -> NestedJsonError {
    NestedJsonError {
        kind: ErrorKind::Type,
        message: format!(
            "Cannot perform '{operation}' based access on '{prefix}' \
             which has a type of '{actual}' but this operation requires \
             a type of '{required}'.",
            prefix = render_prefix(&segments[..offending]),
            actual = json_type_name(actual),
        ),
        key: key.to_string(),
        span: segments[offending].span,
    }
}

/// Reconstruct the path up to the offending segment for error messages.
///
/// Accessors render decoded except that literal backslashes are doubled;
/// brackets that were escaped in the source are not re-escaped.
fn render_prefix(segments: &[Segment]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (i, segment) in segments.iter().enumerate() {
        match &segment.kind {
            SegmentKind::Key(k) if i == 0 => out.push_str(&k.replace('\\', "\\\\")),
            SegmentKind::Key(k) => {
                let _ = write!(out, "[{}]", k.replace('\\', "\\\\"));
            }
            SegmentKind::Index { value, .. } => {
                let _ = write!(out, "[{value}]");
            }
            SegmentKind::Append => out.push_str("[]"),
        }
    }
    out
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}
