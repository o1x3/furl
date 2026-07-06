//! `application/x-www-form-urlencoded` percent-encoding.
//!
//! The rules match the reference stack's form encoder: space becomes `+`,
//! ASCII letters, digits, and `_.-~` pass through, and everything else is
//! percent-encoded from its UTF-8 bytes with uppercase hex.

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-' | b'~')
}

/// Encode one form key or value.
pub fn quote_plus(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &byte in text.as_bytes() {
        if byte == b' ' {
            out.push('+');
        } else if is_unreserved(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Encode `(key, value)` pairs as a form body / query string.
///
/// Repeated keys group together: keys emit in first-occurrence order,
/// each with all its values in their own order (multi-map iteration).
pub fn urlencode(pairs: &[(String, String)]) -> String {
    let mut key_order: Vec<&String> = Vec::new();
    for (key, _) in pairs {
        if !key_order.contains(&key) {
            key_order.push(key);
        }
    }
    let mut encoded: Vec<String> = Vec::new();
    for key in key_order {
        for (k, value) in pairs {
            if k == key {
                encoded.push(format!("{}={}", quote_plus(k), quote_plus(value)));
            }
        }
    }
    encoded.join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_become_plus() {
        assert_eq!(quote_plus("a b"), "a+b");
    }

    #[test]
    fn unreserved_pass_through() {
        assert_eq!(quote_plus("Az09_.-~"), "Az09_.-~");
    }

    #[test]
    fn everything_else_percent_encodes_utf8() {
        assert_eq!(quote_plus("a&b=c"), "a%26b%3Dc");
        assert_eq!(quote_plus("é"), "%C3%A9");
        assert_eq!(quote_plus("+"), "%2B");
        assert_eq!(quote_plus("line\nbreak"), "line%0Abreak");
        assert_eq!(quote_plus("/"), "%2F");
    }

    #[test]
    fn pairs_join_in_order() {
        let pairs = vec![
            ("a b".to_string(), "c d".to_string()),
            ("a b".to_string(), "2".to_string()),
            ("x".to_string(), String::new()),
        ];
        assert_eq!(urlencode(&pairs), "a+b=c+d&a+b=2&x=");
    }
}
