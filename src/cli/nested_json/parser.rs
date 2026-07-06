//! Shapes a token stream into a path: a root segment plus bracketed
//! accessors.
//!
//! Grammar: the root is a bare literal (an object key), `[n]`/`[]` (a
//! top-level array access), or empty (the `""` key). Every following
//! segment is `[text]` (key), `[n]` (index), or `[]` (append).

use super::lexer::{Token, TokenKind, tokenize};
use super::{ErrorKind, NestedJsonError, Span};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SegmentKind {
    Key(String),
    Index {
        value: i64,
        /// Span of just the number token, highlighted by index errors.
        number_span: Span,
    },
    Append,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Segment {
    pub kind: SegmentKind,
    /// Full source span of the segment, brackets and escapes included.
    /// `None` for the implicit empty root key, which has no source text.
    pub span: Option<Span>,
}

pub(crate) fn parse(key: &str) -> Result<Vec<Segment>, NestedJsonError> {
    let tokens = tokenize(key);
    let mut segments = Vec::new();
    let mut pos: usize;

    match tokens.first() {
        None => {
            segments.push(Segment {
                kind: SegmentKind::Key(String::new()),
                span: None,
            });
            return Ok(segments);
        }
        Some(token) => match &token.kind {
            TokenKind::Text(text) => {
                segments.push(Segment {
                    kind: SegmentKind::Key(text.clone()),
                    span: Some(token.span),
                });
                pos = 1;
            }
            // A bare number at the root is an object key, not an index.
            TokenKind::Number { text, .. } => {
                segments.push(Segment {
                    kind: SegmentKind::Key(text.clone()),
                    span: Some(token.span),
                });
                pos = 1;
            }
            TokenKind::LBracket => match tokens.get(1) {
                Some(Token {
                    kind: TokenKind::Number { value, .. },
                    span: number_span,
                }) => {
                    let close = expect_close(&tokens, 2, key)?;
                    segments.push(Segment {
                        kind: SegmentKind::Index {
                            value: *value,
                            number_span: *number_span,
                        },
                        span: Some(Span {
                            start: token.span.start,
                            end: close.end,
                        }),
                    });
                    pos = 3;
                }
                Some(Token {
                    kind: TokenKind::RBracket,
                    span: close,
                }) => {
                    segments.push(Segment {
                        kind: SegmentKind::Append,
                        span: Some(Span {
                            start: token.span.start,
                            end: close.end,
                        }),
                    });
                    pos = 2;
                }
                other => {
                    return Err(syntax_error(
                        "Expecting a number or ']'",
                        key,
                        span_of(other.map(|t| t.span), key),
                    ));
                }
            },
            TokenKind::RBracket => {
                return Err(syntax_error(
                    "Expecting a text, a number or '['",
                    key,
                    Some(token.span),
                ));
            }
        },
    }

    while pos < tokens.len() {
        let open = &tokens[pos];
        if !matches!(open.kind, TokenKind::LBracket) {
            return Err(syntax_error("Expecting '['", key, Some(open.span)));
        }
        pos += 1;
        match tokens.get(pos) {
            Some(Token {
                kind: TokenKind::Text(text),
                ..
            }) => {
                let close = expect_close(&tokens, pos + 1, key)?;
                segments.push(Segment {
                    kind: SegmentKind::Key(text.clone()),
                    span: Some(Span {
                        start: open.span.start,
                        end: close.end,
                    }),
                });
                pos += 2;
            }
            Some(Token {
                kind: TokenKind::Number { value, .. },
                span: number_span,
            }) => {
                let close = expect_close(&tokens, pos + 1, key)?;
                segments.push(Segment {
                    kind: SegmentKind::Index {
                        value: *value,
                        number_span: *number_span,
                    },
                    span: Some(Span {
                        start: open.span.start,
                        end: close.end,
                    }),
                });
                pos += 2;
            }
            Some(Token {
                kind: TokenKind::RBracket,
                span: close,
            }) => {
                segments.push(Segment {
                    kind: SegmentKind::Append,
                    span: Some(Span {
                        start: open.span.start,
                        end: close.end,
                    }),
                });
                pos += 1;
            }
            other => {
                return Err(syntax_error(
                    "Expecting a text, a number or ']'",
                    key,
                    span_of(other.map(|t| t.span), key),
                ));
            }
        }
    }
    Ok(segments)
}

/// The next token must be `]`; returns its span.
fn expect_close(tokens: &[Token], pos: usize, key: &str) -> Result<Span, NestedJsonError> {
    match tokens.get(pos) {
        Some(Token {
            kind: TokenKind::RBracket,
            span,
        }) => Ok(*span),
        other => Err(syntax_error(
            "Expecting ']'",
            key,
            span_of(other.map(|t| t.span), key),
        )),
    }
}

/// Errors at end of input point one column past the last character.
fn span_of(token_span: Option<Span>, key: &str) -> Option<Span> {
    token_span.or_else(|| {
        let end = key.chars().count();
        Some(Span {
            start: end,
            end: end + 1,
        })
    })
}

fn syntax_error(message: &str, key: &str, span: Option<Span>) -> NestedJsonError {
    NestedJsonError {
        kind: ErrorKind::Syntax,
        message: message.to_string(),
        key: key.to_string(),
        span,
    }
}
