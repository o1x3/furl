//! A small cookie jar: enough of RFC 6265 for redirect chains and
//! sessions (domain/path matching, Secure with the localhost extension).

#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// The cookie's domain: the setting host, or the `Domain` attribute
    /// (which additionally allows subdomains).
    ///
    /// An empty string means "unbound" — the cookie matches any host. This
    /// arises only from session files (a legacy insecure cookie stored with
    /// no domain, or `domain: null`); cookies created from a `Set-Cookie`
    /// response always carry the setting host.
    pub domain: String,
    /// True when a `Domain` attribute was present.
    pub domain_attribute: bool,
    /// True when the session record stored `domain: null` rather than an
    /// empty string. Both an empty domain and a null domain match any host;
    /// the marker exists only so a session round-trips the on-disk form
    /// faithfully (null back to null, `""` back to `""`) and so the
    /// legacy-upgrade warning can distinguish the two.
    pub explicit_none_domain: bool,
    pub path: String,
    /// Absolute expiry in epoch seconds, or `None` for a session cookie
    /// (no persistent lifetime). Sessions persist this; a cookie parsed
    /// from a bare `Set-Cookie` line is a session cookie.
    pub expires: Option<u64>,
    pub secure: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Jar {
    cookies: Vec<Cookie>,
}

impl Jar {
    pub fn new() -> Jar {
        Jar::default()
    }

    /// Parse one `Set-Cookie` value from `host` and store it.
    pub fn store(&mut self, host: &str, set_cookie: &str) {
        let Some(cookie) = parse_set_cookie(host, set_cookie) else {
            return;
        };
        self.cookies.retain(|c| {
            !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        });
        self.cookies.push(cookie);
    }

    /// The `Cookie:` header value for a request, or None when nothing
    /// matches.
    pub fn header_for(&self, scheme: &str, host: &str, path: &str) -> Option<String> {
        let matching: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| c.matches(scheme, host, path))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        if matching.is_empty() {
            None
        } else {
            Some(matching.join("; "))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    /// The stored cookies, in insertion order. Used by the session layer to
    /// serialize the jar to disk.
    pub fn cookies(&self) -> &[Cookie] {
        &self.cookies
    }

    /// Insert a fully-specified cookie (as reconstructed from a session
    /// file), replacing any existing cookie with the same name, domain, and
    /// path. Used when loading a session into the jar.
    pub fn insert(&mut self, cookie: Cookie) {
        self.cookies.retain(|c| {
            !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        });
        self.cookies.push(cookie);
    }
}

impl Cookie {
    fn matches(&self, scheme: &str, host: &str, path: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let domain_ok = if self.domain.is_empty() {
            // An unbound cookie (session file with empty or null domain)
            // matches any host.
            true
        } else if self.domain_attribute {
            host == self.domain || host.ends_with(&format!(".{}", self.domain))
        } else {
            host == self.domain
        };
        if !domain_ok {
            return false;
        }
        if !path_matches(&self.path, path) {
            return false;
        }
        if self.secure {
            // Secure cookies also flow to localhost over plain http,
            // mirroring modern browsers.
            let localhost = host == "localhost" || host.ends_with(".localhost");
            return scheme == "https" || localhost;
        }
        true
    }
}

fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    let request_path = if request_path.is_empty() {
        "/"
    } else {
        request_path
    };
    request_path == cookie_path
        || (request_path.starts_with(cookie_path)
            && (cookie_path.ends_with('/')
                || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')))
}

fn parse_set_cookie(host: &str, header: &str) -> Option<Cookie> {
    let mut parts = header.split(';');
    let pair = parts.next()?;
    let (name, value) = pair.split_once('=')?;
    let mut cookie = Cookie {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        domain: host.to_ascii_lowercase(),
        domain_attribute: false,
        explicit_none_domain: false,
        path: "/".to_string(),
        expires: None,
        secure: false,
    };
    if cookie.name.is_empty() {
        return None;
    }
    for attribute in parts {
        let attribute = attribute.trim();
        let (key, value) = match attribute.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim()),
            None => (attribute.to_ascii_lowercase(), ""),
        };
        match key.as_str() {
            "domain" if !value.is_empty() => {
                cookie.domain = value.trim_start_matches('.').to_ascii_lowercase();
                cookie.domain_attribute = true;
            }
            "path" if value.starts_with('/') => cookie.path = value.to_string(),
            "secure" => cookie.secure = true,
            _ => {}
        }
    }
    Some(cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_host_only_without_domain_attribute() {
        let mut jar = Jar::new();
        jar.store("a.example", "k=v");
        assert_eq!(jar.header_for("http", "a.example", "/"), Some("k=v".into()));
        assert_eq!(jar.header_for("http", "b.example", "/"), None);
        assert_eq!(jar.header_for("http", "sub.a.example", "/"), None);
    }

    #[test]
    fn domain_attribute_allows_subdomains() {
        let mut jar = Jar::new();
        jar.store("a.example", "k=v; Domain=a.example");
        assert_eq!(
            jar.header_for("http", "sub.a.example", "/"),
            Some("k=v".into())
        );
    }

    #[test]
    fn path_matching() {
        let mut jar = Jar::new();
        jar.store("h", "k=v; Path=/api");
        assert_eq!(jar.header_for("http", "h", "/api"), Some("k=v".into()));
        assert_eq!(jar.header_for("http", "h", "/api/x"), Some("k=v".into()));
        assert_eq!(jar.header_for("http", "h", "/apix"), None);
        assert_eq!(jar.header_for("http", "h", "/"), None);
    }

    #[test]
    fn secure_cookies_need_https_or_localhost() {
        let mut jar = Jar::new();
        jar.store("h.example", "k=v; Secure");
        assert_eq!(jar.header_for("http", "h.example", "/"), None);
        assert_eq!(
            jar.header_for("https", "h.example", "/"),
            Some("k=v".into())
        );
        let mut jar = Jar::new();
        jar.store("localhost", "k=v; Secure");
        assert_eq!(jar.header_for("http", "localhost", "/"), Some("k=v".into()));
    }

    #[test]
    fn parsed_cookie_has_no_expiry_and_bound_domain() {
        // A cookie from a bare Set-Cookie line is a session cookie (no
        // persistent expiry) bound to the setting host (not null).
        let mut jar = Jar::new();
        jar.store("a.example", "k=v");
        let cookie = &jar.cookies()[0];
        assert_eq!(cookie.expires, None);
        assert!(!cookie.explicit_none_domain);
        assert_eq!(cookie.domain, "a.example");
    }

    #[test]
    fn unbound_cookie_matches_any_host() {
        // A session-loaded cookie with an empty domain flows to every host.
        let mut jar = Jar::new();
        jar.insert(Cookie {
            name: "k".to_string(),
            value: "v".to_string(),
            domain: String::new(),
            domain_attribute: false,
            explicit_none_domain: true,
            path: "/".to_string(),
            expires: None,
            secure: false,
        });
        assert_eq!(jar.header_for("http", "a.example", "/"), Some("k=v".into()));
        assert_eq!(jar.header_for("http", "b.example", "/"), Some("k=v".into()));
    }

    #[test]
    fn later_cookies_replace_matching_ones() {
        let mut jar = Jar::new();
        jar.store("h", "k=1");
        jar.store("h", "k=2");
        assert_eq!(jar.header_for("http", "h", "/"), Some("k=2".into()));
        jar.store("h", "j=3");
        assert_eq!(jar.header_for("http", "h", "/"), Some("k=2; j=3".into()));
    }
}
