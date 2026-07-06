//! The header assembly engine.
//!
//! Headers come from three layers: the engine layer (Accept-Encoding,
//! Accept */*, Connection, computed Content-Length), the application
//! layer (User-Agent, content-type defaults, session and CLI headers),
//! and the connection layer (Host, synthesized last unless given).
//!
//! On the wire the engine-layer headers keep their canonical slots while
//! every application-layer header is emitted after them, in application
//! insertion order — reproducing the observable order of the
//! compatibility target.

use crate::cli::items::HeaderItem;

pub const ACCEPT_ENCODING_VALUE: &str = "gzip, deflate";
pub const DEFAULT_ACCEPT: &str = "*/*";

/// The application layer's ordered header set: an ordered multi-map with
/// case-insensitive names, deletion markers, and multi-value
/// accumulation.
#[derive(Debug, Default, Clone)]
pub struct HeaderSet {
    entries: Vec<(String, String)>,
    /// Names deleted with `Name:` — they suppress engine defaults and
    /// auto-synthesis until a later value re-adds the name.
    deleted: Vec<String>,
}

impl HeaderSet {
    pub fn new() -> HeaderSet {
        HeaderSet::default()
    }

    /// Add or replace a default/session-level header: replaces the value
    /// in place (keeping position) or appends.
    pub fn set(&mut self, name: &str, value: &str) {
        match self.position(name) {
            Some(at) => self.entries[at].1 = value.to_string(),
            None => self.entries.push((name.to_string(), value.to_string())),
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.position(name).is_some()
    }

    pub fn is_deleted(&self, name: &str) -> bool {
        self.deleted.iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// The effective single value for a name (last occurrence).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// Overlay one batch of CLI header items.
    ///
    /// A valued item replaces existing values the first time its name is
    /// seen in the batch and accumulates on repetition; a deletion item
    /// (`Name:`) wipes the name and suppresses defaults until a later
    /// value re-adds it. Values are stripped of surrounding ASCII
    /// whitespace.
    pub fn apply_cli_items(&mut self, items: &[HeaderItem]) {
        let mut replaced: Vec<String> = Vec::new();
        for item in items {
            match &item.value {
                Some(value) => {
                    let value = value.trim_matches(|c: char| c.is_ascii_whitespace());
                    let seen = replaced.iter().any(|n| n.eq_ignore_ascii_case(&item.name));
                    self.deleted.retain(|n| !n.eq_ignore_ascii_case(&item.name));
                    if seen {
                        // Repetition accumulates adjacent to the first
                        // occurrence's slot.
                        let after = self
                            .entries
                            .iter()
                            .rposition(|(n, _)| n.eq_ignore_ascii_case(&item.name))
                            .map(|at| at + 1)
                            .unwrap_or(self.entries.len());
                        self.entries
                            .insert(after, (item.name.clone(), value.to_string()));
                    } else {
                        replaced.push(item.name.clone());
                        // The first occurrence replaces a default in its
                        // slot, keeping the default's position.
                        match self.position(&item.name) {
                            Some(at) => self.entries[at].1 = value.to_string(),
                            None => self.entries.push((item.name.clone(), value.to_string())),
                        }
                    }
                }
                None => {
                    self.remove(&item.name);
                    replaced.retain(|n| !n.eq_ignore_ascii_case(&item.name));
                    self.deleted.push(item.name.clone());
                }
            }
        }
    }

    fn remove(&mut self, name: &str) {
        self.entries.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }

    /// All `(name, value)` pairs in application order.
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
}

/// The finished wire header list.
#[derive(Debug, Clone, Default)]
pub struct WireHeaders {
    pub entries: Vec<(String, String)>,
    /// True when `Host:` deletion was requested: rendering and the
    /// connection layer leave Host out. (The compatibility target cannot
    /// actually delete Host; making it work is a documented deviation.)
    pub skip_host: bool,
}

impl WireHeaders {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Assemble the final wire order: engine-layer slots first, application
/// headers after, Content-Length computed from the body.
///
/// `body_length` carries the computed length when a sized body exists.
/// Bodiless methods other than GET/HEAD get a bare `Content-Length: 0`,
/// dropped again for OPTIONS.
pub fn assemble(
    app_headers: &HeaderSet,
    method: &str,
    body_length: Option<u64>,
    chunked: bool,
    authorization: Option<String>,
) -> WireHeaders {
    let mut wire: Vec<(String, String)> = Vec::new();

    // Engine layer, canonical slots — suppressed when the application
    // layer supplies or deletes the name.
    for (name, value) in [
        ("Accept-Encoding", ACCEPT_ENCODING_VALUE),
        ("Accept", DEFAULT_ACCEPT),
        ("Connection", "keep-alive"),
    ] {
        if !app_headers.contains(name) && !app_headers.is_deleted(name) {
            wire.push((name.to_string(), value.to_string()));
        }
    }

    // Content-Length: computed independently of --chunked (both headers
    // can legitimately appear together, matching the reference stack).
    let method_upper = method.to_ascii_uppercase();
    match body_length {
        // A computed length always wins over a user-supplied one.
        Some(length) => wire.push(("Content-Length".into(), length.to_string())),
        None => {
            let implied_zero = !matches!(method_upper.as_str(), "GET" | "HEAD")
                && app_headers.get("Content-Length").is_none();
            if implied_zero && method_upper != "OPTIONS" {
                wire.push(("Content-Length".into(), "0".to_string()));
            }
        }
    }

    // Credentials computed from --auth/userinfo slot in after the body
    // headers (an explicit Authorization header stays application-side).
    if let Some(value) = authorization {
        wire.push(("Authorization".into(), value));
    }

    // Application layer, insertion order. A user Content-Length is
    // dropped when a computed one exists.
    for (name, value) in app_headers.pairs() {
        if name.eq_ignore_ascii_case("content-length") && body_length.is_some() {
            continue;
        }
        wire.push((name.to_string(), value.to_string()));
    }

    // The chunked marker lands after every application header.
    if chunked && !app_headers.contains("Transfer-Encoding") {
        wire.push(("Transfer-Encoding".into(), "chunked".into()));
    }

    WireHeaders {
        entries: wire,
        skip_host: app_headers.is_deleted("Host"),
    }
}
