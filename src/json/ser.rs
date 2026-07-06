//! JSON serialization with the reference tool's output conventions.

use super::{NumberRepr, Value};

/// Serialization style.
///
/// The defaults mirror the reference's serializer defaults: compact-ish
/// separators (`", "` / `": "`), ASCII escaping on, insertion order kept.
#[derive(Debug, Clone)]
pub struct DumpOptions {
    /// `Some(n)` pretty-prints with `n`-space indentation (item separator
    /// becomes a bare comma before each newline); `None` writes a single
    /// line with `", "` between items.
    pub indent: Option<usize>,
    /// Sort object keys (recursively, stable for duplicates).
    pub sort_keys: bool,
    /// Escape all characters outside printable ASCII as `\uXXXX`.
    pub ensure_ascii: bool,
}

impl Default for DumpOptions {
    fn default() -> Self {
        DumpOptions {
            indent: None,
            sort_keys: false,
            ensure_ascii: true,
        }
    }
}

pub fn dumps(value: &Value, options: &DumpOptions) -> String {
    let mut out = String::new();
    write_value(&mut out, value, options, 0);
    out
}

fn write_value(out: &mut String, value: &Value, options: &DumpOptions, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => match &number.0 {
            NumberRepr::Int(text) => out.push_str(text),
            NumberRepr::Float(f) => out.push_str(&render_float(*f)),
        },
        Value::String(s) => write_string(out, s, options.ensure_ascii),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    push_item_separator(out, options);
                }
                push_newline_indent(out, options, depth + 1);
                write_value(out, item, options, depth + 1);
            }
            push_newline_indent(out, options, depth);
            out.push(']');
        }
        Value::Object(pairs) => {
            if pairs.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut ordered: Vec<&(String, Value)> = pairs.iter().collect();
            if options.sort_keys {
                ordered.sort_by(|a, b| a.0.cmp(&b.0));
            }
            out.push('{');
            for (i, (key, item)) in ordered.into_iter().enumerate() {
                if i > 0 {
                    push_item_separator(out, options);
                }
                push_newline_indent(out, options, depth + 1);
                write_string(out, key, options.ensure_ascii);
                out.push_str(": ");
                write_value(out, item, options, depth + 1);
            }
            push_newline_indent(out, options, depth);
            out.push('}');
        }
    }
}

fn push_item_separator(out: &mut String, options: &DumpOptions) {
    if options.indent.is_some() {
        out.push(',');
    } else {
        out.push_str(", ");
    }
}

fn push_newline_indent(out: &mut String, options: &DumpOptions, depth: usize) {
    if let Some(width) = options.indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', width * depth));
    }
}

fn write_string(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                push_u_escape(out, c as u32);
            }
            // ASCII mode escapes everything outside space..tilde,
            // including DEL; astral characters use surrogate pairs.
            c if ensure_ascii && !(0x20..=0x7e).contains(&(c as u32)) => {
                let code = c as u32;
                if code > 0xFFFF {
                    let reduced = code - 0x10000;
                    push_u_escape(out, 0xD800 + (reduced >> 10));
                    push_u_escape(out, 0xDC00 + (reduced & 0x3FF));
                } else {
                    push_u_escape(out, code);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_u_escape(out: &mut String, code: u32) {
    use std::fmt::Write;
    let _ = write!(out, "\\u{code:04x}");
}

/// Render a float the way the reference language's `repr` does: shortest
/// round-trip digits, fixed notation for decimal exponents in `-4..16`,
/// otherwise scientific with a signed, two-digit-minimum exponent.
fn render_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }

    let (negative, digits, exponent) = shortest_decimal(f);
    let sign = if negative { "-" } else { "" };

    if (-4..16).contains(&exponent) {
        // Fixed notation, always with a fractional part.
        if exponent >= 0 {
            let exponent = exponent as usize;
            let mut int_part: String = digits.clone();
            while int_part.len() <= exponent {
                int_part.push('0');
            }
            let frac = &int_part[exponent + 1..];
            let int = &int_part[..exponent + 1];
            if frac.is_empty() {
                format!("{sign}{int}.0")
            } else {
                format!("{sign}{int}.{frac}")
            }
        } else {
            let zeros = "0".repeat((-exponent - 1) as usize);
            format!("{sign}0.{zeros}{digits}")
        }
    } else {
        let mantissa = if digits.len() > 1 {
            format!("{}.{}", &digits[..1], &digits[1..])
        } else {
            digits
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{exp_sign}{:02}", exponent.abs())
    }
}

/// Shortest round-trip decimal form of a finite float, as
/// (negative, significant digits, decimal exponent), where the value is
/// `0.digits × 10^(exponent+1)` — i.e. `digits[0].digits[1..] × 10^exponent`.
fn shortest_decimal(f: f64) -> (bool, String, i32) {
    // The standard formatter already produces shortest round-trip output;
    // reduce it to digits and exponent.
    let repr = format!("{f:?}");
    let negative = repr.starts_with('-');
    let repr = repr.trim_start_matches('-');
    let (mantissa, exp_part) = match repr.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (repr, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let all_digits: String = format!("{int_part}{frac_part}");
    // Position of the first significant digit relative to the decimal point.
    let leading_zeros = all_digits.chars().take_while(|&c| c == '0').count();
    let digits: String = all_digits[leading_zeros..]
        .trim_end_matches('0')
        .to_string();
    if digits.is_empty() {
        // Zero (of either sign).
        return (negative, "0".to_string(), 0);
    }
    let point = int_part.len() as i32;
    let exponent = point - 1 - leading_zeros as i32 + exp_part;
    (negative, digits, exponent)
}
