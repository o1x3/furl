//! Dev harness: verify float rendering against reference `repr` vectors.
//!
//! Reads JSONL lines `{"bits": "<little-endian f64 hex>", "repr": "<expected>"}`
//! from stdin and reports mismatches.

use std::io::BufRead;

use furl::json::{DumpOptions, Value, dumps};

fn main() {
    let stdin = std::io::stdin();
    let mut checked = 0u64;
    let mut failed = 0u64;
    for line in stdin.lock().lines() {
        let line = line.expect("stdin read");
        if line.trim().is_empty() {
            continue;
        }
        let record: serde_json::Value = serde_json::from_str(&line).expect("valid vector line");
        let hex = record["bits"].as_str().expect("bits field");
        let expected = record["repr"].as_str().expect("repr field");
        let mut bytes = [0u8; 8];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
        }
        let f = f64::from_le_bytes(bytes);
        let rendered = dumps(&Value::from(f), &DumpOptions::default());
        // Python repr says nan/inf; json.dumps says NaN/Infinity — vectors
        // use repr, so translate.
        let expected = match expected {
            "nan" => "NaN".to_string(),
            "inf" => "Infinity".to_string(),
            "-inf" => "-Infinity".to_string(),
            other => other.to_string(),
        };
        checked += 1;
        if rendered != expected {
            failed += 1;
            if failed <= 20 {
                println!("MISMATCH bits={hex} expected={expected} got={rendered}");
            }
        }
    }
    println!("checked={checked} failed={failed}");
    std::process::exit(if failed > 0 { 1 } else { 0 });
}
