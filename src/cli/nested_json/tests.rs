use serde_json::{Value, json};

use super::{NestedJson, NestedJsonError};

fn build(pairs: &[(&str, Value)]) -> Result<Value, NestedJsonError> {
    let mut nested = NestedJson::new();
    for (key, value) in pairs {
        nested.assign(key, value.clone())?;
    }
    Ok(nested.finish())
}

#[track_caller]
fn ok(pairs: &[(&str, Value)]) -> Value {
    match build(pairs) {
        Ok(value) => value,
        Err(error) => panic!("expected success, got error:\n{error}"),
    }
}

#[track_caller]
fn err(pairs: &[(&str, Value)]) -> String {
    match build(pairs) {
        Ok(value) => panic!("expected error, built {value}"),
        Err(error) => error.to_string(),
    }
}

#[track_caller]
fn err_message(pairs: &[(&str, Value)]) -> String {
    err(pairs).lines().next().unwrap().to_string()
}

#[test]
fn empty_batch_is_empty_object() {
    assert_eq!(ok(&[]), json!({}));
}

#[test]
fn append_preserves_order() {
    assert_eq!(
        ok(&[
            ("bottle-on-wall[]", json!(1)),
            ("bottle-on-wall[]", json!(2)),
            ("bottle-on-wall[]", json!(3)),
        ]),
        json!({"bottle-on-wall": [1, 2, 3]})
    );
}

#[test]
fn object_keys_and_out_of_order_indexes() {
    assert_eq!(
        ok(&[
            ("pet[species]", json!("Dahut")),
            ("pet[name]", json!("Hypatia")),
            ("kids[1]", json!("Thelma")),
            ("kids[0]", json!("Ashley")),
        ]),
        json!({
            "pet": {"species": "Dahut", "name": "Hypatia"},
            "kids": ["Ashley", "Thelma"],
        })
    );
}

#[test]
fn array_of_objects_via_explicit_indexes() {
    assert_eq!(
        ok(&[
            ("pet[0][species]", json!("Dahut")),
            ("pet[0][name]", json!("Hypatia")),
            ("pet[1][species]", json!("Felis Stultus")),
            ("pet[1][name]", json!("Billie")),
        ]),
        json!({
            "pet": [
                {"species": "Dahut", "name": "Hypatia"},
                {"species": "Felis Stultus", "name": "Billie"},
            ]
        })
    );
}

#[test]
fn deep_mixed_nesting_with_sparse_fill() {
    assert_eq!(
        ok(&[("wow[such][deep][3][much][power][!]", json!("Amaze"))]),
        json!({
            "wow": {
                "such": {
                    "deep": [null, null, null, {"much": {"power": {"!": "Amaze"}}}]
                }
            }
        })
    );
}

#[test]
fn append_and_sparse_index_interplay() {
    assert_eq!(
        ok(&[
            ("mix[]", json!("scalar")),
            ("mix[2]", json!("x")),
            ("mix[4]", json!(1)),
        ]),
        json!({"mix": ["scalar", null, "x", null, 1]})
    );
}

#[test]
fn single_element_append() {
    assert_eq!(
        ok(&[("highlander[]", json!("one"))]),
        json!({"highlander": ["one"]})
    );
}

#[test]
fn escaped_bracket_makes_literal_top_level_key() {
    assert_eq!(
        ok(&[("error[good]", json!("god")), (r"error\[bad", json!(1)),]),
        json!({"error": {"good": "god"}, "error[bad": 1})
    );
}

#[test]
fn json_literals_via_raw_values() {
    assert_eq!(
        ok(&[
            ("special[]", json!(true)),
            ("special[]", json!(false)),
            ("special[]", json!("true")),
            ("special[]", json!(null)),
        ]),
        json!({"special": [true, false, "true", null]})
    );
}

#[test]
fn escaped_brackets_in_root_and_bracketed_positions() {
    assert_eq!(
        ok(&[
            (r"\[\]", json!(1)),
            (r"escape\[d\]", json!(2)),
            (r"escaped\[\]", json!(3)),
            (r"e\[s\][c][a][p][\[ed\]][]", json!(4)),
        ]),
        json!({
            "[]": 1,
            "escape[d]": 2,
            "escaped[]": 3,
            "e[s]": {"c": {"a": {"p": {"[ed]": [4]}}}},
        })
    );
}

#[test]
fn top_level_array() {
    assert_eq!(
        ok(&[("[]", json!(1)), ("[]", json!("foo"))]),
        json!([1, "foo"])
    );
}

#[test]
fn escaped_bracket_only_keys() {
    assert_eq!(
        ok(&[
            (r"\]", json!(1)),
            (r"\[\]1", json!(2)),
            (r"\[1\]\]", json!(3)),
        ]),
        json!({"]": 1, "[]1": 2, "[1]]": 3})
    );
}

#[test]
fn escape_placement_decides_structure_vs_data() {
    assert_eq!(
        ok(&[
            (r"foo\[bar\][baz]", json!(1)),
            (r"foo\[bar\]\[baz\]", json!(3)),
            (r"foo[bar][\[baz\]]", json!(4)),
        ]),
        json!({
            "foo[bar]": {"baz": 1},
            "foo[bar][baz]": 3,
            "foo": {"bar": {"[baz]": 4}},
        })
    );
}

#[test]
fn each_append_chain_appends_fresh_nested_arrays() {
    assert_eq!(
        ok(&[
            ("key[]", json!(1)),
            ("key[][]", json!(2)),
            ("key[][][]", json!(3)),
            ("key[][][]", json!(4)),
        ]),
        json!({"key": [1, [2], [[3]], [[4]]]})
    );
}

#[test]
fn append_continues_at_current_end() {
    assert_eq!(
        ok(&[
            ("x[0]", json!(1)),
            ("x[]", json!(2)),
            ("x[]", json!(3)),
            ("x[][]", json!(4)),
            ("x[][]", json!(5)),
        ]),
        json!({"x": [1, 2, 3, [4], [5]]})
    );
}

#[test]
fn sparse_fill_then_appends_and_wholesale_array_extension() {
    assert_eq!(
        ok(&[
            ("foo[bar][5][]", json!(5)),
            ("foo[bar][][x]", json!("y")),
            ("foo[baz]", json!([1, 2, 3])),
            ("foo[baz][]", json!(4)),
        ]),
        json!({
            "foo": {
                "bar": [null, null, null, null, null, [5], {"x": "y"}],
                "baz": [1, 2, 3, 4],
            }
        })
    );
}

#[test]
fn extend_appended_object_by_explicit_index() {
    assert_eq!(
        ok(&[
            ("foo[]", json!(1)),
            ("foo[]", json!(2)),
            ("foo[][key]", json!("value")),
            ("foo[2][key 2]", json!("value 2")),
            (r"foo[2][key \[]", json!("value 3")),
            (r"bar[nesting][under][!][empty][?][\\key]", json!(4)),
        ]),
        json!({
            "foo": [1, 2, {"key": "value", "key 2": "value 2", "key [": "value 3"}],
            "bar": {"nesting": {"under": {"!": {"empty": {"?": {"\\key": 4}}}}}},
        })
    );
}

#[test]
fn heavy_escape_torture() {
    assert_eq!(
        ok(&[
            (r"foo\[key\]", json!(1)),
            (r"bar\[1\]", json!(2)),
            (r"quux[key\[escape\]]", json!(4)),
            (r"quux[key 2][\\][\\\\][\\\[\]\\\]\\\[\n\\]", json!(5)),
        ]),
        json!({
            "foo[key]": 1,
            "bar[1]": 2,
            "quux": {
                "key[escape]": 4,
                "key 2": {"\\": {"\\\\": {"\\[]\\]\\[\\n\\": 5}}},
            },
        })
    );
}

#[test]
fn backslash_before_non_special_is_literal() {
    assert_eq!(
        ok(&[
            (r"A[B\\]", json!("C1")),
            (r"D[E\\\\]", json!("C2")),
            (r"F[\B\\]", json!("C3")),
        ]),
        json!({
            "A": {"B\\": "C1"},
            "D": {"E\\\\": "C2"},
            "F": {"\\B\\": "C3"},
        })
    );
}

#[test]
fn kitchen_sink_document_example() {
    assert_eq!(
        ok(&[
            ("name", json!("python")),
            ("version", json!(3)),
            ("date[year]", json!(2021)),
            ("date[month]", json!("December")),
            ("systems[]", json!("Linux")),
            ("systems[]", json!("Mac")),
            ("systems[]", json!("Windows")),
            ("people[known_ids][1]", json!(1000)),
            ("people[known_ids][5]", json!(5000)),
        ]),
        json!({
            "name": "python",
            "version": 3,
            "date": {"year": 2021, "month": "December"},
            "systems": ["Linux", "Mac", "Windows"],
            "people": {"known_ids": [null, 1000, null, null, null, 5000]},
        })
    );
}

#[test]
fn backslash_digit_forces_string_keys() {
    assert_eq!(
        ok(&[
            (r"foo[\1][type]", json!("migration")),
            (r"foo[\2][type]", json!("migration")),
        ]),
        json!({
            "foo": {"1": {"type": "migration"}, "2": {"type": "migration"}}
        })
    );
}

#[test]
fn backslash_retained_when_rest_is_not_integer() {
    assert_eq!(
        ok(&[
            (r"foo[\dates]", json!([2011, 2012])),
            (r"foo[\2012 bleh]", json!("a")),
            (r"foo[bleh \2012]", json!("b")),
            (r"foo[\dates][0]", json!(2014)),
        ]),
        json!({
            "foo": {
                r"\dates": [2014, 2012],
                r"\2012 bleh": "a",
                r"bleh \2012": "b",
            }
        })
    );
}

#[test]
fn root_backslash_int_strips_to_string_key() {
    assert_eq!(
        ok(&[
            (r"\2012[x]", json!("y")),
            (r"\1", json!("top level int")),
            (r"\\1", json!("escaped")),
            (r"\2[\3][\4]", json!(5)),
        ]),
        json!({
            "2012": {"x": "y"},
            "1": "top level int",
            r"\1": "escaped",
            "2": {"3": {"4": 5}},
        })
    );
}

#[test]
fn escaped_int_stripping_only_with_unescaped_leading_backslash() {
    assert_eq!(
        ok(&[
            (r"a[\0]", json!(0)),
            (r"a[\\1]", json!(1)),
            (r"a[\\\2]", json!(2)),
            (r"a[\\\\\3]", json!(3)),
            (r"a[-1\\]", json!(4)),
            (r"a[-2\\\\]", json!(5)),
            (r"a[\\-3\\\\]", json!(6)),
        ]),
        json!({
            "a": {
                "0": 0,
                r"\1": 1,
                r"\\2": 2,
                r"\\\3": 3,
                r"-1\": 4,
                r"-2\\": 5,
                r"\-3\\": 6,
            }
        })
    );
}

#[test]
fn top_level_array_with_sparse_fill() {
    assert_eq!(
        ok(&[
            ("[]", json!(0)),
            ("[]", json!(1)),
            ("[5]", json!(5)),
            ("[]", json!(6)),
            ("[9]", json!(9)),
        ]),
        json!([0, 1, null, null, null, 5, 6, null, null, 9])
    );
}

#[test]
fn empty_root_key_coexists_with_others() {
    assert_eq!(
        ok(&[
            ("", json!("empty")),
            ("foo", json!("bar")),
            ("bar[baz][quux]", json!("tuut")),
        ]),
        json!({
            "": "empty",
            "foo": "bar",
            "bar": {"baz": {"quux": "tuut"}},
        })
    );
}

#[test]
fn empty_root_with_raw_object_value() {
    assert_eq!(
        ok(&[("", json!({"foo": {"bar": "baz"}})), ("top", json!("val")),]),
        json!({"": {"foo": {"bar": "baz"}}, "top": "val"})
    );
}

#[test]
fn top_level_array_of_containers_reentered_by_index() {
    assert_eq!(
        ok(&[
            ("[][a][b][]", json!(1)),
            ("[0][a][b][]", json!(2)),
            ("[][]", json!(2)),
        ]),
        json!([{"a": {"b": [1, 2]}}, [2]])
    );
}

#[test]
fn raw_array_under_empty_root_is_not_a_top_level_array() {
    assert_eq!(ok(&[("", json!([1, 2, 3]))]), json!({"": [1, 2, 3]}));
    assert_eq!(
        ok(&[("", json!([1, 2, 3])), ("foo", json!("bar"))]),
        json!({"": [1, 2, 3], "foo": "bar"})
    );
}

#[test]
fn sparse_fill_to_exact_length() {
    let value = ok(&[("test[0]", json!(1)), ("test[100]", json!(1))]);
    assert_eq!(value["test"].as_array().unwrap().len(), 101);
}

#[test]
fn root_number_coerces_to_string_key() {
    assert_eq!(ok(&[("5[x]", json!("y"))]), json!({"5": {"x": "y"}}));
    assert_eq!(ok(&[("007[x]", json!("y"))]), json!({"7": {"x": "y"}}));
}

#[test]
fn index_literal_quirks() {
    assert_eq!(ok(&[("a[ 5 ]", json!("v"))])["a"][5], json!("v"));
    assert_eq!(ok(&[("a[1_0]", json!("v"))])["a"][10], json!("v"));
    assert_eq!(ok(&[("a[+5]", json!("v"))])["a"][5], json!("v"));
    assert_eq!(ok(&[("a[1.5]", json!("v"))]), json!({"a": {"1.5": "v"}}));
}

#[test]
fn last_write_wins() {
    assert_eq!(
        ok(&[("a[0]", json!("v")), ("a[0]", json!("w"))]),
        json!({"a": ["w"]})
    );
}

#[test]
fn descending_through_null_replaces_it() {
    // Deviation from the reference implementation, which rebinds the whole
    // context (discarding earlier data) on key access through null.
    assert_eq!(
        ok(&[("a", json!(null)), ("a[x]", json!(1)), ("b", json!(2))]),
        json!({"a": {"x": 1}, "b": 2})
    );
}

#[test]
fn trailing_lone_backslash_is_literal() {
    // Deviation: the reference implementation crashes on this input.
    assert_eq!(ok(&[(r"a\", json!(1))]), json!({"a\\": 1}));
    assert_eq!(ok(&[(r"\", json!(1))]), json!({"\\": 1}));
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn syntax_error_unterminated_root_bracket() {
    assert_eq!(
        err(&[("A[", json!(1))]),
        "furl Syntax Error: Expecting a text, a number or ']'\nA[\n  ^"
    );
}

#[test]
fn syntax_error_missing_close_after_number() {
    assert_eq!(
        err(&[("A[1", json!(1))]),
        "furl Syntax Error: Expecting ']'\nA[1\n   ^"
    );
}

#[test]
fn syntax_error_missing_close_after_text() {
    assert_eq!(
        err(&[("A[text", json!(1))]),
        "furl Syntax Error: Expecting ']'\nA[text\n      ^"
    );
}

#[test]
fn syntax_error_unterminated_second_bracket() {
    assert_eq!(
        err(&[("A[text][", json!(1))]),
        "furl Syntax Error: Expecting a text, a number or ']'\nA[text][\n        ^"
    );
}

#[test]
fn syntax_error_reports_only_the_offending_key() {
    assert_eq!(
        err(&[("fine[ok]", json!(1)), ("A[text][", json!(1))]),
        "furl Syntax Error: Expecting a text, a number or ']'\nA[text][\n        ^"
    );
}

#[test]
fn syntax_error_trailing_garbage_after_group() {
    assert_eq!(
        err(&[("A[text]1", json!(1))]),
        "furl Syntax Error: Expecting '['\nA[text]1\n       ^"
    );
}

#[test]
fn syntax_error_escaped_open_then_stray_close() {
    assert_eq!(
        err(&[(r"A\[]", json!(1))]),
        "furl Syntax Error: Expecting '['\nA\\[]\n   ^"
    );
}

#[test]
fn syntax_error_escaped_close_is_data() {
    assert_eq!(
        err(&[(r"A[something\]", json!(1))]),
        "furl Syntax Error: Expecting ']'\nA[something\\]\n             ^"
    );
}

#[test]
fn syntax_error_columns_count_escape_characters() {
    assert_eq!(
        err(&[(r"foo\[bar\]\\[   bleh", json!(1))]),
        "furl Syntax Error: Expecting ']'\nfoo\\[bar\\]\\\\[   bleh\n                    ^"
    );
}

#[test]
fn syntax_error_trailing_spaces_are_data() {
    assert_eq!(
        err(&[(r"foo\[bar\]\\[   bleh   ", json!(1))]),
        "furl Syntax Error: Expecting ']'\nfoo\\[bar\\]\\\\[   bleh   \n                       ^"
    );
}

#[test]
fn syntax_error_stray_close_after_group() {
    assert_eq!(
        err(&[("foo[bar][1]][]", json!(1))]),
        "furl Syntax Error: Expecting '['\nfoo[bar][1]][]\n           ^"
    );
}

#[test]
fn syntax_error_wide_caret_over_literal() {
    assert_eq!(
        err(&[("foo[bar][1]something[]", json!(1))]),
        "furl Syntax Error: Expecting '['\nfoo[bar][1]something[]\n           ^^^^^^^^^"
    );
}

#[test]
fn syntax_error_open_bracket_inside_group() {
    assert_eq!(
        err(&[("foo[bar][1][142241[]", json!(1))]),
        "furl Syntax Error: Expecting ']'\nfoo[bar][1][142241[]\n                  ^"
    );
}

#[test]
fn syntax_error_escaped_literal_span_includes_escapes() {
    assert_eq!(
        err(&[(r"foo[bar][1]\[142241[]", json!(1))]),
        "furl Syntax Error: Expecting '['\nfoo[bar][1]\\[142241[]\n           ^^^^^^^^"
    );
}

#[test]
fn type_error_key_access_on_string() {
    assert_eq!(
        err(&[("foo", json!("1")), ("foo[key]", json!(2))]),
        "furl Type Error: Cannot perform 'key' based access on 'foo' \
         which has a type of 'string' but this operation requires a type of 'object'.\
         \nfoo[key]\n   ^^^^^"
    );
}

#[test]
fn type_error_index_access_on_string() {
    assert_eq!(
        err(&[("foo", json!("1")), ("foo[0]", json!(2))]),
        "furl Type Error: Cannot perform 'index' based access on 'foo' \
         which has a type of 'string' but this operation requires a type of 'array'.\
         \nfoo[0]\n   ^^^"
    );
}

#[test]
fn type_error_append_access_on_string() {
    assert_eq!(
        err(&[("foo", json!("1")), ("foo[]", json!(2))]),
        "furl Type Error: Cannot perform 'append' based access on 'foo' \
         which has a type of 'string' but this operation requires a type of 'array'.\
         \nfoo[]\n   ^^"
    );
}

#[test]
fn type_error_index_access_on_object() {
    assert_eq!(
        err_message(&[
            ("data[key]", json!("dasd")),
            ("data[0]", json!("something"))
        ]),
        "furl Type Error: Cannot perform 'index' based access on 'data' \
         which has a type of 'object' but this operation requires a type of 'array'."
    );
}

#[test]
fn type_error_append_access_on_object() {
    assert_eq!(
        err_message(&[("data[key]", json!("dasd")), ("data[]", json!("something"))]),
        "furl Type Error: Cannot perform 'append' based access on 'data' \
         which has a type of 'object' but this operation requires a type of 'array'."
    );
}

#[test]
fn type_error_key_access_on_nested_array_names_prefix() {
    assert_eq!(
        err(&[
            ("foo[bar][baz][5]", json!([1, 2, 3])),
            ("foo[bar][baz][5][]", json!(4)),
            ("foo[bar][baz][key][]", json!(5)),
        ]),
        "furl Type Error: Cannot perform 'key' based access on 'foo[bar][baz]' \
         which has a type of 'array' but this operation requires a type of 'object'.\
         \nfoo[bar][baz][key][]\n             ^^^^^"
    );
}

#[test]
fn value_error_negative_index() {
    assert_eq!(
        err(&[("foo[-10]", json!([1, 2]))]),
        "furl Value Error: Negative indexes are not supported.\nfoo[-10]\n    ^^^"
    );
}

#[test]
fn type_error_span_includes_escaped_key() {
    assert_eq!(
        err(&[("foo", json!([1, 2])), (r"foo[\2]", json!(3))]),
        "furl Type Error: Cannot perform 'key' based access on 'foo' \
         which has a type of 'array' but this operation requires a type of 'object'.\
         \nfoo[\\2]\n   ^^^^"
    );
}

#[test]
fn type_error_index_on_object_created_by_forced_string_key() {
    assert_eq!(
        err_message(&[(r"foo[\1]", json!(2)), ("foo[5]", json!(3))]),
        "furl Type Error: Cannot perform 'index' based access on 'foo' \
         which has a type of 'object' but this operation requires a type of 'array'."
    );
}

#[test]
fn type_error_append_on_root_object() {
    assert_eq!(
        err_message(&[("x", json!("y")), ("[]", json!(2))]),
        "furl Type Error: Cannot perform 'append' based access on '' \
         which has a type of 'object' but this operation requires a type of 'array'."
    );
}

#[test]
fn type_error_key_on_root_array() {
    assert_eq!(
        err_message(&[("[]", json!(2)), ("x", json!("y"))]),
        "furl Type Error: Cannot perform 'key' based access on '' \
         which has a type of 'array' but this operation requires a type of 'object'."
    );
}

#[test]
fn type_error_append_on_object_holding_raw_array() {
    assert_eq!(
        err_message(&[("", json!([1, 2, 3])), ("[]", json!(4))]),
        "furl Type Error: Cannot perform 'append' based access on '' \
         which has a type of 'object' but this operation requires a type of 'array'."
    );
}

#[test]
fn type_error_empty_root_key_on_array_has_no_caret_block() {
    assert_eq!(
        err(&[("[]", json!(4)), ("", json!([1, 2, 3]))]),
        "furl Type Error: Cannot perform 'key' based access on '' \
         which has a type of 'array' but this operation requires a type of 'object'."
    );
}

#[test]
fn type_error_actual_type_names() {
    // Deviation: the reference leaks Python type names ('bool', 'NoneType');
    // we use JSON names.
    assert_eq!(
        err_message(&[("a", json!(true)), ("a[x]", json!(1))]),
        "furl Type Error: Cannot perform 'key' based access on 'a' \
         which has a type of 'boolean' but this operation requires a type of 'object'."
    );
    assert_eq!(
        err_message(&[("a", json!(1)), ("a[x]", json!(1))]),
        "furl Type Error: Cannot perform 'key' based access on 'a' \
         which has a type of 'number' but this operation requires a type of 'object'."
    );
}

#[test]
fn value_error_absurd_index() {
    // Deviation: the reference attempts the allocation (and dies on OOM);
    // we fail cleanly.
    assert_eq!(
        err_message(&[("a[9223372036854775807]", json!(1))]),
        "furl Value Error: Index is too large."
    );
}
