//! Dev harness for differential testing of the nested-JSON syntax.
//!
//! Reads one JSON document per line from stdin: an array of `[key, value]`
//! pairs. Prints one JSON document per line: `{"ok": <body>}` on success or
//! `{"error": "<rendered error>"}` on failure.

use std::io::BufRead;

use furl::cli::nested_json::NestedJson;

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin read");
        if line.trim().is_empty() {
            continue;
        }
        let pairs: Vec<(String, serde_json::Value)> =
            serde_json::from_str(&line).expect("input line must be a JSON array of pairs");
        let mut nested = NestedJson::new();
        let mut error = None;
        for (key, value) in pairs {
            if let Err(e) = nested.assign(&key, value) {
                error = Some(e);
                break;
            }
        }
        let output = match error {
            Some(e) => serde_json::json!({ "error": e.to_string() }),
            None => serde_json::json!({ "ok": nested.finish() }),
        };
        println!("{output}");
    }
}
