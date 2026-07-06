//! Dev harness for differential testing of the nested-JSON syntax.
//!
//! Reads one JSON document per line from stdin: an array of `[key, value]`
//! pairs. Prints one JSON document per line: `{"ok": <body>}` on success or
//! `{"error": "<rendered error>"}` on failure.

use std::io::BufRead;

use furl::cli::nested_json::NestedJson;
use furl::json::{DumpOptions, Value, dumps, parse};

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin read");
        if line.trim().is_empty() {
            continue;
        }
        let Value::Array(pairs) = parse(&line).expect("input line must be a JSON array") else {
            panic!("input line must be a JSON array of pairs");
        };
        let mut nested = NestedJson::new();
        let mut error = None;
        for pair in pairs {
            let Value::Array(mut pair) = pair else {
                panic!("each pair must be a two-element array");
            };
            assert_eq!(pair.len(), 2, "each pair must be a two-element array");
            let value = pair.pop().expect("value");
            let Value::String(key) = pair.pop().expect("key") else {
                panic!("pair keys must be strings");
            };
            if let Err(e) = nested.assign(&key, value) {
                error = Some(e);
                break;
            }
        }
        let output = match error {
            Some(e) => Value::Object(vec![("error".to_string(), Value::from(e.to_string()))]),
            None => Value::Object(vec![("ok".to_string(), nested.finish())]),
        };
        println!("{}", dumps(&output, &DumpOptions::default()));
    }
}
