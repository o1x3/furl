//! Splits a data-item key into bracket and literal tokens.
//!
//! Special characters are `[`, `]`, and `\`. A backslash escapes a
//! following special character; before any other character it is kept
//! literally together with that character (there are no C-style escapes).

use super::Span;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    LBracket,
    RBracket,
    /// A literal run that reads as an integer.
    Number {
        /// Parsed value, saturated at the `i64` range; the interpreter
        /// rejects out-of-range indexes.
        value: i64,
        /// Canonical decimal rendering (sign, no leading zeros), used when
        /// a number serves as an object key.
        text: String,
    },
    /// Any other literal run.
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub(crate) fn tokenize(key: &str) -> Vec<Token> {
    let chars: Vec<char> = key.chars().collect();
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut buf_start = 0usize;
    let mut has_special_escape = false;
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                if buf.is_empty() {
                    buf_start = i;
                }
                match chars.get(i + 1) {
                    Some(&next @ ('[' | ']' | '\\')) => {
                        buf.push(next);
                        has_special_escape = true;
                        i += 2;
                    }
                    Some(&next) => {
                        buf.push('\\');
                        buf.push(next);
                        i += 2;
                    }
                    // A trailing lone backslash escapes nothing; keep it
                    // as literal data.
                    None => {
                        buf.push('\\');
                        i += 1;
                    }
                }
            }
            c @ ('[' | ']') => {
                flush(&mut tokens, &mut buf, buf_start, i, &mut has_special_escape);
                let kind = if c == '[' {
                    TokenKind::LBracket
                } else {
                    TokenKind::RBracket
                };
                tokens.push(Token {
                    kind,
                    span: Span {
                        start: i,
                        end: i + 1,
                    },
                });
                i += 1;
            }
            c => {
                if buf.is_empty() {
                    buf_start = i;
                }
                buf.push(c);
                i += 1;
            }
        }
    }
    flush(
        &mut tokens,
        &mut buf,
        buf_start,
        chars.len(),
        &mut has_special_escape,
    );
    tokens
}

fn flush(
    tokens: &mut Vec<Token>,
    buf: &mut String,
    start: usize,
    end: usize,
    has_special_escape: &mut bool,
) {
    if !buf.is_empty() {
        tokens.push(Token {
            kind: classify(buf, *has_special_escape),
            span: Span { start, end },
        });
        buf.clear();
    }
    *has_special_escape = false;
}

/// Decides whether a finished literal run is a number or text.
///
/// A run containing any `\[`/`\]`/`\\` escape is always text. Otherwise a
/// run that reads as an integer is a number, and a run whose leading
/// backslash guards an integer becomes the bare digits as a string key
/// (the `\1` → key `"1"` mechanism).
fn classify(buf: &str, has_special_escape: bool) -> TokenKind {
    if has_special_escape {
        return TokenKind::Text(buf.to_string());
    }
    if let Some((value, text)) = parse_int_literal(buf) {
        return TokenKind::Number { value, text };
    }
    if let Some(rest) = buf.strip_prefix('\\') {
        if parse_int_literal(rest).is_some() {
            return TokenKind::Text(rest.to_string());
        }
    }
    TokenKind::Text(buf.to_string())
}

/// Recognizes integer literals: optional surrounding whitespace, an
/// optional sign, and ASCII digit groups separated by single underscores.
/// Returns the saturated value and the canonical decimal text.
fn parse_int_literal(s: &str) -> Option<(i64, String)> {
    let trimmed = s.trim_matches(char::is_whitespace);
    if trimmed.is_empty() {
        return None;
    }
    let (negative, digits) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let clean: String = digits.chars().filter(|&c| c != '_').collect();
    let unsigned = clean.trim_start_matches('0');
    let canonical = if unsigned.is_empty() {
        "0".to_string()
    } else if negative {
        format!("-{unsigned}")
    } else {
        unsigned.to_string()
    };
    let value = match canonical.parse::<i64>() {
        Ok(n) => n,
        Err(_) if negative => i64::MIN,
        Err(_) => i64::MAX,
    };
    Some((value, canonical))
}
