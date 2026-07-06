//! URL normalization for the positional URL argument.

/// Does the URL already carry a scheme (`letter [letter/digit/.+-]* ://`,
/// ASCII case-insensitive)?
fn has_scheme(url: &str) -> bool {
    let Some(separator) = url.find("://") else {
        return false;
    };
    let scheme = &url[..separator];
    let mut chars = scheme.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// Normalize the URL argument:
///
/// 1. A leading `://` is dropped (the "paste a URL and add a space"
///    shortcut: `furl ://pie.dev/get`).
/// 2. A URL that already has a scheme passes through untouched.
/// 3. Otherwise the default scheme is prefixed, with the curl-style
///    localhost shorthand: a leading `:` not followed by another `:`
///    means `localhost`, with optional port digits (`:3000/path`).
pub fn normalize_url(url: &str, default_scheme: &str) -> String {
    let url = url.strip_prefix("://").unwrap_or(url);
    if has_scheme(url) {
        return url.to_string();
    }
    let scheme = format!("{default_scheme}://");

    // Localhost shorthand: `:` `(?!:)` digits rest.
    if let Some(rest) = url.strip_prefix(':') {
        if !rest.starts_with(':') {
            let digits_end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let (port, tail) = rest.split_at(digits_end);
            let mut out = format!("{scheme}localhost");
            if !port.is_empty() {
                out.push(':');
                out.push_str(port);
            }
            out.push_str(tail);
            return out;
        }
    }
    format!("{scheme}{url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn norm(url: &str) -> String {
        normalize_url(url, "http")
    }

    #[test]
    fn localhost_shorthand() {
        assert_eq!(norm(":"), "http://localhost");
        assert_eq!(norm(":/"), "http://localhost/");
        assert_eq!(norm(":3000"), "http://localhost:3000");
        assert_eq!(norm(":/path"), "http://localhost/path");
        assert_eq!(norm(":3000/"), "http://localhost:3000/");
        assert_eq!(norm(":3000/path"), "http://localhost:3000/path");
    }

    #[test]
    fn ipv6_literals_are_not_shorthand() {
        assert_eq!(norm("::1"), "http://::1");
        assert_eq!(norm("::ffff:c000:0280"), "http://::ffff:c000:0280");
        assert_eq!(
            norm("2001:db8:85a3:8d3:1319:8a2e:370:7348"),
            "http://2001:db8:85a3:8d3:1319:8a2e:370:7348"
        );
    }

    #[test]
    fn shorthand_tail_is_kept_verbatim() {
        // The shorthand grammar puts anything after the digits straight
        // onto the rewritten URL.
        assert_eq!(norm(":abc"), "http://localhostabc");
        assert_eq!(norm(":3000abc"), "http://localhost:3000abc");
    }

    #[test]
    fn existing_schemes_pass_through() {
        assert_eq!(norm("http://example.org"), "http://example.org");
        assert_eq!(norm("HTTPS://example.org"), "HTTPS://example.org");
        assert_eq!(norm("foo+bar-BAZ.123://x"), "foo+bar-BAZ.123://x");
        assert_eq!(norm("ftp://host"), "ftp://host");
    }

    #[test]
    fn invalid_schemes_get_prefixed() {
        assert_eq!(norm("1http://x"), "http://1http://x");
        assert_eq!(norm("ht tp://x"), "http://ht tp://x");
    }

    #[test]
    fn scheme_prefixing() {
        assert_eq!(norm("example.org"), "http://example.org");
        assert_eq!(norm("example.org/path?q=1"), "http://example.org/path?q=1");
        assert_eq!(normalize_url("example.org", "https"), "https://example.org");
        assert_eq!(
            normalize_url("example.org", "custom"),
            "custom://example.org"
        );
    }

    #[test]
    fn paste_shortcut_strips_leading_separator() {
        assert_eq!(norm("://pie.dev/get"), "http://pie.dev/get");
        // After stripping, normal rules apply.
        assert_eq!(norm("://"), "http://");
    }

    #[test]
    fn empty_url() {
        assert_eq!(norm(""), "http://");
    }
}
