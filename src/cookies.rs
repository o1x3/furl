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

    /// Parse one `Set-Cookie` value received for a request to
    /// `request_path` on `host`, and store it. A cookie whose `Max-Age`
    /// or `Expires` already lies at or before `now` deletes any matching
    /// cookie instead of being stored (a server-driven deletion).
    pub fn store(&mut self, host: &str, request_path: &str, set_cookie: &str, now: u64) {
        let Some(cookie) = parse_set_cookie(host, request_path, set_cookie) else {
            return;
        };
        let expired = cookie.expires.is_some_and(|expiry| expiry <= now);
        self.cookies.retain(|c| {
            !(c.name == cookie.name && c.domain == cookie.domain && c.path == cookie.path)
        });
        if !expired {
            self.cookies.push(cookie);
        }
    }

    /// The `Cookie:` header value for a request, or None when nothing
    /// matches. Cookies whose lifetime has elapsed by `now` are omitted.
    pub fn header_for(&self, scheme: &str, host: &str, path: &str, now: u64) -> Option<String> {
        let matching: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|expiry| expiry > now))
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

fn parse_set_cookie(host: &str, request_path: &str, header: &str) -> Option<Cookie> {
    let mut parts = header.split(';');
    let pair = parts.next()?;
    let (name, value) = pair.split_once('=')?;
    let mut cookie = Cookie {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        domain: host.to_ascii_lowercase(),
        domain_attribute: false,
        explicit_none_domain: false,
        // No `Path` attribute defaults to the request path's directory
        // (Netscape rule), not `/`.
        path: default_cookie_path(request_path),
        expires: None,
        secure: false,
    };
    if cookie.name.is_empty() {
        return None;
    }
    // `Max-Age` (relative seconds) takes precedence over `Expires` (an
    // absolute date); both resolve to an absolute epoch. A past or zero
    // value marks the cookie already expired.
    let mut max_age: Option<i64> = None;
    let mut expires_at: Option<u64> = None;
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
            "max-age" => max_age = value.parse::<i64>().ok(),
            "expires" if expires_at.is_none() => expires_at = parse_http_date(value),
            _ => {}
        }
    }
    cookie.expires = match max_age {
        // A non-positive Max-Age is expiry in the past (epoch 0 is a
        // safe "already elapsed" sentinel).
        Some(seconds) if seconds <= 0 => Some(0),
        Some(seconds) => Some(now_add(seconds)),
        None => expires_at,
    };
    Some(cookie)
}

/// The default cookie path: the request path up to (not including) its
/// last `/`, or `/` when that leaves nothing (Netscape/version-0 rule).
fn default_cookie_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => request_path[..index].to_string(),
    }
}

/// Absolute epoch for a positive relative `Max-Age`, clamped so the
/// arithmetic cannot overflow `u64`.
fn now_add(seconds: i64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_add(seconds as u64)
}

/// Parse an HTTP cookie `Expires` date into an epoch. Handles the
/// RFC 1123 form (`Wdy, DD Mon YYYY HH:MM:SS GMT`) and the RFC 850 form
/// with a 2-digit year; other forms yield `None` (the cookie then has no
/// persistent lifetime, treated as a session cookie).
fn parse_http_date(text: &str) -> Option<u64> {
    // Split off the weekday (comma-terminated or space-terminated).
    let rest = text
        .split_once(", ")
        .map(|(_, r)| r)
        .or_else(|| text.split_once(' ').map(|(_, r)| r))?
        .trim();
    // Two shapes: "DD Mon YYYY HH:MM:SS GMT" or "DD-Mon-YY HH:MM:SS GMT".
    let normalized = rest.replace('-', " ");
    let mut fields = normalized.split_whitespace();
    let day: i64 = fields.next()?.parse().ok()?;
    let month = month_number(fields.next()?)?;
    let mut year: i64 = fields.next()?.parse().ok()?;
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let time = fields.next()?;
    let mut hms = time.split(':');
    let hour: i64 = hms.next()?.parse().ok()?;
    let minute: i64 = hms.next()?.parse().ok()?;
    let second: i64 = hms.next().unwrap_or("0").parse().ok()?;
    let days = days_from_civil(year, month, day);
    let seconds = days * 86400 + hour * 3600 + minute * 60 + second;
    u64::try_from(seconds).ok()
}

fn month_number(name: &str) -> Option<i64> {
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = name.to_ascii_lowercase();
    months
        .iter()
        .position(|m| lower.starts_with(m))
        .map(|index| index as i64 + 1)
}

/// Days since the Unix epoch for a civil (proleptic Gregorian) date
/// (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed "now" well before any test expiry.
    const NOW: u64 = 1_000_000;

    fn store(jar: &mut Jar, host: &str, set_cookie: &str) {
        jar.store(host, "/", set_cookie, NOW);
    }

    fn header(jar: &Jar, scheme: &str, host: &str, path: &str) -> Option<String> {
        jar.header_for(scheme, host, path, NOW)
    }

    #[test]
    fn same_host_only_without_domain_attribute() {
        let mut jar = Jar::new();
        store(&mut jar, "a.example", "k=v");
        assert_eq!(header(&jar, "http", "a.example", "/"), Some("k=v".into()));
        assert_eq!(header(&jar, "http", "b.example", "/"), None);
        assert_eq!(header(&jar, "http", "sub.a.example", "/"), None);
    }

    #[test]
    fn domain_attribute_allows_subdomains() {
        let mut jar = Jar::new();
        store(&mut jar, "a.example", "k=v; Domain=a.example");
        assert_eq!(
            header(&jar, "http", "sub.a.example", "/"),
            Some("k=v".into())
        );
    }

    #[test]
    fn path_matching() {
        let mut jar = Jar::new();
        store(&mut jar, "h", "k=v; Path=/api");
        assert_eq!(header(&jar, "http", "h", "/api"), Some("k=v".into()));
        assert_eq!(header(&jar, "http", "h", "/api/x"), Some("k=v".into()));
        assert_eq!(header(&jar, "http", "h", "/apix"), None);
        assert_eq!(header(&jar, "http", "h", "/"), None);
    }

    #[test]
    fn default_path_is_the_request_directory() {
        // No Path attribute defaults to the request path's directory.
        let mut jar = Jar::new();
        jar.store("h", "/a/b/c", "k=v", NOW);
        assert_eq!(jar.cookies()[0].path, "/a/b");
        // A top-level request keeps the root path.
        let mut jar = Jar::new();
        jar.store("h", "/a", "k=v", NOW);
        assert_eq!(jar.cookies()[0].path, "/");
    }

    #[test]
    fn max_age_zero_deletes_the_cookie() {
        let mut jar = Jar::new();
        store(&mut jar, "h", "k=v");
        assert_eq!(header(&jar, "http", "h", "/"), Some("k=v".into()));
        store(&mut jar, "h", "k=gone; Max-Age=0");
        assert_eq!(header(&jar, "http", "h", "/"), None);
    }

    #[test]
    fn future_max_age_keeps_the_cookie() {
        let mut jar = Jar::new();
        store(&mut jar, "h", "k=v; Max-Age=3600");
        assert!(jar.cookies()[0].expires.is_some());
        assert_eq!(header(&jar, "http", "h", "/"), Some("k=v".into()));
    }

    #[test]
    fn past_expires_date_deletes_the_cookie() {
        let mut jar = Jar::new();
        store(&mut jar, "h", "k=v");
        store(
            &mut jar,
            "h",
            "k=gone; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        );
        assert_eq!(header(&jar, "http", "h", "/"), None);
    }

    #[test]
    fn parses_rfc1123_expires_date() {
        // 2021-06-10T12:00:00Z = 1623326400.
        assert_eq!(
            parse_http_date("Thu, 10 Jun 2021 12:00:00 GMT"),
            Some(1_623_326_400)
        );
        // RFC 850 two-digit year.
        assert_eq!(
            parse_http_date("Thursday, 10-Jun-21 12:00:00 GMT"),
            Some(1_623_326_400)
        );
    }

    #[test]
    fn secure_cookies_need_https_or_localhost() {
        let mut jar = Jar::new();
        store(&mut jar, "h.example", "k=v; Secure");
        assert_eq!(header(&jar, "http", "h.example", "/"), None);
        assert_eq!(header(&jar, "https", "h.example", "/"), Some("k=v".into()));
        let mut jar = Jar::new();
        store(&mut jar, "localhost", "k=v; Secure");
        assert_eq!(header(&jar, "http", "localhost", "/"), Some("k=v".into()));
    }

    #[test]
    fn parsed_cookie_has_no_expiry_and_bound_domain() {
        // A cookie from a bare Set-Cookie line is a session cookie (no
        // persistent expiry) bound to the setting host (not null).
        let mut jar = Jar::new();
        store(&mut jar, "a.example", "k=v");
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
        assert_eq!(header(&jar, "http", "a.example", "/"), Some("k=v".into()));
        assert_eq!(header(&jar, "http", "b.example", "/"), Some("k=v".into()));
    }

    #[test]
    fn later_cookies_replace_matching_ones() {
        let mut jar = Jar::new();
        store(&mut jar, "h", "k=1");
        store(&mut jar, "h", "k=2");
        assert_eq!(header(&jar, "http", "h", "/"), Some("k=2".into()));
        store(&mut jar, "h", "j=3");
        assert_eq!(header(&jar, "http", "h", "/"), Some("k=2; j=3".into()));
    }
}
