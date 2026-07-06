//! Session persistence: headers, cookies, and auth kept in a JSON file and
//! replayed across invocations.
//!
//! A session is a single JSON document under the config directory (a named
//! session) or at an arbitrary path (an anonymous session). It is loaded
//! before a request runs and — unless read-only against an existing file —
//! rewritten afterward, so headers, the cookie jar, and the resolved auth
//! carry over to the next run against the same host.
//!
//! The on-disk format is deliberately conservative: keys are sorted, indent
//! is four spaces, non-ASCII is escaped, and a trailing newline is written,
//! so a session file is stable under `git diff` and matches the layout of
//! the compatibility target. Unknown top-level keys and the pre-list "dict"
//! layouts of `cookies`/`headers` are preserved verbatim, because a plain
//! run must never silently migrate a file a user may still open with older
//! tooling — only an explicit upgrade command does that.

use std::path::{Path, PathBuf};

use crate::cookies::{Cookie, Jar};
use crate::json::{self, DumpOptions, Value};
use crate::paths::expand_tilde;

/// Where the session module points users to learn about the format. Written
/// into `__meta__.help` on every save.
const SESSION_HELP_URL: &str = "https://github.com/o1x3/furl#sessions";

/// The human-readable `__meta__.about` string.
const SESSION_ABOUT: &str = "furl session file";

/// The `__meta__` key under which the writing program's version is stamped.
const VERSION_KEY: &str = "furl";

/// Resolve the file path for a session.
///
/// A value containing the OS path separator is an *anonymous* session: it is
/// tilde-expanded and used as a literal path. Anything else is a *named*
/// session, stored at `<config_dir>/sessions/<host_dir>/<name>.json`, where
/// `<host_dir>` is the bound hostname with every `:` turned into `_` (so a
/// port produces a distinct directory) and case preserved.
pub fn session_path(name: &str, bound_host: &str, config_dir: &Path) -> PathBuf {
    if is_anonymous(name) {
        return expand_tilde(name);
    }
    let host_dir = host_directory(bound_host);
    config_dir
        .join("sessions")
        .join(host_dir)
        .join(format!("{name}.json"))
}

/// Does this session value name a path (rather than a bare session name)?
/// Only the native path separator counts, matching the parse-time rule.
fn is_anonymous(value: &str) -> bool {
    value.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && value.contains('/'))
}

/// The sessions sub-directory name for a bound hostname: `:` becomes `_` so
/// `example.org:8080` and `example.org` never collide, and so a colon (not
/// a legal path character on every platform) never reaches the filesystem.
fn host_directory(bound_host: &str) -> String {
    bound_host.replace(':', "_")
}

/// The bound hostname used to locate a named session, per the resolution
/// order: an explicit `Host:` header wins; otherwise the URL's netloc with
/// any userinfo stripped and the port kept; an empty result falls back to
/// the literal `localhost`.
pub fn bound_host(host_header: Option<&str>, url_netloc: &str) -> String {
    if let Some(host) = host_header {
        let host = host.trim();
        if !host.is_empty() {
            return host.to_string();
        }
    }
    let netloc = url_netloc
        .rsplit_once('@')
        .map(|(_, after)| after)
        .unwrap_or(url_netloc);
    if netloc.is_empty() {
        "localhost".to_string()
    } else {
        netloc.to_string()
    }
}

/// The port-stripped host (everything before the first `:`), kept for
/// legacy-upgrade warning text and cookie binding.
pub fn port_stripped_host(bound_host: &str) -> &str {
    bound_host.split(':').next().unwrap_or(bound_host)
}

/// An auth record resolved from a session file.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionAuth {
    /// The auth plugin type (e.g. `basic`, `bearer`).
    pub auth_type: String,
    /// The raw credential string (new style, e.g. `user:password`), if the
    /// record carried one.
    pub raw_auth: Option<String>,
    /// Old-style username/password, if the record used the pre-`raw_auth`
    /// shape. The caller may derive `raw_auth` from these when applying.
    pub username: Option<String>,
    pub password: Option<String>,
}

/// The reason a loaded session is flagged as legacy, so the caller can print
/// exactly one advisory warning. Cookies take precedence when both apply
/// (the spec allows only one warning per load).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyKind {
    /// Pre-list `cookies` dict containing at least one cookie with a missing
    /// or empty-string domain (a null domain does not warn).
    Cookies,
    /// Pre-list `headers` dict (non-empty).
    Headers,
}

/// An error that aborts the run. A corrupt session file is fatal (unlike a
/// corrupt config file, which only warns).
#[derive(Debug)]
pub enum SessionError {
    /// The file could not be read (permissions, etc.).
    Unreadable(String),
    /// The file was not valid JSON, or its top level was not an object.
    InvalidJson(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Unreadable(message) | SessionError::InvalidJson(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for SessionError {}

/// A loaded session document.
///
/// The struct keeps the parsed pieces the request layer consults (headers,
/// cookies, auth) plus enough of the original document to round-trip it
/// faithfully: unknown top-level keys, the existing `__meta__` version
/// stamp, and whether `cookies`/`headers` were stored as lists or dicts.
#[derive(Debug, Clone)]
pub struct Session {
    /// Stored request headers, in order. Duplicate names are allowed and
    /// each is replayed.
    headers: Vec<(String, String)>,
    /// Stored cookies, already pruned of anything expired at load time.
    cookies: Vec<Cookie>,
    /// The resolved auth record, if the file stored one.
    auth: Option<SessionAuth>,
    /// The version string already present in `__meta__` on disk. Preserved
    /// verbatim by normal saves; `None` when the file had no stamp (a fresh
    /// session), in which case the current program version is written.
    stored_version: Option<String>,
    /// Top-level keys other than the four furl manages, kept so they survive
    /// a round-trip.
    extra_keys: Vec<(String, Value)>,
    /// True when `cookies` was stored as a dict (pre-list layout); the same
    /// layout is written back.
    cookies_dict_layout: bool,
    /// True when `headers` was stored as a dict (pre-list layout).
    headers_dict_layout: bool,
    /// Which legacy layout, if any, triggered a load-time warning.
    legacy: Option<LegacyKind>,
}

impl Session {
    /// A fresh, empty session (no file on disk yet). Cookies and headers use
    /// the modern list layout.
    pub fn new() -> Session {
        Session {
            headers: Vec::new(),
            cookies: Vec::new(),
            auth: None,
            stored_version: None,
            extra_keys: Vec::new(),
            cookies_dict_layout: false,
            headers_dict_layout: false,
            legacy: None,
        }
    }

    /// Load a session from `path`, pruning cookies whose stored expiry is at
    /// or before `now_epoch`. A missing file yields a fresh session (not an
    /// error). Malformed JSON, a non-object top level, or an unreadable file
    /// is fatal.
    pub fn load(path: &Path, now_epoch: u64) -> Result<Session, SessionError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Session::new());
            }
            Err(error) => {
                return Err(SessionError::Unreadable(format!(
                    "cannot read session file: {error} [{}]",
                    path.display()
                )));
            }
        };
        Session::from_text(&text, path, now_epoch)
    }

    /// Parse a session from its JSON text. Split out from [`Session::load`]
    /// so tests can exercise parsing without touching the filesystem.
    fn from_text(text: &str, path: &Path, now_epoch: u64) -> Result<Session, SessionError> {
        let value = json::parse(text).map_err(|error| {
            SessionError::InvalidJson(format!(
                "invalid session file: {error} [{}]",
                path.display()
            ))
        })?;
        let pairs = match value {
            Value::Object(pairs) => pairs,
            other => {
                return Err(SessionError::InvalidJson(format!(
                    "invalid session file: top level is {}, expected object [{}]",
                    other.type_name(),
                    path.display()
                )));
            }
        };

        let mut session = Session::new();
        for (key, item) in pairs {
            match key.as_str() {
                "__meta__" => {
                    session.stored_version = item
                        .get(VERSION_KEY)
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                "headers" => session.load_headers(item),
                "cookies" => session.load_cookies(item, now_epoch),
                "auth" => session.auth = parse_auth(&item),
                _ => session.extra_keys.push((key, item)),
            }
        }
        Ok(session)
    }

    /// Read the `headers` value in either layout, recording which one so it
    /// can be written back unchanged.
    fn load_headers(&mut self, value: Value) {
        match value {
            Value::Array(items) => {
                for item in &items {
                    if let (Some(name), Some(val)) = (
                        item.get("name").and_then(Value::as_str),
                        item.get("value").and_then(Value::as_str),
                    ) {
                        self.headers.push((name.to_string(), val.to_string()));
                    }
                }
            }
            Value::Object(pairs) => {
                self.headers_dict_layout = true;
                if !pairs.is_empty() {
                    // A non-empty header dict is always a legacy layout.
                    self.flag_legacy(LegacyKind::Headers);
                }
                for (name, val) in &pairs {
                    if let Some(val) = val.as_str() {
                        self.headers.push((name.clone(), val.to_string()));
                    }
                }
            }
            _ => {}
        }
    }

    /// Read the `cookies` value in either layout, pruning expired entries and
    /// flagging the pre-list dict layout as legacy when it warrants a warning.
    fn load_cookies(&mut self, value: Value, now_epoch: u64) {
        match value {
            Value::Array(items) => {
                for item in &items {
                    if let Some(cookie) = parse_cookie(item, None) {
                        self.push_cookie_if_live(cookie, now_epoch);
                    }
                }
            }
            Value::Object(pairs) => {
                self.cookies_dict_layout = true;
                for (name, record) in &pairs {
                    if let Some(cookie) = parse_cookie(record, Some(name)) {
                        // The cookie dict warns only when a cookie has a
                        // missing or empty (but not null) domain.
                        if !cookie.explicit_none_domain && cookie.domain.is_empty() {
                            self.flag_legacy(LegacyKind::Cookies);
                        }
                        self.push_cookie_if_live(cookie, now_epoch);
                    }
                }
            }
            _ => {}
        }
    }

    /// Keep a cookie only if it has not already expired at load time.
    fn push_cookie_if_live(&mut self, cookie: Cookie, now_epoch: u64) {
        if let Some(expires) = cookie.expires {
            if expires <= now_epoch {
                return;
            }
        }
        self.cookies.push(cookie);
    }

    /// Record a legacy layout. Cookies win when both apply, so a header flag
    /// never displaces a cookie flag (only one warning is shown per load).
    fn flag_legacy(&mut self, kind: LegacyKind) {
        match self.legacy {
            Some(LegacyKind::Cookies) => {}
            _ => self.legacy = Some(kind),
        }
    }

    /// The stored request headers, in order, for the caller to merge under
    /// CLI headers (defaults < session < CLI).
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The resolved auth record, if any, for the caller to apply when the
    /// invocation itself carries no auth.
    pub fn auth(&self) -> Option<&SessionAuth> {
        self.auth.as_ref()
    }

    /// Load this session's cookies into `jar`. Already pruned of expired
    /// entries at load time.
    pub fn load_into_jar(&self, jar: &mut Jar) {
        for cookie in &self.cookies {
            jar.insert(cookie.clone());
        }
    }

    /// Whether the loaded file used a legacy layout warranting an advisory
    /// warning, and if so a ready-to-print message. `session_id` is the
    /// session name (or the full path for anonymous sessions); `host` is the
    /// port-stripped bound host used in the fix command. Returns `None` when
    /// the file is already current.
    ///
    /// The message is advisory only — it never migrates the file. It names
    /// the `furl-manager sessions upgrade` command a user would run to fix
    /// the layout for good.
    pub fn legacy_warning(&self, session_id: &str, host: &str, named: bool) -> Option<String> {
        let kind = self.legacy?;
        let mut message = String::from(
            "Outdated layout detected for the current session. Please consider updating it, ",
        );
        match kind {
            LegacyKind::Cookies => {
                message.push_str("as it can lead to potential security problems.\n");
                message.push_str(&format!(
                    "Use `furl-manager sessions upgrade --bind-cookies {host} {session_id}` \
                     to bind cookies to this host (recommended), or \
                     `furl-manager sessions upgrade {host} {session_id}` to keep them unbound.\n",
                ));
            }
            LegacyKind::Headers => {
                message
                    .push_str("to make use of the latest features regarding the header layout.\n");
                message.push_str(&format!(
                    "Use `furl-manager sessions upgrade {host} {session_id}` to update it.\n",
                ));
            }
        }
        if named {
            message.push_str(
                "To upgrade all your sessions at once, run \
                 `furl-manager sessions upgrade-all`.\n",
            );
        }
        message.push_str(&format!("See {SESSION_HELP_URL} for more information."));
        Some(message)
    }

    /// Replace the stored auth record with the auth resolved for this
    /// invocation. Call this only when the run actually resolved an auth
    /// plugin; leaving it unset preserves whatever the file already held.
    pub fn set_auth(&mut self, auth: SessionAuth) {
        self.auth = Some(auth);
    }

    /// Recompute the stored header set from a request's final outgoing
    /// headers.
    ///
    /// `headers` is the merged (defaults + session + CLI) name/value list;
    /// `unset` names those the CLI explicitly cleared (`Name:`). Applying the
    /// persistence rules: headers whose names appear in the request replace
    /// the same-named stored entries (all occurrences, in request order),
    /// while stored headers absent from the request are kept and appended
    /// after. Skipped entirely: `Content-*`/`If-*` (request-specific),
    /// `Cookie` (its pairs belong in the jar), a default `furl/…`
    /// `User-Agent`, and any CLI-unset name.
    pub fn update_headers_from_request(&mut self, headers: &[(String, String)], unset: &[String]) {
        // Names touched by this request drop their old stored occurrences;
        // the request's own (persistable) occurrences are appended in order.
        let touched: Vec<String> = headers
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        let mut kept: Vec<(String, String)> = self
            .headers
            .iter()
            .filter(|(name, _)| {
                let lower = name.to_ascii_lowercase();
                !touched.contains(&lower)
            })
            .cloned()
            .collect();

        let mut fresh: Vec<(String, String)> = Vec::new();
        for (name, value) in headers {
            if should_persist_header(name, value, unset) {
                fresh.push((name.clone(), value.clone()));
            }
        }
        // Request headers first, then the untouched stored ones.
        fresh.append(&mut kept);
        self.headers = fresh;
    }

    /// Recompute the stored cookie set from `jar`, dropping anything expired
    /// at `now_epoch`. `now_epoch` is supplied by the caller; this function
    /// never reads the clock.
    pub fn update_cookies_from_jar(&mut self, jar: &Jar, now_epoch: u64) {
        self.cookies = jar
            .cookies()
            .iter()
            .filter(|c| match c.expires {
                Some(expires) => expires > now_epoch,
                None => true,
            })
            .cloned()
            .collect();
    }

    /// Serialize the session to its canonical JSON text (4-space indent,
    /// sorted keys, ASCII-escaped, trailing newline).
    pub fn to_json(&self) -> String {
        let mut pairs: Vec<(String, Value)> = Vec::new();

        // Preserve unknown top-level keys; sorting on write makes their
        // position irrelevant, but keep them in the object.
        for (key, value) in &self.extra_keys {
            pairs.push((key.clone(), value.clone()));
        }

        pairs.push(("__meta__".to_string(), self.meta_value()));
        pairs.push(("auth".to_string(), self.auth_value()));
        pairs.push(("cookies".to_string(), self.cookies_value()));
        pairs.push(("headers".to_string(), self.headers_value()));

        let options = DumpOptions {
            indent: Some(4),
            sort_keys: true,
            ensure_ascii: true,
        };
        let mut text = json::dumps(&Value::Object(pairs), &options);
        text.push('\n');
        text
    }

    /// Write the session to `path`, creating parent directories (mode 0o700
    /// on Unix). Overwrites the whole file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            create_dir_all_private(parent)?;
        }
        std::fs::write(path, self.to_json())
    }

    fn meta_value(&self) -> Value {
        // `about` and `help` are overwritten every save; the version stamp is
        // written only when the file had none, preserving an older stamp.
        let version = self
            .stored_version
            .clone()
            .unwrap_or_else(|| crate::VERSION.to_string());
        Value::Object(vec![
            ("about".to_string(), Value::from(SESSION_ABOUT)),
            ("help".to_string(), Value::from(SESSION_HELP_URL)),
            (VERSION_KEY.to_string(), Value::from(version)),
        ])
    }

    fn auth_value(&self) -> Value {
        match &self.auth {
            Some(auth) => {
                let raw =
                    auth.raw_auth
                        .clone()
                        .or_else(|| match (&auth.username, &auth.password) {
                            (Some(user), Some(pass)) => Some(format!("{user}:{pass}")),
                            _ => None,
                        });
                Value::Object(vec![
                    ("type".to_string(), Value::from(auth.auth_type.clone())),
                    (
                        "raw_auth".to_string(),
                        raw.map(Value::from).unwrap_or(Value::Null),
                    ),
                ])
            }
            // A fresh, no-auth session writes the old-style null placeholder.
            None => Value::Object(vec![
                ("type".to_string(), Value::Null),
                ("username".to_string(), Value::Null),
                ("password".to_string(), Value::Null),
            ]),
        }
    }

    fn cookies_value(&self) -> Value {
        if self.cookies_dict_layout {
            let mut pairs: Vec<(String, Value)> = Vec::new();
            for cookie in &self.cookies {
                pairs.push((cookie.name.clone(), cookie_record(cookie, false)));
            }
            Value::Object(pairs)
        } else {
            Value::Array(
                self.cookies
                    .iter()
                    .map(|cookie| cookie_record(cookie, true))
                    .collect(),
            )
        }
    }

    fn headers_value(&self) -> Value {
        if self.headers_dict_layout {
            // A dict collapses duplicate names to the last value.
            let mut pairs: Vec<(String, Value)> = Vec::new();
            for (name, value) in &self.headers {
                match pairs.iter_mut().find(|(n, _)| n == name) {
                    Some(slot) => slot.1 = Value::from(value.clone()),
                    None => pairs.push((name.clone(), Value::from(value.clone()))),
                }
            }
            Value::Object(pairs)
        } else {
            Value::Array(
                self.headers
                    .iter()
                    .map(|(name, value)| {
                        Value::Object(vec![
                            ("name".to_string(), Value::from(name.clone())),
                            ("value".to_string(), Value::from(value.clone())),
                        ])
                    })
                    .collect(),
            )
        }
    }
}

impl Default for Session {
    fn default() -> Session {
        Session::new()
    }
}

/// Whether the session should be written back after the run: always when the
/// file did not exist before (first-run bootstrap, including read-only), and
/// otherwise only when the session is not read-only.
pub fn should_save(read_only: bool, existed_before: bool) -> bool {
    !existed_before || !read_only
}

/// Whether a request header is persisted into the session store. Mirrors the
/// exclusion rules: skip `Content-*`/`If-*` (request-specific), skip `Cookie`
/// (its pairs go to the jar), skip a default `furl/…` `User-Agent`, and skip
/// names the CLI explicitly unset.
fn should_persist_header(name: &str, value: &str, unset: &[String]) -> bool {
    if unset.iter().any(|u| u.eq_ignore_ascii_case(name)) {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("content-") || lower.starts_with("if-") {
        return false;
    }
    if lower == "cookie" {
        return false;
    }
    if lower == "user-agent" && value.starts_with("furl/") {
        return false;
    }
    true
}

/// Build one 6-key cookie record (list layout) or the 5-key body of a dict
/// entry (`in_list == false` omits `name`, which is the dict key).
fn cookie_record(cookie: &Cookie, in_list: bool) -> Value {
    let mut pairs: Vec<(String, Value)> = Vec::new();
    if in_list {
        pairs.push(("name".to_string(), Value::from(cookie.name.clone())));
    }
    pairs.push(("value".to_string(), Value::from(cookie.value.clone())));
    // A marked "explicit none" empty domain serializes back as null; any
    // other domain (including a plain empty string) as its string.
    let domain = if cookie.explicit_none_domain {
        Value::Null
    } else {
        Value::from(cookie.domain.clone())
    };
    pairs.push(("domain".to_string(), domain));
    pairs.push(("path".to_string(), Value::from(cookie.path.clone())));
    let expires = match cookie.expires {
        Some(secs) => Value::from(secs as i64),
        None => Value::Null,
    };
    pairs.push(("expires".to_string(), expires));
    pairs.push(("secure".to_string(), Value::from(cookie.secure)));
    Value::Object(pairs)
}

/// Parse one cookie record. `name_from_key` supplies the name for dict-layout
/// entries (where the name is the object key, not a field).
fn parse_cookie(record: &Value, name_from_key: Option<&str>) -> Option<Cookie> {
    let name = match name_from_key {
        Some(name) => name.to_string(),
        None => record.get("name").and_then(Value::as_str)?.to_string(),
    };
    let value = record
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // `domain: null` marks an explicit-none (unbound) cookie; a string domain
    // (including empty) is taken verbatim; a missing key is an empty domain.
    let (domain, explicit_none_domain) = match record.get("domain") {
        Some(Value::Null) => (String::new(), true),
        Some(Value::String(s)) => (s.clone(), false),
        _ => (String::new(), false),
    };
    let path = record
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("/")
        .to_string();
    let expires = match record.get("expires") {
        Some(Value::Number(n)) => n.as_i64().and_then(|s| u64::try_from(s).ok()),
        _ => None,
    };
    let secure = matches!(record.get("secure"), Some(Value::Bool(true)));
    Some(Cookie {
        name,
        value,
        domain,
        // A session cookie's stored domain is authoritative on its own; the
        // subdomain-widening attribute is not persisted, so treat it as a
        // host-only (or unbound) binding.
        domain_attribute: false,
        explicit_none_domain,
        path,
        expires,
        secure,
    })
}

/// Read an `auth` record in either the new (`type`/`raw_auth`) or old
/// (`type`/`username`/`password`) style. A null or empty `type` means "no
/// session auth".
fn parse_auth(value: &Value) -> Option<SessionAuth> {
    let auth_type = value.get("type").and_then(Value::as_str)?;
    if auth_type.is_empty() {
        return None;
    }
    let raw_auth = value
        .get("raw_auth")
        .and_then(Value::as_str)
        .map(str::to_string);
    let username = value
        .get("username")
        .and_then(Value::as_str)
        .map(str::to_string);
    let password = value
        .get("password")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(SessionAuth {
        auth_type: auth_type.to_string(),
        raw_auth,
        username,
        password,
    })
}

/// Create `dir` and its parents. On Unix the leaf directories get mode 0o700
/// so session material (cookies, credentials) is not world-readable.
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dummy_path() -> PathBuf {
        PathBuf::from("/sessions/example.org/test.json")
    }

    // ---- Path resolution -------------------------------------------------

    #[test]
    fn named_session_path_uses_host_dir() {
        let config = PathBuf::from("/config");
        let path = session_path("api", "example.org", &config);
        assert_eq!(path, PathBuf::from("/config/sessions/example.org/api.json"));
    }

    #[test]
    fn named_session_host_port_dir_replaces_colon() {
        let config = PathBuf::from("/config");
        let path = session_path("api", "example.org:8080", &config);
        assert_eq!(
            path,
            PathBuf::from("/config/sessions/example.org_8080/api.json")
        );
    }

    #[test]
    fn named_session_preserves_host_case() {
        let config = PathBuf::from("/config");
        let path = session_path("api", "LOCALHOST", &config);
        assert_eq!(path, PathBuf::from("/config/sessions/LOCALHOST/api.json"));
    }

    #[test]
    fn anonymous_session_uses_literal_path() {
        let config = PathBuf::from("/config");
        let path = session_path("/tmp/my.json", "ignored", &config);
        assert_eq!(path, PathBuf::from("/tmp/my.json"));
    }

    #[test]
    fn anonymous_session_expands_tilde() {
        let home = crate::paths::home_dir().expect("home in test env");
        let config = PathBuf::from("/config");
        let path = session_path("~/s.json", "ignored", &config);
        assert_eq!(path, home.join("s.json"));
    }

    #[test]
    fn bound_host_prefers_host_header() {
        assert_eq!(
            bound_host(Some("h.example:9000"), "other.example"),
            "h.example:9000"
        );
    }

    #[test]
    fn bound_host_strips_userinfo_keeps_port() {
        assert_eq!(
            bound_host(None, "user:pw@host.example:8080"),
            "host.example:8080"
        );
    }

    #[test]
    fn bound_host_empty_falls_back_to_localhost() {
        assert_eq!(bound_host(None, ""), "localhost");
    }

    #[test]
    fn port_stripped_host_drops_port() {
        assert_eq!(port_stripped_host("example.org:8080"), "example.org");
        assert_eq!(port_stripped_host("example.org"), "example.org");
    }

    // ---- Round-trip byte format ------------------------------------------

    #[test]
    fn fresh_session_writes_null_auth_placeholder_and_meta() {
        let session = Session::new();
        let text = session.to_json();
        // Sorted keys, 4-space indent, trailing newline.
        assert!(text.ends_with("}\n"));
        assert!(text.contains("    \"about\": \"furl session file\""));
        assert!(text.contains(&format!("\"help\": \"{SESSION_HELP_URL}\"")));
        assert!(text.contains(&format!("\"furl\": \"{}\"", crate::VERSION)));
        // No-auth placeholder is the old-style null trio.
        assert!(text.contains("\"type\": null"));
        assert!(text.contains("\"username\": null"));
        assert!(text.contains("\"password\": null"));
        // Empty list layout for cookies/headers.
        assert!(text.contains("\"cookies\": []"));
        assert!(text.contains("\"headers\": []"));
    }

    #[test]
    fn full_session_round_trips_byte_for_byte() {
        let source = concat!(
            "{\n",
            "    \"__meta__\": {\n",
            "        \"about\": \"furl session file\",\n",
            "        \"furl\": \"9.9.9\",\n",
            "        \"help\": \"https://github.com/o1x3/furl#sessions\"\n",
            "    },\n",
            "    \"auth\": {\n",
            "        \"raw_auth\": \"user:pass\",\n",
            "        \"type\": \"basic\"\n",
            "    },\n",
            "    \"cookies\": [\n",
            "        {\n",
            "            \"domain\": \"example.org\",\n",
            "            \"expires\": null,\n",
            "            \"name\": \"sid\",\n",
            "            \"path\": \"/\",\n",
            "            \"secure\": false,\n",
            "            \"value\": \"abc\"\n",
            "        }\n",
            "    ],\n",
            "    \"headers\": [\n",
            "        {\n",
            "            \"name\": \"X-Api\",\n",
            "            \"value\": \"v1\"\n",
            "        }\n",
            "    ]\n",
            "}\n",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        // The stored version (9.9.9) is preserved, not bumped.
        assert_eq!(session.stored_version.as_deref(), Some("9.9.9"));
        assert_eq!(session.to_json(), source);
    }

    #[test]
    fn unknown_top_level_keys_survive() {
        let source = concat!(
            "{\n",
            "    \"__meta__\": {\"furl\": \"1.0.0\"},\n",
            "    \"custom_key\": {\"nested\": [1, 2]}\n",
            "}\n",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let text = session.to_json();
        assert!(text.contains("\"custom_key\""));
        assert!(text.contains("\"nested\""));
    }

    // ---- Cookie schema ---------------------------------------------------

    #[test]
    fn cookie_list_has_all_six_keys_with_null_domain() {
        let mut session = Session::new();
        session.cookies.push(Cookie {
            name: "k".to_string(),
            value: "v".to_string(),
            domain: String::new(),
            domain_attribute: false,
            explicit_none_domain: true,
            path: "/".to_string(),
            expires: None,
            secure: false,
        });
        let text = session.to_json();
        assert!(text.contains("\"name\": \"k\""));
        assert!(text.contains("\"value\": \"v\""));
        assert!(text.contains("\"domain\": null"));
        assert!(text.contains("\"path\": \"/\""));
        assert!(text.contains("\"expires\": null"));
        assert!(text.contains("\"secure\": false"));
    }

    #[test]
    fn null_domain_round_trips_as_null_and_empty_string_as_string() {
        let source = concat!(
            "{\n",
            "    \"cookies\": [\n",
            "        {\"name\": \"a\", \"value\": \"1\", \"domain\": null, ",
            "\"path\": \"/\", \"expires\": null, \"secure\": false},\n",
            "        {\"name\": \"b\", \"value\": \"2\", \"domain\": \"\", ",
            "\"path\": \"/\", \"expires\": null, \"secure\": false}\n",
            "    ]\n",
            "}\n",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let a = &session.cookies[0];
        assert!(a.explicit_none_domain);
        let b = &session.cookies[1];
        assert!(!b.explicit_none_domain);
        assert_eq!(b.domain, "");
        let text = session.to_json();
        // First cookie keeps null; second keeps empty string.
        assert!(text.contains("\"domain\": null"));
        assert!(text.contains("\"domain\": \"\""));
    }

    // ---- Legacy dict layouts ---------------------------------------------

    #[test]
    fn legacy_cookie_dict_read_and_written_back_as_dict() {
        let source = concat!(
            "{\"cookies\": {\"sid\": {\"value\": \"x\", \"domain\": \"\", ",
            "\"path\": \"/\", \"expires\": null, \"secure\": false}}}",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.cookies_dict_layout);
        // Empty-domain cookie in a dict triggers the legacy warning.
        assert_eq!(session.legacy, Some(LegacyKind::Cookies));
        let text = session.to_json();
        // Dict layout preserved: keyed by name, so "sid" is an object key.
        assert!(text.contains("\"cookies\": {"));
        assert!(text.contains("\"sid\": {"));
    }

    #[test]
    fn legacy_cookie_dict_null_domain_does_not_warn() {
        let source = concat!(
            "{\"cookies\": {\"sid\": {\"value\": \"x\", \"domain\": null, ",
            "\"path\": \"/\", \"expires\": null, \"secure\": false}}}",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.cookies_dict_layout);
        assert_eq!(session.legacy, None);
    }

    #[test]
    fn legacy_header_dict_read_and_written_back_as_dict() {
        let source = "{\"headers\": {\"X-Api\": \"v1\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.headers_dict_layout);
        assert_eq!(session.legacy, Some(LegacyKind::Headers));
        assert_eq!(
            session.headers,
            vec![("X-Api".to_string(), "v1".to_string())]
        );
        let text = session.to_json();
        assert!(text.contains("\"headers\": {"));
        assert!(text.contains("\"X-Api\": \"v1\""));
    }

    #[test]
    fn empty_header_dict_preserves_layout_without_warning() {
        let source = "{\"headers\": {}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.headers_dict_layout);
        assert_eq!(session.legacy, None);
        assert!(session.to_json().contains("\"headers\": {}"));
    }

    #[test]
    fn header_dict_writeback_collapses_duplicates_to_last() {
        let source = "{\"headers\": {\"X\": \"a\"}}";
        let mut session = Session::from_text(source, &dummy_path(), 0).unwrap();
        // Simulate a request storing two same-named headers.
        session.headers = vec![
            ("X".to_string(), "one".to_string()),
            ("X".to_string(), "two".to_string()),
        ];
        let text = session.to_json();
        assert!(text.contains("\"X\": \"two\""));
        assert!(!text.contains("\"one\""));
    }

    #[test]
    fn mixed_legacy_cookies_and_headers_warn_once_as_cookies() {
        let source = concat!(
            "{\"headers\": {\"X\": \"v\"},",
            "\"cookies\": {\"s\": {\"value\": \"1\", \"domain\": \"\", ",
            "\"path\": \"/\", \"expires\": null, \"secure\": false}}}",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert_eq!(session.legacy, Some(LegacyKind::Cookies));
        assert!(session.legacy_warning("api", "host", true).is_some());
    }

    #[test]
    fn legacy_warning_absent_for_modern_lists() {
        let source = "{\"headers\": [], \"cookies\": []}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.legacy_warning("api", "host", true).is_none());
    }

    #[test]
    fn legacy_warning_text_names_upgrade_command() {
        let source = "{\"headers\": {\"X\": \"v\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let warning = session.legacy_warning("api", "example.org", true).unwrap();
        assert!(warning.contains("Outdated layout detected"));
        assert!(warning.contains("furl-manager sessions upgrade example.org api"));
        assert!(warning.contains("upgrade-all"));
    }

    #[test]
    fn legacy_warning_omits_upgrade_all_for_anonymous() {
        let source = "{\"headers\": {\"X\": \"v\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let warning = session
            .legacy_warning("/tmp/s.json", "host", false)
            .unwrap();
        assert!(!warning.contains("upgrade-all"));
        assert!(warning.contains("/tmp/s.json"));
    }

    // ---- Header exclusion rules ------------------------------------------

    #[test]
    fn header_exclusions_content_if_cookie_default_ua() {
        let mut session = Session::new();
        let headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("If-Match".to_string(), "abc".to_string()),
            ("Cookie".to_string(), "k=v".to_string()),
            ("User-Agent".to_string(), format!("furl/{}", crate::VERSION)),
            ("Authorization".to_string(), "Basic xxx".to_string()),
        ];
        session.update_headers_from_request(&headers, &[]);
        let names: Vec<&str> = session.headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"Accept"));
        assert!(names.contains(&"Authorization"));
        assert!(!names.contains(&"Content-Type"));
        assert!(!names.contains(&"If-Match"));
        assert!(!names.contains(&"Cookie"));
        assert!(!names.contains(&"User-Agent"));
    }

    #[test]
    fn custom_user_agent_is_persisted() {
        let mut session = Session::new();
        let headers = vec![("User-Agent".to_string(), "my-agent/2".to_string())];
        session.update_headers_from_request(&headers, &[]);
        assert_eq!(
            session.headers,
            vec![("User-Agent".to_string(), "my-agent/2".to_string())]
        );
    }

    #[test]
    fn cli_unset_header_is_skipped() {
        let mut session = Session::new();
        let headers = vec![("X-Gone".to_string(), "still-here".to_string())];
        session.update_headers_from_request(&headers, &["X-Gone".to_string()]);
        assert!(session.headers.is_empty());
    }

    #[test]
    fn same_named_request_headers_replace_stored_and_keep_others() {
        let mut session = Session::new();
        session.headers = vec![
            ("Foo".to_string(), "old".to_string()),
            ("Bar".to_string(), "keep".to_string()),
        ];
        let headers = vec![
            ("Foo".to_string(), "new1".to_string()),
            ("Foo".to_string(), "new2".to_string()),
        ];
        session.update_headers_from_request(&headers, &[]);
        // Both Foo occurrences stored, replacing the old one; Bar kept after.
        assert_eq!(
            session.headers,
            vec![
                ("Foo".to_string(), "new1".to_string()),
                ("Foo".to_string(), "new2".to_string()),
                ("Bar".to_string(), "keep".to_string()),
            ]
        );
    }

    // ---- Auth ------------------------------------------------------------

    #[test]
    fn new_style_auth_read_and_written() {
        let source = "{\"auth\": {\"type\": \"basic\", \"raw_auth\": \"u:p\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let auth = session.auth().unwrap();
        assert_eq!(auth.auth_type, "basic");
        assert_eq!(auth.raw_auth.as_deref(), Some("u:p"));
        let text = session.to_json();
        assert!(text.contains("\"type\": \"basic\""));
        assert!(text.contains("\"raw_auth\": \"u:p\""));
    }

    #[test]
    fn old_style_auth_read_and_collapsed_to_raw_on_write() {
        let source = "{\"auth\": {\"type\": \"basic\", \"username\": \"u\", \"password\": \"p\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let auth = session.auth().unwrap();
        assert_eq!(auth.username.as_deref(), Some("u"));
        assert_eq!(auth.password.as_deref(), Some("p"));
        // Written back in new style: raw_auth derived from user:pass.
        let text = session.to_json();
        assert!(text.contains("\"raw_auth\": \"u:p\""));
    }

    #[test]
    fn null_auth_type_yields_no_session_auth() {
        let source = "{\"auth\": {\"type\": null, \"username\": null, \"password\": null}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.auth().is_none());
    }

    #[test]
    fn set_auth_records_resolved_auth() {
        let mut session = Session::new();
        session.set_auth(SessionAuth {
            auth_type: "bearer".to_string(),
            raw_auth: Some("token".to_string()),
            username: None,
            password: None,
        });
        let text = session.to_json();
        assert!(text.contains("\"type\": \"bearer\""));
        assert!(text.contains("\"raw_auth\": \"token\""));
    }

    // ---- Cookie jar <-> session -----------------------------------------

    #[test]
    fn expired_cookies_pruned_on_load() {
        let source = concat!(
            "{\"cookies\": [\n",
            "  {\"name\": \"live\", \"value\": \"1\", \"domain\": \"h\", ",
            "\"path\": \"/\", \"expires\": 5000, \"secure\": false},\n",
            "  {\"name\": \"dead\", \"value\": \"2\", \"domain\": \"h\", ",
            "\"path\": \"/\", \"expires\": 100, \"secure\": false}\n",
            "]}",
        );
        // now = 1000: the 100-second cookie is expired, the 5000 one is not.
        let session = Session::from_text(source, &dummy_path(), 1000).unwrap();
        let names: Vec<&str> = session.cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["live"]);
    }

    #[test]
    fn update_cookies_from_jar_prunes_expired() {
        let mut jar = Jar::new();
        jar.insert(Cookie {
            name: "live".to_string(),
            value: "1".to_string(),
            domain: "h".to_string(),
            domain_attribute: false,
            explicit_none_domain: false,
            path: "/".to_string(),
            expires: Some(5000),
            secure: false,
        });
        jar.insert(Cookie {
            name: "dead".to_string(),
            value: "2".to_string(),
            domain: "h".to_string(),
            domain_attribute: false,
            explicit_none_domain: false,
            path: "/".to_string(),
            expires: Some(100),
            secure: false,
        });
        let mut session = Session::new();
        session.update_cookies_from_jar(&jar, 1000);
        let names: Vec<&str> = session.cookies.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["live"]);
    }

    #[test]
    fn load_into_jar_replays_cookies() {
        let source = concat!(
            "{\"cookies\": [{\"name\": \"sid\", \"value\": \"abc\", ",
            "\"domain\": \"h.example\", \"path\": \"/\", \"expires\": null, ",
            "\"secure\": false}]}",
        );
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        let mut jar = Jar::new();
        session.load_into_jar(&mut jar);
        assert_eq!(
            jar.header_for("http", "h.example", "/"),
            Some("sid=abc".into())
        );
    }

    // ---- Corrupt file & should_save --------------------------------------

    #[test]
    fn corrupt_session_file_is_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "{not json").unwrap();
        let result = Session::load(&path, 0);
        assert!(matches!(result, Err(SessionError::InvalidJson(_))));
    }

    #[test]
    fn non_object_top_level_is_fatal() {
        let result = Session::from_text("[1, 2, 3]", &dummy_path(), 0);
        assert!(matches!(result, Err(SessionError::InvalidJson(_))));
    }

    #[test]
    fn missing_file_is_fresh_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        let session = Session::load(&path, 0).unwrap();
        assert!(session.headers.is_empty());
        assert!(session.cookies.is_empty());
    }

    #[test]
    fn should_save_matrix() {
        // Existing file, read-only: never save.
        assert!(!should_save(true, true));
        // Existing file, writable: save.
        assert!(should_save(false, true));
        // Missing file, read-only: save (first-run bootstrap).
        assert!(should_save(true, false));
        // Missing file, writable: save.
        assert!(should_save(false, false));
    }

    // ---- Save creates dirs & round-trips through disk --------------------

    #[test]
    fn save_creates_parent_dirs_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path();
        let path = session_path("api", "example.org:8080", config);
        let mut session = Session::new();
        session.headers = vec![("X-Api".to_string(), "v1".to_string())];
        session.save(&path).unwrap();
        assert!(path.exists());
        let reloaded = Session::load(&path, 0).unwrap();
        assert_eq!(
            reloaded.headers,
            vec![("X-Api".to_string(), "v1".to_string())]
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_creates_dirs_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = session_path("api", "example.org", dir.path());
        Session::new().save(&path).unwrap();
        let sessions_dir = dir.path().join("sessions").join("example.org");
        let mode = std::fs::metadata(&sessions_dir)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn old_meta_version_survives_save() {
        let source = "{\"__meta__\": {\"furl\": \"0.0.1\"}}";
        let session = Session::from_text(source, &dummy_path(), 0).unwrap();
        assert!(session.to_json().contains("\"furl\": \"0.0.1\""));
    }
}
