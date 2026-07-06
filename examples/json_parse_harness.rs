//! Dev harness: parse each stdin line as JSON and echo the outcome.
//!
//! Output per line: `{"ok": "<compact re-serialization>"}` or
//! `{"error": "<message>"}`.

use std::io::BufRead;

use furl::json::{DumpOptions, dumps, parse};

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("stdin read");
        let input: serde_json::Value = serde_json::from_str(&line).expect("wrapper line");
        let text = input["text"].as_str().expect("text field");
        let output = match parse(text) {
            Ok(value) => {
                serde_json::json!({"ok": dumps(&value, &DumpOptions::default())})
            }
            Err(error) => serde_json::json!({"error": error.to_string()}),
        };
        println!("{output}");
    }
}
