//! Property tests: whole-pipeline invariants over generated keys.
//!
//! Deterministic examples live in [`super::tests`]; these sweep the input
//! space more broadly: no panics on arbitrary unicode keys, round-trips
//! for structurally valid paths, the sparse-index length invariant, and
//! the shape of rendered errors.

use proptest::prelude::*;
use serde_json::Value;

use super::NestedJson;

/// Highest index the valid-path generators produce; keeps the null
/// padding of sparse assignments cheap.
const MAX_INDEX: usize = 10_000;

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// Any unicode string of at most `max` characters.
fn unicode_key(max: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..=max).prop_map(|chars| chars.into_iter().collect())
}

/// Like [`unicode_key`], but single-line, for properties that reason
/// about the line structure of rendered errors. A key containing `'\n'`
/// is echoed verbatim and legitimately spreads the middle line.
fn single_line_key(max: usize) -> impl Strategy<Value = String> {
    let ch = any::<char>().prop_filter("key stays single-line", |c| *c != '\n');
    prop::collection::vec(ch, 0..=max).prop_map(|chars| chars.into_iter().collect())
}

/// Keys for the error-shape property: mostly arbitrary, with the empty
/// key and the bare append mixed in so batches also produce span-less
/// type errors (empty root key on an array), which render as one line.
fn error_probe_key() -> impl Strategy<Value = String> {
    prop_oneof![
        6 => single_line_key(60),
        1 => Just(String::new()),
        1 => Just("[]".to_string()),
    ]
}

/// JSON scalars plus short strings: enough value variety to exercise
/// assignment without dominating generation time.
fn simple_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[ -~]{0,12}".prop_map(Value::from),
    ]
}

/// One segment of a structurally valid path, prior to key syntax.
#[derive(Debug, Clone)]
enum PathSegment {
    Key(String),
    Index(usize),
    Append,
}

/// Accessor text that always lexes as text: no `[`/`]`/`\` to escape, and
/// a leading letter so it can never read as an index literal.
fn key_text() -> impl Strategy<Value = String> {
    proptest::string::string_regex(r"\p{L}[\p{L}\p{N} _.\-]{0,8}").expect("valid regex")
}

fn segment() -> impl Strategy<Value = PathSegment> {
    prop_oneof![
        key_text().prop_map(PathSegment::Key),
        (0..=MAX_INDEX).prop_map(PathSegment::Index),
        Just(PathSegment::Append),
    ]
}

fn valid_path() -> impl Strategy<Value = Vec<PathSegment>> {
    prop::collection::vec(segment(), 1..=6)
}

/// Render a path in key syntax: the root key is bare, everything else is
/// bracketed.
fn render_key(path: &[PathSegment]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (i, segment) in path.iter().enumerate() {
        match segment {
            PathSegment::Key(text) if i == 0 => out.push_str(text),
            PathSegment::Key(text) => {
                let _ = write!(out, "[{text}]");
            }
            PathSegment::Index(n) => {
                let _ = write!(out, "[{n}]");
            }
            PathSegment::Append => out.push_str("[]"),
        }
    }
    out
}

/// Walk `body` along `path`. A single assignment on a fresh context puts
/// every appended element at the end of its freshly created array, so
/// appends resolve to the last element.
fn lookup<'a>(body: &'a Value, path: &[PathSegment]) -> Option<&'a Value> {
    let mut cursor = body;
    for segment in path {
        cursor = match segment {
            PathSegment::Key(text) => cursor.get(text.as_str())?,
            PathSegment::Index(n) => cursor.get(*n)?,
            PathSegment::Append => cursor.as_array()?.last()?,
        };
    }
    Some(cursor)
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

proptest! {
    /// Whatever the key, `assign` returns `Ok` or `Err` — it never panics.
    #[test]
    fn assign_never_panics(key in unicode_key(200), value in simple_value()) {
        let mut nested = NestedJson::new();
        let _ = nested.assign(&key, value);
        let _ = nested.finish();
    }

    /// Batches of prefix-sharing keys never panic, whatever each
    /// assignment returns along the way.
    #[test]
    fn prefixed_batches_never_panic(
        prefix in unicode_key(20),
        pairs in prop::collection::vec((unicode_key(30), simple_value()), 1..=8),
    ) {
        let mut nested = NestedJson::new();
        for (suffix, value) in pairs {
            let _ = nested.assign(&format!("{prefix}{suffix}"), value);
        }
        let _ = nested.finish();
    }

    /// Structurally valid paths always assign cleanly on a fresh context,
    /// and the value is retrievable at that path.
    #[test]
    fn valid_path_round_trips(path in valid_path(), value in simple_value()) {
        let key = render_key(&path);
        let mut nested = NestedJson::new();
        let result = nested.assign(&key, value.clone());
        prop_assert!(result.is_ok(), "assign failed for {:?}:\n{}", key, result.unwrap_err());
        let body = nested.finish();
        let found = lookup(&body, &path);
        prop_assert_eq!(found, Some(&value), "wrong value at {:?} in {}", key, &body);
    }

    /// Sparse index invariant: assigning at valid index `n` on a fresh
    /// context yields an array of exactly `n + 1` elements.
    #[test]
    fn sparse_index_sets_length(
        root in key_text(),
        n in 0..=MAX_INDEX,
        value in simple_value(),
    ) {
        let key = format!("{root}[{n}]");
        let mut nested = NestedJson::new();
        prop_assert!(nested.assign(&key, value).is_ok(), "assign failed for {:?}", key);
        let body = nested.finish();
        let array = body.get(root.as_str()).and_then(Value::as_array);
        prop_assert!(array.is_some(), "expected an array at {:?} in {}", root, &body);
        prop_assert_eq!(array.unwrap().len(), n + 1);
    }

    /// Rendered errors are one line (span-less) or exactly three: the
    /// message, the key verbatim, and a caret line of spaces then carets
    /// whose length never exceeds the key line plus one (end-of-input
    /// errors point one column past the last character).
    #[test]
    fn error_rendering_shape(
        pairs in prop::collection::vec((error_probe_key(), simple_value()), 1..=4),
    ) {
        let mut nested = NestedJson::new();
        for (key, value) in &pairs {
            let Err(error) = nested.assign(key, value.clone()) else {
                continue;
            };
            let rendered = error.to_string();
            let lines: Vec<&str> = rendered.split('\n').collect();
            prop_assert!(
                lines[0].starts_with("furl ") && lines[0].contains(" Error: "),
                "malformed message line: {:?}",
                rendered
            );
            match lines.as_slice() {
                [_message] => {}
                [_message, echoed, carets] => {
                    prop_assert_eq!(*echoed, key.as_str(), "key not echoed verbatim");
                    let pointer = carets.trim_start_matches(' ');
                    prop_assert!(
                        !pointer.is_empty() && pointer.chars().all(|c| c == '^'),
                        "caret line is not spaces then carets: {:?}",
                        carets
                    );
                    prop_assert!(
                        carets.chars().count() <= key.chars().count() + 1,
                        "caret line overshoots key {:?}: {:?}",
                        key,
                        carets
                    );
                }
                _ => prop_assert!(false, "expected 1 or 3 lines, got: {:?}", rendered),
            }
        }
    }
}
