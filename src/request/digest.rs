//! HTTP Digest authentication: challenge parsing and response computation.
//!
//! Digest auth is challenge-driven: the first request goes out without an
//! `Authorization` header, the server replies `401` with a
//! `WWW-Authenticate: Digest …` challenge, and the request is replayed with
//! a computed `Authorization: Digest …` header. This module owns the two
//! pure pieces of that exchange — parsing the challenge and computing the
//! response — so the network layer can drive the retry without carrying any
//! crypto itself.
//!
//! The algorithm is RFC 2617 / RFC 7616 standard, but the *observable*
//! header (field order, which fields are quoted, that `algorithm` is echoed
//! verbatim and quoted, that the response digest always uses the literal
//! `auth` token rather than the challenge's `qop` value) is pinned to match
//! python-requests' `HTTPDigestAuth`, since the reference client delegates
//! to it. Deviating here would produce headers a picky server accepts from
//! requests but rejects from us.

use md5::Md5;
use sha2::{Digest as _, Sha256};

/// A parsed `WWW-Authenticate: Digest` challenge.
///
/// Only the fields we act on are named; anything else in the challenge
/// (`stale`, `domain`, `charset`, …) is parsed away and dropped because it
/// does not influence the response we compute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    /// The raw `qop` token list (e.g. `"auth"` or `"auth,auth-int"`), if the
    /// challenge offered one. Absent means legacy (RFC 2069) response.
    pub qop: Option<String>,
    /// Echoed back verbatim when present (requests quotes it in the header).
    pub opaque: Option<String>,
    /// The hash family. `None` means the server omitted it, which defaults
    /// to `MD5`; when present it is echoed back verbatim in the header even
    /// though it is matched case-insensitively.
    pub algorithm: Option<String>,
}

/// Parse the value of a `WWW-Authenticate` header — the part *after* the
/// `Digest ` scheme prefix (the caller strips the scheme). Returns `None`
/// only when the challenge lacks the two fields a response cannot be built
/// without: `realm` and `nonce`.
///
/// The grammar is a comma-separated list of `key=value` pairs where a value
/// may be a bare token or a double-quoted string; a quoted value may itself
/// contain commas, so splitting has to respect quotes rather than naively
/// splitting on `,`. Keys are matched case-insensitively (per RFC), values
/// are kept verbatim after unquoting.
pub fn parse_challenge(header_value: &str) -> Option<Challenge> {
    let pairs = parse_pairs(header_value);
    let get = |wanted: &str| {
        pairs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value.clone())
    };
    let realm = get("realm")?;
    let nonce = get("nonce")?;
    Some(Challenge {
        realm,
        nonce,
        qop: get("qop"),
        opaque: get("opaque"),
        algorithm: get("algorithm"),
    })
}

/// Split a comma-separated `key=value` list, honouring double-quoted values
/// (which may contain commas and equals signs) and trimming surrounding
/// whitespace. Backslash escapes inside quotes are unescaped so `\"` yields
/// a literal quote, matching how a well-behaved server would encode one.
fn parse_pairs(input: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip separators and leading whitespace before a key.
        while i < bytes.len() && (bytes[i] == b',' || bytes[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read the key up to '=' (or ',' for a valueless token we ignore).
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b',' {
            i += 1;
        }
        let key = input[key_start..i].trim().to_string();
        if i >= bytes.len() || bytes[i] == b',' {
            // A bare token with no value — nothing we consume, skip it.
            continue;
        }
        i += 1; // consume '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1; // consume opening quote
            let mut out = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1; // drop the backslash, keep the escaped byte
                }
                out.push(bytes[i] as char);
                i += 1;
            }
            i += 1; // consume closing quote (if present)
            out
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            input[start..i].trim().to_string()
        };
        if !key.is_empty() {
            pairs.push((key, value));
        }
    }
    pairs
}

/// Which hash family a challenge selects, after case-folding the algorithm
/// name and splitting off the `-SESS` suffix.
#[derive(Clone, Copy)]
enum HashKind {
    Md5,
    Sha256,
}

/// A resolved algorithm: the hash to use and whether the `-SESS` variant is
/// in effect (which folds the nonce and client nonce into `HA1`).
struct Algorithm {
    kind: HashKind,
    sess: bool,
}

impl Algorithm {
    /// Resolve the challenge's algorithm name. `None` defaults to `MD5`
    /// (RFC default). Unknown families fall back to `MD5` rather than
    /// failing, matching requests' lambda table which only special-cases the
    /// names it knows and would otherwise produce no header; here the caller
    /// only reaches this path for families we advertise support for, so the
    /// fallback is defensive.
    fn resolve(name: Option<&str>) -> Self {
        let upper = name.unwrap_or("MD5").to_ascii_uppercase();
        let sess = upper.ends_with("-SESS");
        let base = upper.strip_suffix("-SESS").unwrap_or(&upper);
        let kind = match base {
            "SHA-256" => HashKind::Sha256,
            // "MD5" and anything unrecognized.
            _ => HashKind::Md5,
        };
        Algorithm { kind, sess }
    }

    /// Hex-encode the hash of `input`. The digest crates share the `Digest`
    /// trait, so the only thing that varies by family is which hasher is
    /// instantiated; the lowercase-hex encoding is common.
    fn hash(&self, input: &str) -> String {
        match self.kind {
            HashKind::Md5 => hex(Md5::digest(input.as_bytes()).as_slice()),
            HashKind::Sha256 => hex(Sha256::digest(input.as_bytes()).as_slice()),
        }
    }
}

/// Lowercase hex encoding — digest bytes render this way in the header.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Whether a challenge's `qop` token list selects the `auth` quality of
/// protection. Returns `Some(true)` when the list contains `auth`,
/// `Some(false)` when a list is present but offers only other qualities
/// (e.g. `auth-int` alone), and `None` when the challenge omitted `qop`
/// entirely (legacy RFC 2069 path). A challenge that offers a `qop` we do
/// not implement produces no usable response — matching the reference,
/// which builds no header at all in that case.
fn qop_selects_auth(qop: Option<&str>) -> Option<bool> {
    qop.map(|list| {
        list.split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("auth"))
    })
}

/// Compute the `Authorization: Digest` header value for a request.
///
/// Returns `None` when the challenge offers a `qop` list that does not
/// include `auth` (the only quality of protection implemented): the
/// reference emits no header in that case and replays the request without
/// auth, so we mirror that rather than sending a header the server will
/// reject.
///
/// `nc` is the nonce count (1 for the first use of a nonce, incremented on
/// each reuse); `cnonce` is the client nonce, supplied by the caller so
/// tests can pin a fixed value for deterministic output. `uri` is the
/// request target (path plus `?query` when present), exactly as it appears
/// on the request line — not the full URL.
///
/// HA1 = H(user:realm:pass) — with the `-SESS` variants HA1 is then folded
/// again as H(HA1:nonce:cnonce). HA2 = H(method:uri). With `qop=auth` the
/// response is H(HA1:nonce:nc:cnonce:auth:HA2); without any qop it is the
/// legacy H(HA1:nonce:HA2). The `nc` is rendered as 8-digit lowercase hex,
/// unquoted.
pub fn authorization(
    challenge: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    nc: u32,
    cnonce: &str,
) -> Option<String> {
    // A qop list that offers only qualities we do not implement (e.g.
    // `auth-int`) yields no header at all, matching the reference.
    let uses_qop = match qop_selects_auth(challenge.qop.as_deref()) {
        Some(true) => true,
        None => false,
        Some(false) => return None,
    };

    let algorithm = Algorithm::resolve(challenge.algorithm.as_deref());
    let realm = &challenge.realm;
    let nonce = &challenge.nonce;

    let mut ha1 = algorithm.hash(&format!("{username}:{realm}:{password}"));
    let ha2 = algorithm.hash(&format!("{method}:{uri}"));
    if algorithm.sess {
        ha1 = algorithm.hash(&format!("{ha1}:{nonce}:{cnonce}"));
    }

    // A challenge that offers qop uses the RFC 2617 response with the client
    // nonce and count; one that omits it falls back to the RFC 2069 form.
    // The digest always carries the literal `auth` token, never the raw qop
    // value from the challenge (which may be `auth,auth-int`).
    let nc_hex = format!("{nc:08x}");
    let response = if uses_qop {
        algorithm.hash(&format!(
            "{ha1}:{nonce}:{nc_hex}:{cnonce}:auth:{ha2}"
        ))
    } else {
        algorithm.hash(&format!("{ha1}:{nonce}:{ha2}"))
    };

    // Field order and quoting are pinned to requests: username, realm,
    // nonce, uri, response, then opaque and algorithm (both quoted, echoed
    // only when the challenge carried them), then the qop/nc/cnonce trio
    // (only when qop is in play; qop and cnonce quoted, nc unquoted).
    let mut header = format!(
        "Digest username=\"{username}\", realm=\"{realm}\", nonce=\"{nonce}\", \
         uri=\"{uri}\", response=\"{response}\""
    );
    if let Some(opaque) = &challenge.opaque {
        header.push_str(&format!(", opaque=\"{opaque}\""));
    }
    if let Some(algorithm) = &challenge.algorithm {
        header.push_str(&format!(", algorithm=\"{algorithm}\""));
    }
    if uses_qop {
        header.push_str(&format!(
            ", qop=\"auth\", nc={nc_hex}, cnonce=\"{cnonce}\""
        ));
    }
    Some(header)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_challenge() {
        let challenge = parse_challenge(
            "realm=\"testrealm@host.com\", qop=\"auth,auth-int\", \
             nonce=\"abc\", opaque=\"xyz\", algorithm=MD5, stale=FALSE",
        )
        .expect("valid challenge");
        assert_eq!(challenge.realm, "testrealm@host.com");
        assert_eq!(challenge.nonce, "abc");
        assert_eq!(challenge.qop.as_deref(), Some("auth,auth-int"));
        assert_eq!(challenge.opaque.as_deref(), Some("xyz"));
        assert_eq!(challenge.algorithm.as_deref(), Some("MD5"));
    }

    #[test]
    fn parses_unquoted_and_bare_minimum() {
        let challenge = parse_challenge("realm=a, nonce=b").expect("valid");
        assert_eq!(challenge.realm, "a");
        assert_eq!(challenge.nonce, "b");
        assert_eq!(challenge.qop, None);
        assert_eq!(challenge.opaque, None);
        assert_eq!(challenge.algorithm, None);
    }

    #[test]
    fn quoted_value_may_contain_commas() {
        // A comma inside a quoted realm must not end the value.
        let challenge =
            parse_challenge("realm=\"quoted, comma\", nonce=b").expect("valid");
        assert_eq!(challenge.realm, "quoted, comma");
        assert_eq!(challenge.nonce, "b");
    }

    #[test]
    fn missing_realm_or_nonce_is_none() {
        assert!(parse_challenge("nonce=b").is_none());
        assert!(parse_challenge("realm=a").is_none());
        assert!(parse_challenge("").is_none());
    }

    #[test]
    fn keys_match_case_insensitively() {
        let challenge = parse_challenge("Realm=a, NONCE=b").expect("valid");
        assert_eq!(challenge.realm, "a");
        assert_eq!(challenge.nonce, "b");
    }

    // The expected header strings below were computed against
    // python-requests' HTTPDigestAuth with a fixed cnonce, so they pin exact
    // wire-parity behavior.

    const CNONCE: &str = "0a4f113b";
    const NONCE: &str = "dcd98b7102dd2f0e8b11d0f600bfb0c093";
    const REALM: &str = "testrealm@host.com";

    #[test]
    fn md5_with_qop_exact_header() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth".to_string()),
            opaque: None,
            algorithm: Some("MD5".to_string()),
        };
        let header = authorization(
            &challenge,
            "user",
            "pass",
            "GET",
            "/dir/index.html",
            1,
            CNONCE,
        )
        .expect("qop=auth challenge yields a header");
        assert_eq!(
            header,
            "Digest username=\"user\", realm=\"testrealm@host.com\", \
             nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", \
             uri=\"/dir/index.html\", \
             response=\"cab2df586c2172844e334bba85eb5a8a\", \
             algorithm=\"MD5\", qop=\"auth\", nc=00000001, cnonce=\"0a4f113b\""
        );
    }

    #[test]
    fn no_qop_legacy_header() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: None,
            opaque: None,
            algorithm: None,
        };
        let header = authorization(
            &challenge,
            "user",
            "pass",
            "GET",
            "/dir/index.html",
            1,
            CNONCE,
        )
        .expect("legacy challenge yields a header");
        // No qop → no algorithm/qop/nc/cnonce fields, RFC 2069 response.
        assert_eq!(
            header,
            "Digest username=\"user\", realm=\"testrealm@host.com\", \
             nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", \
             uri=\"/dir/index.html\", \
             response=\"304c72e9fdd046a0b6e0dc04d42b0aee\""
        );
    }

    #[test]
    fn sha256_with_qop_exact_header() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth".to_string()),
            opaque: None,
            algorithm: Some("SHA-256".to_string()),
        };
        let header = authorization(
            &challenge,
            "user",
            "pass",
            "GET",
            "/dir/index.html",
            1,
            CNONCE,
        )
        .expect("qop=auth challenge yields a header");
        assert_eq!(
            header,
            "Digest username=\"user\", realm=\"testrealm@host.com\", \
             nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", \
             uri=\"/dir/index.html\", \
             response=\"e9e454dd6930938c665c09ed186cbc3f29c6a8b356b20fb888708a713ba02f6d\", \
             algorithm=\"SHA-256\", qop=\"auth\", nc=00000001, cnonce=\"0a4f113b\""
        );
    }

    #[test]
    fn md5_sess_folds_nonce_into_ha1() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth".to_string()),
            opaque: None,
            algorithm: Some("MD5-sess".to_string()),
        };
        let header = authorization(
            &challenge,
            "user",
            "pass",
            "GET",
            "/dir/index.html",
            1,
            CNONCE,
        )
        .expect("qop=auth challenge yields a header");
        // Algorithm echoed verbatim (mixed case), response uses the folded HA1.
        assert_eq!(
            header,
            "Digest username=\"user\", realm=\"testrealm@host.com\", \
             nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", \
             uri=\"/dir/index.html\", \
             response=\"1f695223f3d5533625ab84bfb7cdbc8e\", \
             algorithm=\"MD5-sess\", qop=\"auth\", nc=00000001, cnonce=\"0a4f113b\""
        );
    }

    #[test]
    fn opaque_echoed_when_present() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth".to_string()),
            opaque: Some("op4que".to_string()),
            algorithm: Some("MD5".to_string()),
        };
        let header = authorization(
            &challenge, "user", "pass", "GET", "/dir/index.html", 1, CNONCE,
        )
        .expect("qop=auth challenge yields a header");
        // opaque sits between response and algorithm.
        assert!(header.contains(
            "response=\"cab2df586c2172844e334bba85eb5a8a\", opaque=\"op4que\", algorithm=\"MD5\""
        ));
    }

    #[test]
    fn nonce_count_renders_as_eight_hex_digits() {
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth".to_string()),
            opaque: None,
            algorithm: Some("MD5".to_string()),
        };
        let header = authorization(
            &challenge, "user", "pass", "GET", "/dir/index.html", 2, CNONCE,
        )
        .expect("qop=auth challenge yields a header");
        assert!(header.contains("nc=00000002"));
        assert!(header.contains("response=\"d41dae0190b474b13891472c88f44866\""));
    }

    #[test]
    fn qop_without_auth_yields_no_header() {
        // A challenge offering only `auth-int` (a quality of protection we do
        // not implement) must produce no header at all — the reference client
        // replays the request without an Authorization header in that case,
        // rather than sending one that claims `qop="auth"`.
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth-int".to_string()),
            opaque: None,
            algorithm: Some("MD5".to_string()),
        };
        let header = authorization(
            &challenge, "user", "pass", "GET", "/dir/index.html", 1, CNONCE,
        );
        assert_eq!(header, None);
    }

    #[test]
    fn qop_list_including_auth_uses_qop_path() {
        // `auth,auth-int` still selects the `auth` path (same output as a bare
        // `auth` challenge): the header carries the literal `auth` token.
        let challenge = Challenge {
            realm: REALM.to_string(),
            nonce: NONCE.to_string(),
            qop: Some("auth,auth-int".to_string()),
            opaque: None,
            algorithm: Some("MD5".to_string()),
        };
        let header = authorization(
            &challenge, "user", "pass", "GET", "/dir/index.html", 1, CNONCE,
        )
        .expect("auth present in qop list yields a header");
        assert!(header.contains(
            "response=\"cab2df586c2172844e334bba85eb5a8a\", \
             algorithm=\"MD5\", qop=\"auth\", nc=00000001, cnonce=\"0a4f113b\""
        ));
    }
}
