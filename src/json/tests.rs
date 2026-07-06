use super::{DumpOptions, Number, Value, dumps, parse};

#[track_caller]
fn parse_err(text: &str) -> String {
    parse(text).expect_err("expected parse error").to_string()
}

fn compact(value: &Value) -> String {
    dumps(value, &DumpOptions::default())
}

// -- parsing ----------------------------------------------------------------

#[test]
fn parses_scalars() {
    assert_eq!(parse("null").unwrap(), Value::Null);
    assert_eq!(parse("true").unwrap(), Value::Bool(true));
    assert_eq!(parse("false").unwrap(), Value::Bool(false));
    assert_eq!(parse(" 1 ").unwrap(), Value::from(1));
    assert_eq!(parse("\"s\"").unwrap(), Value::from("s"));
}

#[test]
fn parses_python_extras() {
    assert!(matches!(
        parse("NaN").unwrap(),
        Value::Number(n) if n.as_f64().is_nan()
    ));
    assert_eq!(parse("Infinity").unwrap(), Value::from(f64::INFINITY));
    assert_eq!(parse("-Infinity").unwrap(), Value::from(f64::NEG_INFINITY));
    assert_eq!(parse("1e400").unwrap(), Value::from(f64::INFINITY));
    assert_eq!(parse("-1e400").unwrap(), Value::from(f64::NEG_INFINITY));
}

#[test]
fn preserves_duplicate_keys_in_order() {
    let value = parse(r#"{"a": 1, "b": 2, "a": 3}"#).unwrap();
    assert_eq!(compact(&value), r#"{"a": 1, "b": 2, "a": 3}"#);
    // Lookup sees the last occurrence.
    assert_eq!(value.get("a"), Some(&Value::from(3)));
}

#[test]
fn preserves_big_integers() {
    let text = "123456789012345678901234567890";
    let value = parse(text).unwrap();
    assert_eq!(compact(&value), text);
    assert_eq!(parse("-0").unwrap(), Value::from(0));
}

#[test]
fn int_vs_float_identity() {
    assert!(matches!(parse("1").unwrap(), Value::Number(n) if n.is_int()));
    assert!(matches!(parse("1.0").unwrap(), Value::Number(n) if !n.is_int()));
    assert!(matches!(parse("1e2").unwrap(), Value::Number(n) if !n.is_int()));
}

#[test]
fn surrogate_pairs_combine() {
    assert_eq!(parse(r#""🎉""#).unwrap(), Value::from("🎉"));
    // Lone surrogates cannot round-trip into Rust strings.
    assert_eq!(parse(r#""\ud800""#).unwrap(), Value::from("\u{FFFD}"));
}

#[test]
fn string_escapes() {
    assert_eq!(
        parse(r#""\" \\ \/ \b \f \n \r \t é""#).unwrap(),
        Value::from("\" \\ / \u{8} \u{c} \n \r \t é")
    );
}

#[test]
fn parse_error_messages() {
    // Shapes verified against the reference implementation.
    assert_eq!(parse_err(""), "Expecting value: line 1 column 1 (char 0)");
    assert_eq!(parse_err("  "), "Expecting value: line 1 column 3 (char 2)");
    assert_eq!(
        parse_err("{"),
        "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err("{'a':1}"),
        "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err(r#"{"a""#),
        "Expecting ':' delimiter: line 1 column 5 (char 4)"
    );
    assert_eq!(
        parse_err(r#"{"a" 1}"#),
        "Expecting ':' delimiter: line 1 column 6 (char 5)"
    );
    assert_eq!(
        parse_err(r#"{"a":}"#),
        "Expecting value: line 1 column 6 (char 5)"
    );
    assert_eq!(
        parse_err(r#"{"a":1 "b":2}"#),
        "Expecting ',' delimiter: line 1 column 8 (char 7)"
    );
    assert_eq!(
        parse_err("{,}"),
        "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err(r#"{"a":1,,"b":2}"#),
        "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)"
    );
    assert_eq!(
        parse_err(r#"{"a":1,}"#),
        "Illegal trailing comma before end of object: line 1 column 7 (char 6)"
    );
    assert_eq!(
        parse_err("[1,]"),
        "Illegal trailing comma before end of array: line 1 column 3 (char 2)"
    );
    assert_eq!(
        parse_err("[,1]"),
        "Expecting value: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err("[1 2]"),
        "Expecting ',' delimiter: line 1 column 4 (char 3)"
    );
    assert_eq!(
        parse_err("[1"),
        "Expecting ',' delimiter: line 1 column 3 (char 2)"
    );
    assert_eq!(
        parse_err("[01]"),
        "Expecting ',' delimiter: line 1 column 3 (char 2)"
    );
    assert_eq!(
        parse_err("tru"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        parse_err("nul"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        parse_err("Infinit"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        parse_err("-Inf"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(parse_err("-"), "Expecting value: line 1 column 1 (char 0)");
    assert_eq!(
        parse_err("--1"),
        "Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(parse_err(".5"), "Expecting value: line 1 column 1 (char 0)");
    assert_eq!(parse_err("1.2.3"), "Extra data: line 1 column 4 (char 3)");
    assert_eq!(parse_err("1e"), "Extra data: line 1 column 2 (char 1)");
    assert_eq!(parse_err("1e+"), "Extra data: line 1 column 2 (char 1)");
    assert_eq!(parse_err("5."), "Extra data: line 1 column 2 (char 1)");
    assert_eq!(
        parse_err("\"\x01\""),
        "Invalid control character at: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err(r#""abc"#),
        "Unterminated string starting at: line 1 column 1 (char 0)"
    );
    assert_eq!(
        parse_err("\"a\\"),
        "Unterminated string starting at: line 1 column 1 (char 0)"
    );
    assert_eq!(
        parse_err(r#""\x""#),
        "Invalid \\escape: line 1 column 2 (char 1)"
    );
    assert_eq!(
        parse_err(r#""\u12""#),
        "Invalid \\uXXXX escape: line 1 column 3 (char 2)"
    );
    assert_eq!(
        parse_err(r#""\u12zz""#),
        "Invalid \\uXXXX escape: line 1 column 3 (char 2)"
    );
}

#[test]
fn parse_error_line_tracking() {
    assert_eq!(
        parse_err("{\n  \"a\": oops\n}"),
        "Expecting value: line 2 column 8 (char 9)"
    );
}

// -- serialization ------------------------------------------------------------

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[test]
fn compact_separators() {
    let value = obj(vec![
        ("b", Value::from(1)),
        ("a", Value::from("é\n\t\u{7}")),
    ]);
    assert_eq!(
        compact(&value),
        "{\"b\": 1, \"a\": \"\\u00e9\\n\\t\\u0007\"}"
    );
}

#[test]
fn indented_output_with_sorted_keys() {
    let value = obj(vec![
        ("b", Value::from(1)),
        (
            "a",
            Value::Array(vec![
                Value::from(1.5),
                Value::from("x"),
                Value::Null,
                Value::Bool(true),
            ]),
        ),
        ("c", obj(vec![("d", Value::from("é"))])),
    ]);
    let text = dumps(
        &value,
        &DumpOptions {
            indent: Some(4),
            sort_keys: true,
            ensure_ascii: true,
        },
    );
    assert_eq!(
        text,
        "{\n    \"a\": [\n        1.5,\n        \"x\",\n        null,\n        true\n    ],\n    \"b\": 1,\n    \"c\": {\n        \"d\": \"\\u00e9\"\n    }\n}"
    );
}

#[test]
fn empty_containers_stay_inline() {
    let value = obj(vec![
        ("a", Value::Object(vec![])),
        ("b", Value::Array(vec![])),
    ]);
    let text = dumps(
        &value,
        &DumpOptions {
            indent: Some(4),
            ..DumpOptions::default()
        },
    );
    assert_eq!(text, "{\n    \"a\": {},\n    \"b\": []\n}");
}

#[test]
fn ascii_escaping_modes() {
    let value = obj(vec![
        ("emoji", Value::from("🎉")),
        ("cjk", Value::from("漢")),
    ]);
    assert_eq!(
        compact(&value),
        "{\"emoji\": \"\\ud83c\\udf89\", \"cjk\": \"\\u6f22\"}"
    );
    let raw = dumps(
        &value,
        &DumpOptions {
            ensure_ascii: false,
            ..DumpOptions::default()
        },
    );
    assert_eq!(raw, r#"{"emoji": "🎉", "cjk": "漢"}"#);
}

#[test]
fn del_is_escaped_only_in_ascii_mode() {
    let value = Value::from("\u{7f}\u{80}");
    assert_eq!(compact(&value), "\"\\u007f\\u0080\"");
    let raw = dumps(
        &value,
        &DumpOptions {
            ensure_ascii: false,
            ..DumpOptions::default()
        },
    );
    assert_eq!(raw, "\"\u{7f}\u{80}\"");
}

#[test]
fn special_floats_serialize_like_the_reference() {
    let value = Value::Array(vec![
        Value::from(f64::NAN),
        Value::from(f64::INFINITY),
        Value::from(f64::NEG_INFINITY),
    ]);
    assert_eq!(compact(&value), "[NaN, Infinity, -Infinity]");
}

#[test]
#[allow(clippy::excessive_precision, clippy::approx_constant)]
fn float_rendering_matches_reference_repr() {
    // Ground truth generated with the reference implementation; literals
    // are verbatim vectors, precision quirks included.
    let cases: &[(f64, &str)] = &[
        (1.5, "1.5"),
        (0.1, "0.1"),
        (1e15, "1000000000000000.0"),
        (1e16, "1e+16"),
        (1e20, "1e+20"),
        (1.5e-7, "1.5e-07"),
        (1e-4, "0.0001"),
        (9.999e-5, "9.999e-05"),
        (123456789.123456789, "123456789.12345679"),
        (2.0, "2.0"),
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (3.141592653589793, "3.141592653589793"),
        (1.7976931348623157e308, "1.7976931348623157e+308"),
        (5e-324, "5e-324"),
        (100.0, "100.0"),
        (1e100, "1e+100"),
        (2.5e-10, "2.5e-10"),
        (6.02e23, "6.02e+23"),
        (1234567890123456.0, "1234567890123456.0"),
        (12345678901234567.0, "1.2345678901234568e+16"),
        (-1.5, "-1.5"),
        (-1e20, "-1e+20"),
    ];
    for (f, expected) in cases {
        assert_eq!(
            compact(&Value::from(*f)),
            *expected,
            "float {f} rendered wrong"
        );
    }
}

#[test]
fn roundtrip_preserves_number_identity() {
    let value = parse("[1, 1.0, 1e2, 0.5, -7, 123456789012345678901234567890]").unwrap();
    assert_eq!(
        compact(&value),
        "[1, 1.0, 100.0, 0.5, -7, 123456789012345678901234567890]"
    );
}

#[test]
fn number_accessors() {
    assert_eq!(Number::from_i64(42).as_i64(), Some(42));
    assert_eq!(Number::from_f64(1.5).as_i64(), None);
    assert_eq!(Number::from_f64(1.5).as_f64(), 1.5);
    let big = parse("123456789012345678901234567890").unwrap();
    if let Value::Number(n) = big {
        assert_eq!(n.as_i64(), None);
        assert!(n.is_int());
    } else {
        panic!("expected number");
    }
}

#[test]
fn deep_nesting_roundtrip() {
    let text = r#"{"a": [{"b": [1, 2, {"c": "d"}]}, null, true], "e": {"f": {}}}"#;
    assert_eq!(compact(&parse(text).unwrap()), text);
}
