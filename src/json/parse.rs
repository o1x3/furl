//! A strict JSON parser with the reference tool's acceptance rules and
//! error message shapes.
//!
//! Beyond RFC 8259 it accepts the `NaN`, `Infinity`, and `-Infinity`
//! literals. Objects keep duplicate keys in order. Error positions are
//! counted in characters.

use super::{Number, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    /// 1-based line of the error.
    pub line: usize,
    /// 1-based column (in characters) within the line.
    pub column: usize,
    /// 0-based character offset in the whole input.
    pub char_index: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: line {} column {} (char {})",
            self.message, self.line, self.column, self.char_index
        )
    }
}

impl std::error::Error for ParseError {}

pub fn parse(text: &str) -> Result<Value, ParseError> {
    let chars: Vec<char> = text.chars().collect();
    let mut parser = Parser { chars, pos: 0 };
    parser.skip_whitespace();
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos < parser.chars.len() {
        return Err(parser.error("Extra data", parser.pos));
    }
    Ok(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn error(&self, message: &str, at: usize) -> ParseError {
        let mut line = 1;
        let mut line_start = 0;
        for (i, &c) in self.chars.iter().enumerate().take(at) {
            if c == '\n' {
                line += 1;
                line_start = i + 1;
            }
        }
        ParseError {
            message: message.to_string(),
            line,
            column: at - line_start + 1,
            char_index: at,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn eat_literal(&mut self, literal: &str) -> bool {
        let len = literal.chars().count();
        if self.chars[self.pos..].len() >= len
            && self.chars[self.pos..self.pos + len]
                .iter()
                .zip(literal.chars())
                .all(|(&a, b)| a == b)
        {
            self.pos += len;
            true
        } else {
            false
        }
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => Ok(Value::String(self.parse_string()?)),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some('t') if self.eat_literal("true") => Ok(Value::Bool(true)),
            Some('f') if self.eat_literal("false") => Ok(Value::Bool(false)),
            Some('n') if self.eat_literal("null") => Ok(Value::Null),
            Some('N') if self.eat_literal("NaN") => Ok(Value::Number(Number::from_f64(f64::NAN))),
            Some('I') if self.eat_literal("Infinity") => {
                Ok(Value::Number(Number::from_f64(f64::INFINITY)))
            }
            _ => Err(self.error("Expecting value", self.pos)),
        }
    }

    fn parse_object(&mut self) -> Result<Value, ParseError> {
        self.pos += 1; // '{'
        let mut pairs = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Value::Object(pairs));
        }
        loop {
            if self.peek() != Some('"') {
                return Err(self.error(
                    "Expecting property name enclosed in double quotes",
                    self.pos,
                ));
            }
            let key = self.parse_string()?;
            self.skip_whitespace();
            if self.peek() != Some(':') {
                return Err(self.error("Expecting ':' delimiter", self.pos));
            }
            self.pos += 1;
            self.skip_whitespace();
            let value = self.parse_value()?;
            pairs.push((key, value));
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    let comma = self.pos;
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() == Some('}') {
                        return Err(
                            self.error("Illegal trailing comma before end of object", comma)
                        );
                    }
                }
                Some('}') => {
                    self.pos += 1;
                    return Ok(Value::Object(pairs));
                }
                _ => return Err(self.error("Expecting ',' delimiter", self.pos)),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Value, ParseError> {
        self.pos += 1; // '['
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    let comma = self.pos;
                    self.pos += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(']') {
                        return Err(self.error("Illegal trailing comma before end of array", comma));
                    }
                }
                Some(']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("Expecting ',' delimiter", self.pos)),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.error("Unterminated string starting at", start)),
                Some('"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some('\\') => {
                    let backslash = self.pos;
                    self.pos += 1;
                    match self.peek() {
                        None => return Err(self.error("Unterminated string starting at", start)),
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('/') => out.push('/'),
                        Some('b') => out.push('\u{8}'),
                        Some('f') => out.push('\u{c}'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some('u') => {
                            self.pos += 1;
                            let unit = self.parse_hex4(backslash)?;
                            if (0xD800..0xDC00).contains(&unit) {
                                // High surrogate: try to pair it. Lone
                                // surrogates become U+FFFD (the reference
                                // language keeps them; ours cannot).
                                if self.peek() == Some('\\')
                                    && self.chars.get(self.pos + 1) == Some(&'u')
                                {
                                    let second_backslash = self.pos;
                                    self.pos += 2;
                                    let low = self.parse_hex4(second_backslash)?;
                                    if (0xDC00..0xE000).contains(&low) {
                                        let combined =
                                            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                        out.push(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                                    } else {
                                        out.push('\u{FFFD}');
                                        out.push(char::from_u32(low).unwrap_or('\u{FFFD}'));
                                    }
                                } else {
                                    out.push('\u{FFFD}');
                                }
                            } else if (0xDC00..0xE000).contains(&unit) {
                                out.push('\u{FFFD}');
                            } else {
                                out.push(char::from_u32(unit).unwrap_or('\u{FFFD}'));
                            }
                            continue; // pos already advanced past the escape
                        }
                        Some(_) => return Err(self.error("Invalid \\escape", backslash)),
                    }
                    self.pos += 1;
                }
                Some(c) if (c as u32) < 0x20 => {
                    return Err(self.error("Invalid control character at", self.pos));
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// Four hex digits after `\u`; on failure the error points just past
    /// the backslash.
    fn parse_hex4(&mut self, backslash: usize) -> Result<u32, ParseError> {
        let mut unit = 0u32;
        for _ in 0..4 {
            match self.peek().and_then(|c| c.to_digit(16)) {
                Some(d) => {
                    unit = unit * 16 + d;
                    self.pos += 1;
                }
                None => return Err(self.error("Invalid \\uXXXX escape", backslash + 1)),
            }
        }
        Ok(unit)
    }

    /// Scan the longest valid number; anything after it is left in place
    /// (surfacing later as "Extra data" or a delimiter error).
    fn parse_number(&mut self) -> Result<Value, ParseError> {
        let start = self.pos;
        let mut pos = self.pos;
        let negative = self.chars.get(pos) == Some(&'-');
        if negative {
            pos += 1;
            // `-Infinity` is accepted alongside numeric literals.
            if self.chars.get(pos) == Some(&'I') {
                self.pos = pos;
                if self.eat_literal("Infinity") {
                    return Ok(Value::Number(Number::from_f64(f64::NEG_INFINITY)));
                }
                self.pos = start;
                return Err(self.error("Expecting value", start));
            }
        }

        // Integer part: `0` alone or a nonzero digit run.
        let int_start = pos;
        match self.chars.get(pos) {
            Some('0') => pos += 1,
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.chars.get(pos), Some(c) if c.is_ascii_digit()) {
                    pos += 1;
                }
            }
            _ => return Err(self.error("Expecting value", start)),
        }
        let int_end = pos;

        // Fraction: a dot must be followed by at least one digit or the
        // number ends before it.
        let mut is_float = false;
        if self.chars.get(pos) == Some(&'.')
            && matches!(self.chars.get(pos + 1), Some(c) if c.is_ascii_digit())
        {
            is_float = true;
            pos += 2;
            while matches!(self.chars.get(pos), Some(c) if c.is_ascii_digit()) {
                pos += 1;
            }
        }

        // Exponent: `e`/`E`, optional sign, at least one digit — else the
        // number ends before it.
        if matches!(self.chars.get(pos), Some('e' | 'E')) {
            let mut exp_pos = pos + 1;
            if matches!(self.chars.get(exp_pos), Some('+' | '-')) {
                exp_pos += 1;
            }
            if matches!(self.chars.get(exp_pos), Some(c) if c.is_ascii_digit()) {
                is_float = true;
                pos = exp_pos + 1;
                while matches!(self.chars.get(pos), Some(c) if c.is_ascii_digit()) {
                    pos += 1;
                }
            }
        }

        let literal: String = self.chars[start..pos].iter().collect();
        self.pos = pos;
        if is_float {
            // Overflow saturates to infinity, like the reference.
            let f: f64 = literal.parse().unwrap_or(f64::NAN);
            Ok(Value::Number(Number::from_f64(f)))
        } else {
            let digits: String = self.chars[int_start..int_end].iter().collect();
            let canonical = if negative && digits != "0" {
                format!("-{digits}")
            } else {
                digits
            };
            Ok(Value::Number(Number::from_int_literal(canonical)))
        }
    }
}
