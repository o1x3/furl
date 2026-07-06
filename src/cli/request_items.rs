//! Request-item token splitting.
//!
//! A REQUEST_ITEM argument like `name:=value` carries a key, a separator,
//! and a value. The separator characters `:` `;` `=` `@` can be escaped
//! with a backslash; a backslash before any other character (or at the end
//! of the token) is kept literally together with that character.

/// The ten request-item separators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Separator {
    /// `:` — HTTP header.
    Header,
    /// `;` — header with an empty value.
    HeaderEmpty,
    /// `:@` — header, value from file.
    HeaderFromFile,
    /// `==` — URL query parameter.
    Query,
    /// `==@` — query parameter, value from file.
    QueryFromFile,
    /// `=` — data field (string).
    Data,
    /// `=@` — data field, value from file.
    DataFromFile,
    /// `:=` — raw-JSON data field.
    RawJson,
    /// `:=@` — raw-JSON data field from file.
    RawJsonFromFile,
    /// `@` — file-upload form field.
    FileUpload,
}

impl Separator {
    pub fn as_str(self) -> &'static str {
        match self {
            Separator::Header => ":",
            Separator::HeaderEmpty => ";",
            Separator::HeaderFromFile => ":@",
            Separator::Query => "==",
            Separator::QueryFromFile => "==@",
            Separator::Data => "=",
            Separator::DataFromFile => "=@",
            Separator::RawJson => ":=",
            Separator::RawJsonFromFile => ":=@",
            Separator::FileUpload => "@",
        }
    }

    /// Separators whose presence marks the request as carrying data
    /// (used by method guessing).
    pub fn is_data(self) -> bool {
        matches!(
            self,
            Separator::Data
                | Separator::DataFromFile
                | Separator::RawJson
                | Separator::RawJsonFromFile
                | Separator::FileUpload
        )
    }
}

/// All separators, referenced by the standard REQUEST_ITEM grammar.
pub const ALL_SEPARATORS: &[Separator] = &[
    Separator::Header,
    Separator::HeaderEmpty,
    Separator::HeaderFromFile,
    Separator::Query,
    Separator::QueryFromFile,
    Separator::Data,
    Separator::DataFromFile,
    Separator::RawJson,
    Separator::RawJsonFromFile,
    Separator::FileUpload,
];

/// A split token: `key <separator> value`, with escapes resolved in both
/// the key and the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitItem {
    pub key: String,
    pub separator: Separator,
    pub value: String,
}

/// The token contained no usable (unescaped) separator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoSeparatorError {
    pub token: String,
}

/// The separator characters; a backslash before one of these escapes it.
fn is_special(c: char) -> bool {
    matches!(c, ':' | ';' | '=' | '@')
}

/// One resolved character: escaped characters can never take part in a
/// separator match.
#[derive(Debug, Clone, Copy)]
struct ResolvedChar {
    c: char,
    escaped: bool,
}

fn resolve_escapes(token: &str) -> Vec<ResolvedChar> {
    let raw: Vec<char> = token.chars().collect();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == '\\' {
            match raw.get(i + 1) {
                Some(&next) if is_special(next) => {
                    out.push(ResolvedChar {
                        c: next,
                        escaped: true,
                    });
                    i += 2;
                }
                Some(&next) => {
                    // Both the backslash and the character survive; the
                    // pair is consumed together so neither can join a
                    // separator match started earlier.
                    out.push(ResolvedChar {
                        c: '\\',
                        escaped: false,
                    });
                    out.push(ResolvedChar {
                        c: next,
                        escaped: false,
                    });
                    i += 2;
                }
                None => {
                    out.push(ResolvedChar {
                        c: '\\',
                        escaped: false,
                    });
                    i += 1;
                }
            }
        } else {
            out.push(ResolvedChar {
                c: raw[i],
                escaped: false,
            });
            i += 1;
        }
    }
    out
}

/// Does `separator` match at `chars[at..]` using only unescaped characters?
fn matches_at(chars: &[ResolvedChar], at: usize, separator: &str) -> bool {
    let sep: Vec<char> = separator.chars().collect();
    if at + sep.len() > chars.len() {
        return false;
    }
    sep.iter()
        .zip(&chars[at..])
        .all(|(want, have)| !have.escaped && have.c == *want)
}

/// Split a token on the earliest separator occurrence; when several
/// separators start at the same position, the longest wins.
pub fn split_item(token: &str, separators: &[Separator]) -> Result<SplitItem, NoSeparatorError> {
    let chars = resolve_escapes(token);
    for at in 0..chars.len() {
        let best = separators
            .iter()
            .filter(|sep| matches_at(&chars, at, sep.as_str()))
            .max_by_key(|sep| sep.as_str().len());
        if let Some(&separator) = best {
            let sep_len = separator.as_str().chars().count();
            let key: String = chars[..at].iter().map(|r| r.c).collect();
            let value: String = chars[at + sep_len..].iter().map(|r| r.c).collect();
            return Ok(SplitItem {
                key,
                separator,
                value,
            });
        }
    }
    Err(NoSeparatorError {
        token: token.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn split(token: &str) -> (String, Separator, String) {
        let item = split_item(token, ALL_SEPARATORS).expect("expected a valid item");
        (item.key, item.separator, item.value)
    }

    #[test]
    fn basic_separators() {
        assert_eq!(
            split("X-Hdr:v"),
            ("X-Hdr".into(), Separator::Header, "v".into())
        );
        assert_eq!(
            split("name;"),
            ("name".into(), Separator::HeaderEmpty, "".into())
        );
        assert_eq!(
            split("X:@f"),
            ("X".into(), Separator::HeaderFromFile, "f".into())
        );
        assert_eq!(split("q==1"), ("q".into(), Separator::Query, "1".into()));
        assert_eq!(
            split("q==@f"),
            ("q".into(), Separator::QueryFromFile, "f".into())
        );
        assert_eq!(split("d=1"), ("d".into(), Separator::Data, "1".into()));
        assert_eq!(
            split("d=@f"),
            ("d".into(), Separator::DataFromFile, "f".into())
        );
        assert_eq!(split("j:=1"), ("j".into(), Separator::RawJson, "1".into()));
        assert_eq!(
            split("j:=@f"),
            ("j".into(), Separator::RawJsonFromFile, "f".into())
        );
        assert_eq!(
            split("f@p"),
            ("f".into(), Separator::FileUpload, "p".into())
        );
    }

    #[test]
    fn longest_match_wins_at_same_position() {
        assert_eq!(split("a==b").1, Separator::Query);
        assert_eq!(split("a:=b").1, Separator::RawJson);
        assert_eq!(split("a:=@f").1, Separator::RawJsonFromFile);
        assert_eq!(split("a==@f").1, Separator::QueryFromFile);
        assert_eq!(split("a=@f").1, Separator::DataFromFile);
        assert_eq!(split("a:@f").1, Separator::HeaderFromFile);
    }

    #[test]
    fn earliest_position_wins() {
        // The value keeps later separator-lookalikes verbatim.
        assert_eq!(split("a=b:c"), ("a".into(), Separator::Data, "b:c".into()));
        assert_eq!(split("a=b=c"), ("a".into(), Separator::Data, "b=c".into()));
        assert_eq!(
            split("a@b==c"),
            ("a".into(), Separator::FileUpload, "b==c".into())
        );
        assert_eq!(
            split("a==b==c"),
            ("a".into(), Separator::Query, "b==c".into())
        );
    }

    #[test]
    fn escaped_separators_are_literal() {
        assert_eq!(
            split(r"path\==c:\windows"),
            ("path=".into(), Separator::Data, r"c:\windows".into())
        );
        assert_eq!(
            split(r"bob\:==foo"),
            ("bob:".into(), Separator::Query, "foo".into())
        );
        assert_eq!(
            split(r"weird\;=x"),
            ("weird;".into(), Separator::Data, "x".into())
        );
        assert_eq!(
            split(r"only\@=y"),
            ("only@".into(), Separator::Data, "y".into())
        );
    }

    #[test]
    fn escapes_resolve_in_values_too() {
        assert_eq!(split(r"a=b\:c").2, "b:c");
        assert_eq!(split(r"a=b\=c").2, "b=c");
    }

    #[test]
    fn backslash_before_non_special_is_kept() {
        assert_eq!(split(r"a=b\nc").2, r"b\nc");
        assert_eq!(split(r"a=b\\c").2, r"b\\c");
        assert_eq!(split(r"path=c:\windows").2, r"c:\windows");
        assert_eq!(split(r"key\ name=x").0, r"key\ name");
    }

    #[test]
    fn double_backslash_does_not_shield_a_following_separator() {
        assert_eq!(
            split(r"a\\=b"),
            (r"a\\".into(), Separator::Data, "b".into())
        );
    }

    #[test]
    fn trailing_backslash_is_kept() {
        assert_eq!(split(r"a=b\").2, r"b\");
    }

    #[test]
    fn empty_keys_and_values() {
        assert_eq!(split(";"), ("".into(), Separator::HeaderEmpty, "".into()));
        assert_eq!(split(":"), ("".into(), Separator::Header, "".into()));
        assert_eq!(split("=x"), ("".into(), Separator::Data, "x".into()));
        assert_eq!(split("==y"), ("".into(), Separator::Query, "y".into()));
        assert_eq!(split("a:"), ("a".into(), Separator::Header, "".into()));
        assert_eq!(
            split("a;junk"),
            ("a".into(), Separator::HeaderEmpty, "junk".into())
        );
        assert_eq!(split("@f"), ("".into(), Separator::FileUpload, "f".into()));
    }

    #[test]
    fn no_separator_is_an_error() {
        assert!(split_item("plain", ALL_SEPARATORS).is_err());
        assert!(split_item(r"\=leading", ALL_SEPARATORS).is_err());
        assert!(split_item("", ALL_SEPARATORS).is_err());
    }

    #[test]
    fn single_separator_grammars() {
        // --auth and --proxy split on ':' only.
        let auth = split_item("user:pa:ss", &[Separator::Header]).unwrap();
        assert_eq!((auth.key.as_str(), auth.value.as_str()), ("user", "pa:ss"));
        let escaped = split_item(r"user\:name:pw", &[Separator::Header]).unwrap();
        assert_eq!(
            (escaped.key.as_str(), escaped.value.as_str()),
            ("user:name", "pw")
        );
        assert!(split_item("useronly", &[Separator::Header]).is_err());
    }

    #[test]
    fn unicode_passthrough() {
        assert_eq!(
            split("héader:välue"),
            ("héader".into(), Separator::Header, "välue".into())
        );
    }
}
