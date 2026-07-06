//! The declarative option table.
//!
//! One table drives parsing, `--no-OPTION` negation, help rendering, and
//! the machine-readable argument export.

/// Identifies an option's storage destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptId {
    // Predefined content types
    Json,
    Form,
    Multipart,
    Boundary,
    Raw,
    // Content processing
    Compress,
    // Output processing
    Pretty,
    Style,
    Unsorted,
    Sorted,
    NoUnsorted,
    NoSorted,
    ResponseCharset,
    ResponseMime,
    FormatOptions,
    // Output options
    Print,
    Headers,
    Meta,
    Body,
    Verbose,
    All,
    HistoryPrint,
    Stream,
    Output,
    Download,
    Continue,
    Quiet,
    // Sessions
    Session,
    SessionReadOnly,
    // Authentication
    Auth,
    AuthType,
    IgnoreNetrc,
    // Network
    Offline,
    Proxy,
    Follow,
    MaxRedirects,
    MaxHeaders,
    Timeout,
    CheckStatus,
    PathAsIs,
    Chunked,
    // SSL
    Verify,
    Ssl,
    Ciphers,
    Cert,
    CertKey,
    CertKeyPass,
    // Troubleshooting
    IgnoreStdin,
    Help,
    Manual,
    Version,
    Traceback,
    DefaultScheme,
    Debug,
}

/// What parsing an occurrence does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Store the required value (last occurrence wins).
    Store,
    /// Boolean flag.
    Flag,
    /// Occurrence counter (`-v`, `-q`, `-x`).
    Count,
    /// Append the required value to a list.
    Append,
    /// Append a fixed string to a list (`--sorted` and friends).
    AppendConst(&'static str),
    /// Print something and exit 0 (`--help`, `--version`, `--manual`).
    Terminal,
}

/// Help group, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    ContentTypes,
    ContentProcessing,
    OutputProcessing,
    OutputOptions,
    Sessions,
    Authentication,
    Network,
    Ssl,
    Troubleshooting,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::ContentTypes => "Predefined content types",
            Group::ContentProcessing => "Content processing options",
            Group::OutputProcessing => "Output processing",
            Group::OutputOptions => "Output options",
            Group::Sessions => "Sessions",
            Group::Authentication => "Authentication",
            Group::Network => "Network",
            Group::Ssl => "SSL",
            Group::Troubleshooting => "Troubleshooting",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub id: OptId,
    /// Aliases as typed, shortest first (e.g. `-p`, `--pretty`).
    pub aliases: &'static [&'static str],
    pub metavar: Option<&'static str>,
    pub action: Action,
    pub choices: Option<&'static [&'static str]>,
    pub group: Group,
    /// Hidden options parse normally but stay out of help.
    pub hidden: bool,
    pub help: &'static str,
}

impl OptionSpec {
    /// The name shown in argparse-style messages: aliases joined by `/`.
    pub fn display_name(&self) -> String {
        self.aliases.join("/")
    }

    pub fn takes_value(&self) -> bool {
        matches!(self.action, Action::Store | Action::Append)
    }

    /// The long alias this option can be negated through.
    pub fn long_alias(&self) -> Option<&'static str> {
        self.aliases.iter().find(|a| a.starts_with("--")).copied()
    }
}

pub const PRETTY_CHOICES: &[&str] = &["all", "colors", "format", "none"];
pub const AUTH_TYPE_CHOICES: &[&str] = &["basic", "bearer", "digest"];
/// TLS versions the linked TLS backend can actually negotiate.
pub const SSL_CHOICES: &[&str] = &["tls1.2", "tls1.3"];

pub const SORTED_FORMAT_OPTIONS: &str = "headers.sort:true,json.sort_keys:true";
pub const UNSORTED_FORMAT_OPTIONS: &str = "headers.sort:false,json.sort_keys:false";

/// Every option, in help order.
pub const OPTIONS: &[OptionSpec] = &[
    // -- Predefined content types --------------------------------------
    OptionSpec {
        id: OptId::Json,
        aliases: &["-j", "--json"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::ContentTypes,
        hidden: false,
        help: "Serialize data items as a JSON object (the default). Sets \
               Content-Type and Accept to JSON when data is present.",
    },
    OptionSpec {
        id: OptId::Form,
        aliases: &["-f", "--form"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::ContentTypes,
        hidden: false,
        help: "Serialize data items as form fields. The Content-Type is \
               urlencoded, or multipart/form-data when files are attached.",
    },
    OptionSpec {
        id: OptId::Multipart,
        aliases: &["--multipart"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::ContentTypes,
        hidden: false,
        help: "Send a multipart/form-data request even without files.",
    },
    OptionSpec {
        id: OptId::Boundary,
        aliases: &["--boundary"],
        metavar: Some("BOUNDARY"),
        action: Action::Store,
        choices: None,
        group: Group::ContentTypes,
        hidden: false,
        help: "Use a custom boundary string for multipart/form-data requests.",
    },
    OptionSpec {
        id: OptId::Raw,
        aliases: &["--raw"],
        metavar: Some("RAW"),
        action: Action::Store,
        choices: None,
        group: Group::ContentTypes,
        hidden: false,
        help: "Pass raw request data without extra processing.",
    },
    // -- Content processing --------------------------------------------
    OptionSpec {
        id: OptId::Compress,
        aliases: &["-x", "--compress"],
        metavar: None,
        action: Action::Count,
        choices: None,
        group: Group::ContentProcessing,
        hidden: false,
        help: "Compress the request body with Deflate, when that saves \
               bytes. Give the flag twice to compress unconditionally.",
    },
    // -- Output processing ----------------------------------------------
    OptionSpec {
        id: OptId::Pretty,
        aliases: &["--pretty"],
        metavar: Some("{all,colors,format,none}"),
        action: Action::Store,
        choices: Some(PRETTY_CHOICES),
        group: Group::OutputProcessing,
        hidden: false,
        help: "Control output processing: 'all' colors and formats (the \
               terminal default), 'colors' or 'format' pick one, and \
               'none' passes output through untouched (the default when \
               redirecting).",
    },
    OptionSpec {
        id: OptId::Style,
        aliases: &["-s", "--style"],
        metavar: Some("STYLE"),
        action: Action::Store,
        choices: None, // validated against the style registry
        group: Group::OutputProcessing,
        hidden: false,
        help: "Output color scheme. 'auto' follows the terminal palette.",
    },
    OptionSpec {
        id: OptId::Unsorted,
        aliases: &["--unsorted"],
        metavar: None,
        action: Action::AppendConst(UNSORTED_FORMAT_OPTIONS),
        choices: None,
        group: Group::OutputProcessing,
        hidden: false,
        help: "Disable all output sorting; keep headers and JSON keys in \
               their original order.",
    },
    OptionSpec {
        id: OptId::Sorted,
        aliases: &["--sorted"],
        metavar: None,
        action: Action::AppendConst(SORTED_FORMAT_OPTIONS),
        choices: None,
        group: Group::OutputProcessing,
        hidden: false,
        help: "Re-enable all output sorting of headers and JSON keys.",
    },
    OptionSpec {
        id: OptId::NoUnsorted,
        aliases: &["--no-unsorted"],
        metavar: None,
        action: Action::AppendConst(SORTED_FORMAT_OPTIONS),
        choices: None,
        group: Group::OutputProcessing,
        hidden: true,
        help: "",
    },
    OptionSpec {
        id: OptId::NoSorted,
        aliases: &["--no-sorted"],
        metavar: None,
        action: Action::AppendConst(UNSORTED_FORMAT_OPTIONS),
        choices: None,
        group: Group::OutputProcessing,
        hidden: true,
        help: "",
    },
    OptionSpec {
        id: OptId::ResponseCharset,
        aliases: &["--response-charset"],
        metavar: Some("ENCODING"),
        action: Action::Store,
        choices: None, // validated as an encoding label
        group: Group::OutputProcessing,
        hidden: false,
        help: "Override the response encoding used for terminal display.",
    },
    OptionSpec {
        id: OptId::ResponseMime,
        aliases: &["--response-mime"],
        metavar: Some("MIME_TYPE"),
        action: Action::Store,
        choices: None, // validated as type/subtype
        group: Group::OutputProcessing,
        hidden: false,
        help: "Override the response MIME type used for coloring and \
               formatting on the terminal.",
    },
    OptionSpec {
        id: OptId::FormatOptions,
        aliases: &["--format-options"],
        metavar: Some("FORMAT_OPTIONS"),
        action: Action::Append,
        choices: None,
        group: Group::OutputProcessing,
        hidden: false,
        help: "Comma-separated 'section.key:value' pairs tuning output \
               formatting, merged left to right over the defaults.",
    },
    // -- Output options --------------------------------------------------
    OptionSpec {
        id: OptId::Print,
        aliases: &["-p", "--print"],
        metavar: Some("WHAT"),
        action: Action::Store,
        choices: None, // letters validated in post-processing
        group: Group::OutputOptions,
        hidden: false,
        help: "Select the exchange parts to output: 'H' request headers, \
               'B' request body, 'h' response headers, 'b' response body, \
               'm' response metadata.",
    },
    OptionSpec {
        id: OptId::Headers,
        aliases: &["-h", "--headers"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Print only the response headers (same as --print=h).",
    },
    OptionSpec {
        id: OptId::Meta,
        aliases: &["-m", "--meta"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Print only the response metadata (same as --print=m).",
    },
    OptionSpec {
        id: OptId::Body,
        aliases: &["-b", "--body"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Print only the response body (same as --print=b).",
    },
    OptionSpec {
        id: OptId::Verbose,
        aliases: &["-v", "--verbose"],
        metavar: None,
        action: Action::Count,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Print the whole exchange (request and response). Give the \
               flag twice to add response metadata as well.",
    },
    OptionSpec {
        id: OptId::All,
        aliases: &["--all"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Show every intermediary exchange too (redirects, retries \
               during authentication).",
    },
    OptionSpec {
        id: OptId::HistoryPrint,
        aliases: &["-P", "--history-print"],
        metavar: Some("WHAT"),
        action: Action::Store,
        choices: None,
        group: Group::OutputOptions,
        hidden: true,
        help: "",
    },
    OptionSpec {
        id: OptId::Stream,
        aliases: &["-S", "--stream"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Stream the response body line by line as it arrives.",
    },
    OptionSpec {
        id: OptId::Output,
        aliases: &["-o", "--output"],
        metavar: Some("FILE"),
        action: Action::Store,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Write output to FILE instead of the terminal.",
    },
    OptionSpec {
        id: OptId::Download,
        aliases: &["-d", "--download"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Save the response body to a file while printing the rest \
               of the exchange to the terminal.",
    },
    OptionSpec {
        id: OptId::Continue,
        aliases: &["-c", "--continue"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Resume an interrupted download (requires --download and \
               --output).",
    },
    OptionSpec {
        id: OptId::Quiet,
        aliases: &["-q", "--quiet"],
        metavar: None,
        action: Action::Count,
        choices: None,
        group: Group::OutputOptions,
        hidden: false,
        help: "Silence terminal output except errors and warnings. Give \
               the flag twice to silence warnings as well.",
    },
    // -- Sessions ---------------------------------------------------------
    OptionSpec {
        id: OptId::Session,
        aliases: &["--session"],
        metavar: Some("SESSION_NAME_OR_PATH"),
        action: Action::Store,
        choices: None,
        group: Group::Sessions,
        hidden: false,
        help: "Reuse and update a named session (headers, cookies, and \
               credentials persist between requests). A value with a path \
               separator is used as a session file path.",
    },
    OptionSpec {
        id: OptId::SessionReadOnly,
        aliases: &["--session-read-only"],
        metavar: Some("SESSION_NAME_OR_PATH"),
        action: Action::Store,
        choices: None,
        group: Group::Sessions,
        hidden: false,
        help: "Use a session without recording anything back to it.",
    },
    // -- Authentication ----------------------------------------------------
    OptionSpec {
        id: OptId::Auth,
        aliases: &["-a", "--auth"],
        metavar: Some("USER[:PASS] | TOKEN"),
        action: Action::Store,
        choices: None,
        group: Group::Authentication,
        hidden: false,
        help: "Credentials for the selected auth type. With a username \
               only, the password is prompted for; a trailing colon means \
               an empty password.",
    },
    OptionSpec {
        id: OptId::AuthType,
        aliases: &["-A", "--auth-type"],
        metavar: Some("{basic,bearer,digest}"),
        action: Action::Store,
        choices: Some(AUTH_TYPE_CHOICES),
        group: Group::Authentication,
        hidden: false,
        help: "The authentication mechanism (default: basic).",
    },
    OptionSpec {
        id: OptId::IgnoreNetrc,
        aliases: &["--ignore-netrc"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Authentication,
        hidden: false,
        help: "Never read credentials from .netrc.",
    },
    // -- Network -------------------------------------------------------------
    OptionSpec {
        id: OptId::Offline,
        aliases: &["--offline"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Build and print the request without sending it.",
    },
    OptionSpec {
        id: OptId::Proxy,
        aliases: &["--proxy"],
        metavar: Some("PROTOCOL:PROXY_URL"),
        action: Action::Append,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Route requests for PROTOCOL through PROXY_URL (e.g. \
               http:http://proxy:3128). Repeatable per protocol.",
    },
    OptionSpec {
        id: OptId::Follow,
        aliases: &["-F", "--follow"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Follow 30x redirects.",
    },
    OptionSpec {
        id: OptId::MaxRedirects,
        aliases: &["--max-redirects"],
        metavar: Some("MAX_REDIRECTS"),
        action: Action::Store,
        choices: None, // int-validated
        group: Group::Network,
        hidden: false,
        help: "The redirect limit used with --follow (default 30).",
    },
    OptionSpec {
        id: OptId::MaxHeaders,
        aliases: &["--max-headers"],
        metavar: Some("MAX_HEADERS"),
        action: Action::Store,
        choices: None, // int-validated
        group: Group::Network,
        hidden: false,
        help: "Abort when the response carries more than this many \
               headers (0 means no limit).",
    },
    OptionSpec {
        id: OptId::Timeout,
        aliases: &["--timeout"],
        metavar: Some("SECONDS"),
        action: Action::Store,
        choices: None, // float-validated
        group: Group::Network,
        hidden: false,
        help: "Give up when the connection is idle for this many seconds \
               (0 means never).",
    },
    OptionSpec {
        id: OptId::CheckStatus,
        aliases: &["--check-status"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Reflect the HTTP status in the exit code: 3 for 3xx (when \
               not following), 4 for 4xx, and 5 for 5xx responses.",
    },
    OptionSpec {
        id: OptId::PathAsIs,
        aliases: &["--path-as-is"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Keep dot segments (/../ and /./) in the URL path.",
    },
    OptionSpec {
        id: OptId::Chunked,
        aliases: &["--chunked"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Network,
        hidden: false,
        help: "Send the request body with chunked transfer encoding.",
    },
    // -- SSL -------------------------------------------------------------------
    OptionSpec {
        id: OptId::Verify,
        aliases: &["--verify"],
        metavar: Some("VERIFY"),
        action: Action::Store,
        choices: None,
        group: Group::Ssl,
        hidden: false,
        help: "Set to 'no' to skip server certificate verification, or \
               pass a CA bundle path (default: 'yes').",
    },
    OptionSpec {
        id: OptId::Ssl,
        aliases: &["--ssl"],
        metavar: Some("{tls1.2,tls1.3}"),
        action: Action::Store,
        choices: Some(SSL_CHOICES),
        group: Group::Ssl,
        hidden: false,
        help: "Force a specific TLS protocol version instead of \
               negotiating the best mutual one.",
    },
    OptionSpec {
        id: OptId::Ciphers,
        aliases: &["--ciphers"],
        metavar: Some("CIPHERS"),
        action: Action::Store,
        choices: None,
        group: Group::Ssl,
        hidden: false,
        help: "A comma-separated list of TLS cipher suite names to allow.",
    },
    OptionSpec {
        id: OptId::Cert,
        aliases: &["--cert"],
        metavar: Some("CERT"),
        action: Action::Store,
        choices: None, // readability-validated
        group: Group::Ssl,
        hidden: false,
        help: "A client certificate in PEM format, optionally including \
               the private key.",
    },
    OptionSpec {
        id: OptId::CertKey,
        aliases: &["--cert-key"],
        metavar: Some("CERT_KEY"),
        action: Action::Store,
        choices: None, // readability-validated
        group: Group::Ssl,
        hidden: false,
        help: "The private key for --cert, when kept in a separate file.",
    },
    OptionSpec {
        id: OptId::CertKeyPass,
        aliases: &["--cert-key-pass"],
        metavar: Some("CERT_KEY_PASS"),
        action: Action::Store,
        choices: None,
        group: Group::Ssl,
        hidden: false,
        help: "The passphrase for the private key; prompted for when the \
               key is encrypted and this is not given.",
    },
    // -- Troubleshooting ----------------------------------------------------------
    OptionSpec {
        id: OptId::IgnoreStdin,
        aliases: &["-I", "--ignore-stdin"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Never read piped standard input as the request body.",
    },
    OptionSpec {
        id: OptId::Help,
        aliases: &["--help"],
        metavar: None,
        action: Action::Terminal,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Show this help message and exit.",
    },
    OptionSpec {
        id: OptId::Manual,
        aliases: &["--manual"],
        metavar: None,
        action: Action::Terminal,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Show the full manual.",
    },
    OptionSpec {
        id: OptId::Version,
        aliases: &["--version"],
        metavar: None,
        action: Action::Terminal,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Show the program version and exit.",
    },
    OptionSpec {
        id: OptId::Traceback,
        aliases: &["--traceback"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Print the full error backtrace when something fails.",
    },
    OptionSpec {
        id: OptId::DefaultScheme,
        aliases: &["--default-scheme"],
        metavar: Some("DEFAULT_SCHEME"),
        action: Action::Store,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "The scheme to use when the URL does not carry one \
               (default: http).",
    },
    OptionSpec {
        id: OptId::Debug,
        aliases: &["--debug"],
        metavar: None,
        action: Action::Flag,
        choices: None,
        group: Group::Troubleshooting,
        hidden: false,
        help: "Print diagnostic information useful for bug reports, and \
               turn on --traceback.",
    },
];

/// Exact alias lookup (`-p` or `--pretty`).
pub fn find_exact(alias: &str) -> Option<&'static OptionSpec> {
    OPTIONS.iter().find(|spec| spec.aliases.contains(&alias))
}

/// Long options matching an unambiguous prefix, argparse-style. An exact
/// match always wins over being a prefix of something longer.
pub fn find_long_prefix(token: &str) -> Result<Option<&'static OptionSpec>, Vec<&'static str>> {
    if let Some(spec) = find_exact(token) {
        return Ok(Some(spec));
    }
    let matches: Vec<(&'static str, &'static OptionSpec)> = OPTIONS
        .iter()
        .flat_map(|spec| {
            spec.aliases
                .iter()
                .filter(|alias| alias.starts_with("--") && alias.starts_with(token))
                .map(move |alias| (*alias, spec))
        })
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].1)),
        // Several aliases of the SAME option would be unambiguous, but no
        // option here has two long aliases, so any multi-match is a clash.
        _ => Err(matches.into_iter().map(|(alias, _)| alias).collect()),
    }
}

/// Find the option a `--no-X` leftover inverts: exact long-alias match only.
pub fn find_negation_target(inverted: &str) -> Option<&'static OptionSpec> {
    OPTIONS.iter().find(|spec| {
        spec.aliases
            .iter()
            .any(|a| a.starts_with("--") && *a == inverted)
    })
}
