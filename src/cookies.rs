//! A small cookie jar: enough of RFC 6265 for redirect chains and
//! sessions (domain/path matching, Secure with the localhost extension).

#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// The cookie's domain: the setting host, or the `Domain` attribute
    /// (which additionally allows subdomains).
    pub domain: String,
    /// True when a `Domain` attribute was present.
    pub domain_attribute: bool,
    pub path: String,
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
}

impl Cookie {
    fn matches(&self, scheme: &str, host: &str, path: &str) -> bool {
        let host = host.to_ascii_lowercase();
        let domain_ok = if self.domain_attribute {
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
        path: "/".to_string(),
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
    fn later_cookies_replace_matching_ones() {
        let mut jar = Jar::new();
        jar.store("h", "k=1");
        jar.store("h", "k=2");
        assert_eq!(jar.header_for("http", "h", "/"), Some("k=2".into()));
        jar.store("h", "j=3");
        assert_eq!(jar.header_for("http", "h", "/"), Some("k=2; j=3".into()));
    }
}
