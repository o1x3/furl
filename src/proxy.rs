//! Proxy routing: `--proxy` entries and environment variables, bypass
//! rules, and the resolved route a request takes.
//!
//! Selection mirrors the conventional client stack: explicit entries win
//! over the environment; `no_proxy` gates only the environment-derived
//! proxies; lookup tries `scheme://host`, `scheme`, `all://host`, `all`.

use base64::Engine as _;

/// One resolved proxy hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyRoute {
    /// Connect host (IPv6 kept bracketed, as URL hosts are).
    pub host: String,
    pub port: u16,
    /// The proxy itself is reached over TLS.
    pub https: bool,
    /// Ready `Basic …` Proxy-Authorization value from the URL userinfo.
    pub authorization: Option<String>,
}

/// Why a proxy URL cannot be used.
#[derive(Debug, PartialEq, Eq)]
pub enum ProxyError {
    /// socks4/socks5 URLs: not supported.
    Socks,
    /// Any other non-http(s) scheme.
    UnsupportedScheme(String),
    /// The proxy URL does not parse or has no host.
    Invalid(String),
}

/// The proxy route for a request, from `--proxy` entries (later entries
/// win per key) over environment variables. `None` means direct.
pub fn route_for(
    target: &url::Url,
    cli_entries: &[String],
) -> Result<Option<ProxyRoute>, ProxyError> {
    let env: Vec<(String, String)> = std::env::vars().collect();
    route_with_env(target, cli_entries, &env)
}

/// Testable core of [`route_for`].
pub fn route_with_env(
    target: &url::Url,
    cli_entries: &[String],
    env: &[(String, String)],
) -> Result<Option<ProxyRoute>, ProxyError> {
    let mut user: Vec<(String, String)> = Vec::new();
    for entry in cli_entries {
        if let Some((key, value)) = entry.split_once(':') {
            user.retain(|(k, _)| k != key);
            user.push((key.to_string(), value.to_string()));
        }
    }
    let scheme = target.scheme().to_string();

    // A user-supplied `no_proxy` key overrides the environment list; the
    // bypass gates only environment proxies — explicit ones always apply.
    let no_proxy_override = user
        .iter()
        .find(|(k, _)| k == "no_proxy")
        .map(|(_, v)| v.clone());
    let mut effective = user;
    if !should_bypass(target, no_proxy_override.as_deref(), env) {
        let env_map = environment_proxies(env);
        let from_env = env_map
            .iter()
            .find(|(k, _)| *k == scheme)
            .or_else(|| env_map.iter().find(|(k, _)| k == "all"))
            .map(|(_, v)| v.clone());
        if let Some(value) = from_env {
            if !effective.iter().any(|(k, _)| *k == scheme) {
                effective.push((scheme.clone(), value));
            }
        }
    }

    let host = target.host_str().unwrap_or_default();
    let keys = [
        format!("{scheme}://{host}"),
        scheme.clone(),
        format!("all://{host}"),
        "all".to_string(),
    ];
    let selected = keys
        .iter()
        .find_map(|key| effective.iter().find(|(k, _)| k == key).map(|(_, v)| v));
    match selected {
        Some(value) => parse_route(value).map(Some),
        None => Ok(None),
    }
}

/// The environment's proxy mapping (`http_proxy`, `HTTPS_PROXY`, …):
/// two passes so lowercase names win over uppercase, and an empty
/// lowercase value unsets the uppercase one. `REQUEST_METHOD` (CGI)
/// drops the uppercase-derived `http` entry.
fn environment_proxies(env: &[(String, String)]) -> Vec<(String, String)> {
    let mut map: Vec<(String, String)> = Vec::new();
    let put = |map: &mut Vec<(String, String)>, key: String, value: String| {
        map.retain(|(k, _)| *k != key);
        map.push((key, value));
    };
    for (name, value) in env {
        let lowered = name.to_lowercase();
        if !value.is_empty() && lowered.ends_with("_proxy") {
            let key = lowered[..lowered.len() - 6].to_string();
            put(&mut map, key, value.clone());
        }
    }
    if env.iter().any(|(name, _)| name == "REQUEST_METHOD") {
        map.retain(|(k, _)| k != "http");
    }
    for (name, value) in env {
        if name.ends_with("_proxy") {
            let lowered = name.to_lowercase();
            let key = lowered[..lowered.len() - 6].to_string();
            if value.is_empty() {
                map.retain(|(k, _)| *k != key);
            } else {
                put(&mut map, key, value.clone());
            }
        }
    }
    map
}

/// Does `no_proxy` (explicit or from the environment) exempt this host?
fn should_bypass(target: &url::Url, no_proxy: Option<&str>, env: &[(String, String)]) -> bool {
    let from_env = |key: &str| -> Option<String> {
        env.iter()
            .find(|(name, _)| name == key)
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
    };
    let no_proxy = match no_proxy {
        Some(value) => value.to_string(),
        None => match from_env("no_proxy").or_else(|| from_env("NO_PROXY")) {
            Some(value) => value,
            None => return false,
        },
    };
    let Some(host) = target.host_str() else {
        return true;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if no_proxy.is_empty() {
        return false;
    }

    let entries: Vec<String> = no_proxy
        .replace(' ', "")
        .split(',')
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect();
    if entries.iter().any(|e| e == "*") {
        return true;
    }

    if let Ok(address) = host.parse::<std::net::Ipv4Addr>() {
        for entry in &entries {
            if let Some((network, bits)) = parse_cidr(entry) {
                let mask = if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                };
                if (u32::from(address) & mask) == (u32::from(network) & mask) {
                    return true;
                }
            } else if host == entry {
                return true;
            }
        }
        return false;
    }

    let with_port = match target.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    for entry in &entries {
        let bare = entry.trim_start_matches('.');
        if host == bare || with_port == bare {
            return true;
        }
        let dotted = format!(".{bare}");
        if host.ends_with(&dotted) || with_port.ends_with(&dotted) {
            return true;
        }
    }
    false
}

/// `a.b.c.d/N` with N in 1..=32.
fn parse_cidr(entry: &str) -> Option<(std::net::Ipv4Addr, u32)> {
    let (address, bits) = entry.split_once('/')?;
    let bits: u32 = bits.parse().ok()?;
    if !(1..=32).contains(&bits) {
        return None;
    }
    Some((address.parse().ok()?, bits))
}

/// Parse a proxy URL (scheme defaulting to http) into a route.
fn parse_route(value: &str) -> Result<ProxyRoute, ProxyError> {
    let text = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let url = url::Url::parse(&text).map_err(|_| ProxyError::Invalid(value.to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        "socks4" | "socks4a" | "socks5" | "socks5h" => return Err(ProxyError::Socks),
        other => return Err(ProxyError::UnsupportedScheme(other.to_string())),
    }
    let host = url
        .host_str()
        .ok_or_else(|| ProxyError::Invalid(value.to_string()))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| ProxyError::Invalid(value.to_string()))?;
    let authorization = if url.username().is_empty() && url.password().is_none() {
        None
    } else {
        let user = percent_decode(url.username());
        let password = percent_decode(url.password().unwrap_or_default());
        let credentials = format!("{user}:{password}");
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())
        ))
    };
    Ok(ProxyRoute {
        host,
        port,
        https: url.scheme() == "https",
        authorization,
    })
}

/// Percent-decode a URL userinfo component (lossy UTF-8).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(value) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> url::Url {
        url::Url::parse(text).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn explicit_entry_selects_by_scheme() {
        let route = route_with_env(
            &url("http://example.org/"),
            &["http:http://127.0.0.1:8098".to_string()],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "127.0.0.1");
        assert_eq!(route.port, 8098);
        assert!(!route.https);
        assert!(route.authorization.is_none());
    }

    #[test]
    fn entries_split_at_the_first_colon() {
        // "http://example.org:http://x" keys as "http", like the CLI
        // grammar's first-colon split — the remainder is a bad proxy URL.
        let result = route_with_env(
            &url("http://example.org/"),
            &["http://example.org:http://specific:2".to_string()],
            &[],
        );
        assert!(matches!(result, Err(ProxyError::Invalid(_))));
    }

    #[test]
    fn later_entries_win_per_key() {
        let route = route_with_env(
            &url("http://example.org/"),
            &[
                "http:http://first:1".to_string(),
                "http:http://second:2".to_string(),
            ],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "second");
    }

    #[test]
    fn all_key_is_the_fallback() {
        let route = route_with_env(
            &url("https://example.org/"),
            &["all:http://everything:9".to_string()],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "everything");
    }

    #[test]
    fn environment_supplies_the_proxy_when_no_entry_matches() {
        let route = route_with_env(
            &url("http://example.org/"),
            &[],
            &env(&[("http_proxy", "http://envproxy:3128")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "envproxy");
        assert_eq!(route.port, 3128);
    }

    #[test]
    fn lowercase_environment_wins_and_empty_unsets() {
        let vars = env(&[
            ("HTTP_PROXY", "http://upper:1"),
            ("http_proxy", "http://lower:2"),
        ]);
        let route = route_with_env(&url("http://example.org/"), &[], &vars)
            .unwrap()
            .unwrap();
        assert_eq!(route.host, "lower");

        let vars = env(&[("HTTP_PROXY", "http://upper:1"), ("http_proxy", "")]);
        assert!(
            route_with_env(&url("http://example.org/"), &[], &vars)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn no_proxy_gates_environment_but_not_explicit_entries() {
        let vars = env(&[
            ("http_proxy", "http://envproxy:3128"),
            ("no_proxy", "example.org"),
        ]);
        assert!(
            route_with_env(&url("http://example.org/"), &[], &vars)
                .unwrap()
                .is_none()
        );
        // Explicit --proxy applies even when no_proxy matches.
        let route = route_with_env(
            &url("http://example.org/"),
            &["http:http://forced:1".to_string()],
            &vars,
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "forced");
    }

    #[test]
    fn no_proxy_matches_exact_dot_suffix_star_and_cidr() {
        let bypass = |host: &str, list: &str| {
            should_bypass(
                &url(&format!("http://{host}/")),
                None,
                &env(&[("no_proxy", list)]),
            )
        };
        assert!(bypass("example.org", "example.org"));
        assert!(bypass("sub.example.org", "example.org"));
        assert!(bypass("sub.example.org", ".example.org"));
        assert!(!bypass("notexample.org", "example.org"));
        assert!(bypass("anything.at.all", "*"));
        assert!(bypass("10.1.2.3", "10.1.0.0/16"));
        assert!(!bypass("10.2.2.3", "10.1.0.0/16"));
        assert!(bypass("10.1.2.3", "10.1.2.3"));
    }

    #[test]
    fn userinfo_becomes_basic_authorization_percent_decoded() {
        let route = route_with_env(
            &url("http://example.org/"),
            &["http:http://user:p%40ss@127.0.0.1:8098".to_string()],
            &[],
        )
        .unwrap()
        .unwrap();
        // user:p@ss
        assert_eq!(route.authorization.as_deref(), Some("Basic dXNlcjpwQHNz"));
    }

    #[test]
    fn schemeless_value_defaults_to_http() {
        let route = route_with_env(
            &url("http://example.org/"),
            &["http:127.0.0.1:8098".to_string()],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.host, "127.0.0.1");
        assert_eq!(route.port, 8098);
    }

    #[test]
    fn socks_and_unknown_schemes_are_rejected() {
        let socks = route_with_env(
            &url("http://example.org/"),
            &["http:socks5://127.0.0.1:1080".to_string()],
            &[],
        );
        assert_eq!(socks.unwrap_err(), ProxyError::Socks);
        let odd = route_with_env(
            &url("http://example.org/"),
            &["http:ftp://127.0.0.1:21".to_string()],
            &[],
        );
        assert_eq!(
            odd.unwrap_err(),
            ProxyError::UnsupportedScheme("ftp".to_string())
        );
    }

    #[test]
    fn https_default_port_is_443() {
        let route = route_with_env(
            &url("http://example.org/"),
            &["http:https://secureproxy".to_string()],
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(route.port, 443);
        assert!(route.https);
    }
}
