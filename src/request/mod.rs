//! Building a request from parsed arguments and processed items.

pub mod body;
pub mod headers;
pub mod urlencode;

#[cfg(test)]
mod tests;

use base64::Engine as _;

use crate::cli::args::{ParsedArgs, RequestType};
use crate::cli::items::RequestItems;
use crate::cli::nested_json::NestedJsonError;
use crate::cli::request_items::{Separator, split_item};
use crate::cli::urls::normalize_url;

use body::Body;
use headers::{HeaderSet, WireHeaders};

pub const JSON_CONTENT_TYPE: &str = "application/json";
pub const JSON_ACCEPT: &str = "application/json, */*;q=0.5";
pub const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded; charset=utf-8";

/// A request ready for rendering or sending.
#[derive(Debug)]
pub struct PreparedRequest {
    /// Uppercased wire method.
    pub method: String,
    pub url: url::Url,
    /// The authority as typed (userinfo stripped, explicit port kept) —
    /// the Host header value.
    pub host_netloc: String,
    /// `--path-as-is`: the original path replaces the normalized one.
    pub path_override: Option<String>,
    pub headers: WireHeaders,
    pub body: Option<Body>,
    pub chunked: bool,
}

impl PreparedRequest {
    /// The origin-form target for the request line.
    pub fn request_target(&self) -> String {
        let path = match &self.path_override {
            Some(original) => original.clone(),
            None => self.url.path().to_string(),
        };
        let path = if path.is_empty() {
            "/".to_string()
        } else {
            path
        };
        match self.url.query() {
            Some(query) => format!("{path}?{query}"),
            None => path,
        }
    }
}

#[derive(Debug)]
pub enum BuildError {
    /// Rendered as a usage error (usage block + exit 1).
    Usage(String),
    /// Rendered as a runtime error line (`furl: error: …`).
    InvalidUrl { url: String, reason: String },
    /// Nested-JSON errors carry their own annotated rendering.
    NestedJson(NestedJsonError),
    /// File access problems while materializing the body.
    Body(String),
    /// `-a user` without a password: the caller prompts (or errors when
    /// prompting is unavailable) before building.
    PasswordRequired { user: String },
}

/// Inputs resolved by the main flow before building.
pub struct BuildContext<'a> {
    pub args: &'a ParsedArgs,
    pub items: &'a RequestItems,
    /// Piped stdin bytes, when attachable (not a tty, not ignored).
    pub stdin_body: Option<Vec<u8>>,
    /// The default scheme after program-variant forcing.
    pub default_scheme: &'a str,
    pub version: &'a str,
}

pub fn build(context: &BuildContext<'_>) -> Result<PreparedRequest, BuildError> {
    let args = context.args;
    let items = context.items;

    let form_mode = matches!(
        args.request_type,
        Some(RequestType::Form) | Some(RequestType::Multipart)
    );
    let multipart_mode =
        args.request_type == Some(RequestType::Multipart) || (form_mode && !items.files.is_empty());

    if args.compress > 0 {
        if multipart_mode {
            return Err(BuildError::Usage(
                "Cannot combine --compress and --multipart.".to_string(),
            ));
        }
        if args.chunked {
            return Err(BuildError::Usage(
                "Cannot combine --compress and --chunked.".to_string(),
            ));
        }
    }

    // -- URL ------------------------------------------------------------
    let normalized = normalize_url(&args.url, context.default_scheme);
    let mut url = url::Url::parse(&normalized).map_err(|error| BuildError::InvalidUrl {
        url: normalized.clone(),
        reason: url_error_reason(error),
    })?;
    if url.host().is_none() {
        return Err(BuildError::InvalidUrl {
            url: normalized.clone(),
            reason: "No host supplied".to_string(),
        });
    }

    let userinfo = extract_userinfo(&mut url);
    let host_netloc = netloc_of(&normalized);
    let original_path = args.path_as_is.then(|| raw_path_of(&normalized));

    // Query parameters append after any query already in the URL.
    if !items.params.is_empty() {
        let encoded = urlencode::urlencode(&items.params);
        let merged = match url.query() {
            Some(existing) if !existing.is_empty() => format!("{existing}&{encoded}"),
            _ => encoded,
        };
        url.set_query(Some(&merged));
    }

    // -- Body (single-source rule) ---------------------------------------
    let data_items_present = !items.data.is_empty() || !items.files.is_empty();
    let source_count = usize::from(data_items_present)
        + usize::from(args.raw.is_some())
        + usize::from(context.stdin_body.is_some())
        + usize::from(items.body_file.is_some());
    if source_count > 1 {
        return Err(BuildError::Usage(
            "Request body (from stdin, --raw or a file) and request data \
             (key=value) cannot be mixed. Pass --ignore-stdin to let \
             key/value take priority."
                .to_string(),
        ));
    }

    let mut file_content_type: Option<String> = None;
    let boundary = args.boundary.clone().unwrap_or_else(body::random_boundary);
    let built_body: Option<Body> = if let Some(body_file) = &items.body_file {
        file_content_type = body::guess_mime(&body_file.path);
        let bytes = std::fs::read(&body_file.path)
            .map_err(|error| BuildError::Body(format!("{}: {error}", body_file.path.display())))?;
        Some(Body {
            bytes,
            boundary: None,
        })
    } else if let Some(raw) = &args.raw {
        Some(Body {
            bytes: raw.clone().into_bytes(),
            boundary: None,
        })
    } else if let Some(stdin) = &context.stdin_body {
        Some(Body {
            bytes: stdin.clone(),
            boundary: None,
        })
    } else if multipart_mode {
        Some(map_body_error(body::multipart_body(
            items,
            boundary.clone(),
        ))?)
    } else if form_mode {
        body::form_body(items)
    } else {
        map_body_error(body::json_body(items))?
    };

    // -- Headers -----------------------------------------------------------
    let mut app_headers = HeaderSet::new();
    app_headers.set("User-Agent", &format!("furl/{}", context.version));

    let data_present = !items.data.is_empty()
        || items.body_file.is_some()
        || args.raw.is_some()
        || context.stdin_body.is_some();
    let json_applies = !form_mode && (args.request_type == Some(RequestType::Json) || data_present);
    if json_applies {
        app_headers.set("Accept", JSON_ACCEPT);
        app_headers.set("Content-Type", JSON_CONTENT_TYPE);
    } else if form_mode && !multipart_mode {
        app_headers.set("Content-Type", FORM_CONTENT_TYPE);
    }
    if let Some(mime) = &file_content_type {
        app_headers.set("Content-Type", mime);
    }

    app_headers.apply_cli_items(&items.headers);

    if multipart_mode {
        match app_headers.get("Content-Type") {
            None => {
                app_headers.set(
                    "Content-Type",
                    &format!("multipart/form-data; boundary={boundary}"),
                );
            }
            Some(user_type) if !user_type.contains("boundary=") => {
                let with_boundary = format!("{user_type}; boundary={boundary}");
                app_headers.set("Content-Type", &with_boundary);
            }
            Some(_) => {} // user boundary parameter: kept verbatim
        }
    }

    // -- Method -------------------------------------------------------------
    let method = args
        .method
        .as_deref()
        .expect("method resolved before build")
        .to_ascii_uppercase();

    // -- Wire headers + auth ---------------------------------------------------
    let body_length = built_body.as_ref().map(|b| b.bytes.len() as u64);
    let mut wire = headers::assemble(&app_headers, &method, body_length, args.chunked);
    if let Some(authorization) = resolve_authorization(args, userinfo.as_ref())? {
        if wire.get("Authorization").is_none() {
            wire.entries.push(("Authorization".into(), authorization));
        }
    }

    Ok(PreparedRequest {
        method,
        url,
        host_netloc,
        path_override: original_path,
        headers: wire,
        body: built_body,
        chunked: args.chunked,
    })
}

fn map_body_error<T>(result: Result<T, body::BodyError>) -> Result<T, BuildError> {
    result.map_err(|error| match error {
        body::BodyError::NestedJson(e) => BuildError::NestedJson(e),
        body::BodyError::File { message } => BuildError::Body(message),
    })
}

fn url_error_reason(error: url::ParseError) -> String {
    match error {
        url::ParseError::EmptyHost => "No host supplied".to_string(),
        other => other.to_string(),
    }
}

/// Pull `user:password@` out of the URL, percent-decoded.
fn extract_userinfo(url: &mut url::Url) -> Option<(String, String)> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let decode = |text: &str| String::from_utf8_lossy(&percent_decode(text.as_bytes())).to_string();
    let user = decode(url.username());
    let password = decode(url.password().unwrap_or_default());
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some((user, password))
}

/// Decode percent escapes; malformed escapes stay literal.
fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let decoded = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| {
                let hi = (hex[0] as char).to_digit(16)?;
                let lo = (hex[1] as char).to_digit(16)?;
                Some((hi * 16 + lo) as u8)
            });
        match decoded {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    out
}

/// The authority component as typed: userinfo stripped, port kept.
fn netloc_of(normalized_url: &str) -> String {
    let after_scheme = match normalized_url.find("://") {
        Some(at) => &normalized_url[at + 3..],
        None => normalized_url,
    };
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    match authority.rfind('@') {
        Some(at) => authority[at + 1..].to_string(),
        None => authority.to_string(),
    }
}

/// The path exactly as typed (before normalization), for `--path-as-is`.
fn raw_path_of(normalized_url: &str) -> String {
    let after_scheme = match normalized_url.find("://") {
        Some(at) => &normalized_url[at + 3..],
        None => normalized_url,
    };
    let path_start = match after_scheme.find(['/', '?', '#']) {
        Some(at) if after_scheme.as_bytes()[at] == b'/' => at,
        _ => return String::new(),
    };
    let path_area = &after_scheme[path_start..];
    let path_end = path_area.find(['?', '#']).unwrap_or(path_area.len());
    path_area[..path_end].to_string()
}

/// Resolve the Authorization header for auth types that need no
/// challenge. Digest waits for the 401 exchange; netrc integration comes
/// with the network layer.
fn resolve_authorization(
    args: &ParsedArgs,
    userinfo: Option<&(String, String)>,
) -> Result<Option<String>, BuildError> {
    let auth_type = args.auth_type.as_deref().unwrap_or("basic");
    if let Some(auth) = &args.auth {
        return Ok(match auth_type {
            "basic" => {
                let (user, password) = split_credentials(auth);
                let Some(password) = password else {
                    return Err(BuildError::PasswordRequired { user });
                };
                Some(basic_authorization(&user, &password))
            }
            "bearer" => Some(format!("Bearer {auth}")),
            _ => None, // digest: challenge-driven
        });
    }
    // URL userinfo applies only when no explicit --auth-type was given.
    if args.auth_type.is_none() {
        if let Some((user, password)) = userinfo {
            return Ok(Some(basic_authorization(user, password)));
        }
    }
    Ok(None)
}

/// The host shown in the password prompt, from the (possibly shorthand)
/// URL argument.
pub fn host_for_prompt(url_argument: &str, default_scheme: &str) -> String {
    let normalized = normalize_url(url_argument, default_scheme);
    netloc_of(&normalized)
}

/// Split `user:password` on the first unescaped colon; no colon means
/// the password must be prompted for.
pub fn split_credentials(auth: &str) -> (String, Option<String>) {
    match split_item(auth, &[Separator::Header]) {
        Ok(split) => (split.key, Some(split.value)),
        Err(_) => (auth.to_string(), None),
    }
}

pub fn basic_authorization(user: &str, password: &str) -> String {
    let credentials = format!("{user}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes())
    )
}
