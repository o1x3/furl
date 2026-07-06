//! The parsed argument store and per-option value semantics.

use super::options::{OptId, OptionSpec};
use super::request_items::{ALL_SEPARATORS, Separator, split_item};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestType {
    Json,
    Form,
    Multipart,
}

/// Everything the raw parse produces. Post-processing (tty defaults,
/// method guessing, URL normalization, stream setup) happens elsewhere.
#[derive(Debug, Clone)]
pub struct ParsedArgs {
    pub method: Option<String>,
    pub url: String,
    pub request_items: Vec<String>,

    pub request_type: Option<RequestType>,
    pub boundary: Option<String>,
    pub raw: Option<String>,
    pub compress: u32,
    pub pretty: Option<String>,
    pub style: String,
    pub format_options: Vec<String>,
    pub response_charset: Option<String>,
    pub response_mime: Option<String>,
    pub print: Option<String>,
    pub verbose: u32,
    pub all: bool,
    pub history_print: Option<String>,
    pub stream: bool,
    pub output: Option<String>,
    pub download: bool,
    pub download_resume: bool,
    pub quiet: u32,
    pub session: Option<String>,
    pub session_read_only: Option<String>,
    pub auth: Option<String>,
    pub auth_type: Option<String>,
    pub ignore_netrc: bool,
    pub offline: bool,
    pub proxy: Vec<String>,
    pub follow: bool,
    pub max_redirects: i64,
    pub max_headers: i64,
    pub timeout: f64,
    pub check_status: bool,
    pub path_as_is: bool,
    pub chunked: bool,
    pub verify: String,
    pub ssl: Option<String>,
    pub ciphers: Option<String>,
    pub cert: Option<String>,
    pub cert_key: Option<String>,
    pub cert_key_pass: Option<String>,
    pub ignore_stdin: bool,
    pub traceback: bool,
    pub default_scheme: String,
    pub debug: bool,
}

impl Default for ParsedArgs {
    fn default() -> Self {
        ParsedArgs {
            method: None,
            url: String::new(),
            request_items: Vec::new(),
            request_type: None,
            boundary: None,
            raw: None,
            compress: 0,
            pretty: None,
            style: "auto".to_string(),
            format_options: Vec::new(),
            response_charset: None,
            response_mime: None,
            print: None,
            verbose: 0,
            all: false,
            history_print: None,
            stream: false,
            output: None,
            download: false,
            download_resume: false,
            quiet: 0,
            session: None,
            session_read_only: None,
            auth: None,
            auth_type: None,
            ignore_netrc: false,
            offline: false,
            proxy: Vec::new(),
            follow: false,
            max_redirects: 30,
            max_headers: 0,
            timeout: 0.0,
            check_status: false,
            path_as_is: false,
            chunked: false,
            verify: "yes".to_string(),
            ssl: None,
            ciphers: None,
            cert: None,
            cert_key: None,
            cert_key_pass: None,
            ignore_stdin: false,
            traceback: false,
            default_scheme: "http".to_string(),
            debug: false,
        }
    }
}

impl ParsedArgs {
    /// Store one occurrence of a value-less option (flags, counters,
    /// fixed-string appends).
    pub(crate) fn apply_flag(&mut self, spec: &OptionSpec, append_const: Option<&str>) {
        match spec.id {
            OptId::Json => self.request_type = Some(RequestType::Json),
            OptId::Form => self.request_type = Some(RequestType::Form),
            OptId::Multipart => self.request_type = Some(RequestType::Multipart),
            OptId::Compress => self.compress += 1,
            OptId::Unsorted | OptId::Sorted | OptId::NoUnsorted | OptId::NoSorted => {
                self.format_options
                    .push(append_const.expect("append-const option").to_string());
            }
            OptId::Headers => self.print = Some("h".to_string()),
            OptId::Meta => self.print = Some("m".to_string()),
            OptId::Body => self.print = Some("b".to_string()),
            OptId::Verbose => self.verbose += 1,
            OptId::All => self.all = true,
            OptId::Stream => self.stream = true,
            OptId::Download => self.download = true,
            OptId::Continue => self.download_resume = true,
            OptId::Quiet => self.quiet += 1,
            OptId::IgnoreNetrc => self.ignore_netrc = true,
            OptId::Offline => self.offline = true,
            OptId::Follow => self.follow = true,
            OptId::CheckStatus => self.check_status = true,
            OptId::PathAsIs => self.path_as_is = true,
            OptId::Chunked => self.chunked = true,
            OptId::IgnoreStdin => self.ignore_stdin = true,
            OptId::Traceback => self.traceback = true,
            OptId::Debug => self.debug = true,
            other => unreachable!("{other:?} is not a flag option"),
        }
    }

    /// Store one occurrence of a value-taking option, validating the
    /// value. Error strings are argparse-shaped fragments; the caller
    /// prefixes `argument <name>: `.
    pub(crate) fn apply_value(&mut self, spec: &OptionSpec, value: &str) -> Result<(), String> {
        if let Some(choices) = spec.choices {
            if !choices.contains(&value) {
                let listed = choices
                    .iter()
                    .map(|c| format!("'{c}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!("invalid choice: '{value}' (choose from {listed})"));
            }
        }
        match spec.id {
            OptId::Boundary => self.boundary = Some(value.to_string()),
            OptId::Raw => self.raw = Some(value.to_string()),
            OptId::Pretty => self.pretty = Some(value.to_string()),
            OptId::Style => self.style = value.to_string(),
            OptId::ResponseCharset => {
                if !crate::encoding::is_known_encoding(value) {
                    return Err(format!("'{value}' is not a supported encoding"));
                }
                self.response_charset = Some(value.to_string());
            }
            OptId::ResponseMime => {
                if value.split('/').count() != 2 {
                    return Err(format!(
                        "'{value}' doesn't look like a mime type; use type/subtype"
                    ));
                }
                self.response_mime = Some(value.to_string());
            }
            OptId::FormatOptions => self.format_options.push(value.to_string()),
            OptId::Print => self.print = Some(value.to_string()),
            OptId::HistoryPrint => self.history_print = Some(value.to_string()),
            OptId::Output => self.output = Some(value.to_string()),
            OptId::Session => {
                validate_session_name(value)?;
                self.session = Some(value.to_string());
            }
            OptId::SessionReadOnly => {
                validate_session_name(value)?;
                self.session_read_only = Some(value.to_string());
            }
            OptId::Auth => self.auth = Some(value.to_string()),
            OptId::AuthType => self.auth_type = Some(value.to_string()),
            OptId::Proxy => {
                if split_item(value, &[Separator::Header]).is_err() {
                    return Err(format!("'{value}' is not a valid value"));
                }
                self.proxy.push(value.to_string());
            }
            OptId::MaxRedirects => {
                self.max_redirects = value
                    .parse()
                    .map_err(|_| format!("invalid int value: '{value}'"))?;
            }
            OptId::MaxHeaders => {
                self.max_headers = value
                    .parse()
                    .map_err(|_| format!("invalid int value: '{value}'"))?;
            }
            OptId::Timeout => {
                self.timeout = value
                    .parse()
                    .map_err(|_| format!("invalid float value: '{value}'"))?;
            }
            OptId::Verify => self.verify = value.to_string(),
            OptId::Ssl => self.ssl = Some(value.to_string()),
            OptId::Ciphers => self.ciphers = Some(value.to_string()),
            OptId::Cert => {
                validate_readable(value)?;
                self.cert = Some(value.to_string());
            }
            OptId::CertKey => {
                validate_readable(value)?;
                self.cert_key = Some(value.to_string());
            }
            OptId::CertKeyPass => self.cert_key_pass = Some(value.to_string()),
            OptId::DefaultScheme => self.default_scheme = value.to_string(),
            other => unreachable!("{other:?} does not take a value"),
        }
        Ok(())
    }

    /// `--no-OPTION`: put the destination back to its default. Options
    /// sharing a destination (the print shortcuts, the request-type trio)
    /// share the reset too.
    pub(crate) fn reset(&mut self, id: OptId) {
        let defaults = ParsedArgs::default();
        match id {
            OptId::Json | OptId::Form | OptId::Multipart => self.request_type = None,
            OptId::Boundary => self.boundary = None,
            OptId::Raw => self.raw = None,
            OptId::Compress => self.compress = 0,
            OptId::Pretty => self.pretty = None,
            OptId::Style => self.style = defaults.style,
            // The sorted/unsorted family is registered (never swept), but
            // --no-format-options resets the accumulated list.
            OptId::Unsorted | OptId::Sorted | OptId::NoUnsorted | OptId::NoSorted => {}
            OptId::FormatOptions => self.format_options = Vec::new(),
            OptId::ResponseCharset => self.response_charset = None,
            OptId::ResponseMime => self.response_mime = None,
            OptId::Print | OptId::Headers | OptId::Meta | OptId::Body => self.print = None,
            OptId::Verbose => self.verbose = 0,
            OptId::All => self.all = false,
            OptId::HistoryPrint => self.history_print = None,
            OptId::Stream => self.stream = false,
            OptId::Output => self.output = None,
            OptId::Download => self.download = false,
            OptId::Continue => self.download_resume = false,
            OptId::Quiet => self.quiet = 0,
            OptId::Session => self.session = None,
            OptId::SessionReadOnly => self.session_read_only = None,
            OptId::Auth => self.auth = None,
            OptId::AuthType => self.auth_type = None,
            OptId::IgnoreNetrc => self.ignore_netrc = false,
            OptId::Offline => self.offline = false,
            OptId::Proxy => self.proxy = Vec::new(),
            OptId::Follow => self.follow = false,
            OptId::MaxRedirects => self.max_redirects = defaults.max_redirects,
            OptId::MaxHeaders => self.max_headers = defaults.max_headers,
            OptId::Timeout => self.timeout = defaults.timeout,
            OptId::CheckStatus => self.check_status = false,
            OptId::PathAsIs => self.path_as_is = false,
            OptId::Chunked => self.chunked = false,
            OptId::Verify => self.verify = defaults.verify,
            OptId::Ssl => self.ssl = None,
            OptId::Ciphers => self.ciphers = None,
            OptId::Cert => self.cert = None,
            OptId::CertKey => self.cert_key = None,
            OptId::CertKeyPass => self.cert_key_pass = None,
            OptId::IgnoreStdin => self.ignore_stdin = false,
            OptId::Traceback => self.traceback = false,
            OptId::DefaultScheme => self.default_scheme = defaults.default_scheme,
            OptId::Debug => self.debug = false,
            // Negating a terminal action is a harmless no-op.
            OptId::Help | OptId::Manual | OptId::Version => {}
        }
    }

    /// Validate positional request-item tokens (grammar only; files are
    /// opened during item processing).
    pub(crate) fn validate_item_token(token: &str) -> Result<(), String> {
        match split_item(token, ALL_SEPARATORS) {
            Ok(_) => Ok(()),
            Err(_) => Err(format!("'{token}' is not a valid value")),
        }
    }
}

/// Does this raw item token use a data separator (`=`, `:=`, `@`, `=@`,
/// `:=@`)? Method guessing counts these.
pub fn token_has_data_separator(token: &str) -> bool {
    split_item(token, ALL_SEPARATORS)
        .map(|split| split.separator.is_data())
        .unwrap_or(false)
}

fn validate_session_name(value: &str) -> Result<(), String> {
    let has_path_separator =
        value.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && value.contains('/'));
    if has_path_separator {
        return Ok(());
    }
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err("Session name contains invalid characters.".to_string())
    }
}

fn validate_readable(path: &str) -> Result<(), String> {
    match std::fs::File::open(path) {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("{path}: {error}")),
    }
}
