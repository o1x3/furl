//! The main program flow for `furl` and `furls`.

use std::io::{IsTerminal, Read, Write};

use crate::cli::args::ParsedArgs;
use crate::cli::items::{RequestItems, process_items};
use crate::cli::parser::{Outcome, UsageError, parse};
use crate::cookies::Jar;
use crate::errors::{runtime_error_line, usage_error_block};
use crate::output::message::{RequestParts, render_request, render_response_head};
use crate::request::{BuildContext, BuildError, PreparedRequest, build, split_credentials};
use crate::status::ExitStatus;
use crate::transport::{self, RawResponse, TransportError, TransportOptions, tls};
use crate::{Program, VERSION};

/// Valid `--print` letters: request headers/body, response
/// headers/body, metadata.
const PRINT_LETTERS: &str = "HBhbm";

pub fn run(program: Program) -> i32 {
    let cli_argv: Vec<String> = std::env::args().skip(1).collect();
    let program_name = match program {
        Program::Furl => "furl",
        Program::Furls => "furls",
    };

    // Config `default_options` are prepended to the user's argv, so CLI
    // tokens (coming later) win for last-wins options and accumulate for
    // counts and appends. A malformed config file is a warning, not fatal.
    let config_dir = crate::config::config_dir();
    let (config, config_warning) = crate::config::load(&config_dir);
    if let Some(warning) = config_warning {
        let message = match warning {
            crate::config::ConfigWarning::InvalidJson(m)
            | crate::config::ConfigWarning::Unreadable(m) => m,
        };
        eprintln!("{program_name}: warning: {message}");
    }
    let mut argv = config.default_options;
    argv.extend(cli_argv);

    // A literal --debug/--traceback token anywhere lets the parser's
    // internal status escape untranslated.
    let traceback_literal = argv.iter().any(|a| a == "--debug" || a == "--traceback");

    let outcome = match parse(&argv) {
        Ok(outcome) => outcome,
        Err(error) => {
            let code = if traceback_literal { 2 } else { 1 };
            return exit_usage(program_name, &error, code);
        }
    };
    let mut args = match outcome {
        Outcome::Args(args) => *args,
        Outcome::Version => {
            println!("{VERSION}");
            return ExitStatus::Success.code();
        }
        Outcome::Help | Outcome::Manual => {
            print!("{}", crate::help::full_help(program_name));
            return ExitStatus::Success.code();
        }
    };

    match execute(program, program_name, &mut args) {
        Ok(status) => status.code(),
        Err(failure) => report_failure(program_name, failure, args.traceback || args.debug),
    }
}

/// A failure on the way to (or during) the request.
enum Failure {
    Usage(String),
    Runtime {
        kind: String,
        message: String,
        status: ExitStatus,
    },
    Annotated(String),
}

impl Failure {
    fn runtime(kind: &str, message: impl Into<String>) -> Failure {
        Failure::Runtime {
            kind: kind.to_string(),
            message: message.into(),
            status: ExitStatus::Error,
        }
    }
}

fn report_failure(program_name: &str, failure: Failure, _traceback: bool) -> i32 {
    match failure {
        Failure::Usage(message) => {
            let error = UsageError {
                message,
                option: None,
            };
            exit_usage(program_name, &error, 1)
        }
        Failure::Runtime {
            kind,
            message,
            status,
        } => {
            eprint!("{}", runtime_error_line(program_name, &kind, &message));
            status.code()
        }
        Failure::Annotated(rendered) => {
            eprintln!("{rendered}");
            ExitStatus::Error.code()
        }
    }
}

fn exit_usage(program_name: &str, error: &UsageError, code: i32) -> i32 {
    eprint!(
        "{}",
        usage_error_block(program_name, &error.message, error.option)
    );
    eprintln!();
    code
}

fn execute(
    program: Program,
    program_name: &str,
    args: &mut ParsedArgs,
) -> Result<ExitStatus, Failure> {
    // -- Post-parse pipeline -------------------------------------------
    if args.debug {
        args.traceback = true;
    }

    let stdin_is_tty = std::io::stdin().is_terminal();
    let stdin_available = !args.ignore_stdin && !stdin_is_tty;

    if args.offline {
        args.download = false;
        args.download_resume = false;
    }
    if args.download_resume && !(args.download && args.output.is_some()) {
        return Err(Failure::Usage(
            "--continue only works with --download".to_string(),
        ));
    }

    // Print parts: validated even where not yet consumed.
    for (flag, value) in [
        ("--print", args.print.as_deref()),
        ("--history-print", args.history_print.as_deref()),
    ] {
        if let Some(letters) = value {
            let bad: String = letters
                .chars()
                .filter(|c| !PRINT_LETTERS.contains(*c))
                .collect();
            if !bad.is_empty() {
                return Err(Failure::Usage(format!(
                    "Unknown output options: {flag}={letters}"
                )));
            }
        }
    }

    // -- Method guessing and the positional shift ------------------------
    let has_data_separator = args
        .request_items
        .iter()
        .any(|t| crate::cli::args::token_has_data_separator(t));
    if let Some(method) = args.method.clone() {
        let is_method_like = !method.is_empty() && method.chars().all(|c| c.is_ascii_alphabetic());
        if !is_method_like {
            // The "method" was really the URL: the URL slot holds an item.
            let item = std::mem::replace(&mut args.url, method);
            // Unlike parse-time item validation, the shift error carries
            // no argparse-style prefix.
            ParsedArgs::validate_item_token(&item).map_err(Failure::Usage)?;
            let item_is_data = crate::cli::args::token_has_data_separator(&item);
            args.request_items.insert(0, item);
            args.method = Some(guess_method(
                stdin_available || args.raw.is_some(),
                item_is_data || has_data_separator,
            ));
        }
    } else {
        args.method = Some(guess_method(
            stdin_available || args.raw.is_some(),
            has_data_separator,
        ));
    }

    // -- Items -------------------------------------------------------------
    let items =
        process_items(&args.request_items, args.request_type).map_err(|error| match error {
            crate::cli::items::ItemError::Message(message) => Failure::Usage(message),
            crate::cli::items::ItemError::NestedJson(error) => {
                Failure::Annotated(error.to_string())
            }
        })?;

    // -- Auth password prompt ------------------------------------------------
    let auth_type = args.auth_type.as_deref().unwrap_or("basic");
    if let Some(auth) = args.auth.clone() {
        if matches!(auth_type, "basic" | "digest") {
            let (user, password) = split_credentials(&auth);
            if password.is_none() {
                if args.ignore_stdin {
                    return Err(Failure::Usage(
                        "Unable to prompt for passwords because --ignore-stdin is set.".to_string(),
                    ));
                }
                let host =
                    crate::request::host_for_prompt(&args.url, default_scheme(program, args));
                let prompt = format!("{program_name}: password for {user}@{host}: ");
                let password = rpassword::prompt_password(prompt).unwrap_or_default();
                // The split takes the first colon, so a password with
                // colons survives the round-trip verbatim.
                args.auth = Some(format!("{user}:{password}"));
            }
        }
    }

    // -- Body from stdin -----------------------------------------------------
    // Available stdin always counts as a body source — even empty — so
    // conflicts with --raw and data items surface, and an empty piped
    // body still triggers data-driven defaults.
    let stdin_body = if stdin_available {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| Failure::runtime("IOError", error.to_string()))?;
        Some(bytes)
    } else {
        None
    };

    // -- Session load -----------------------------------------------------------
    let scheme = default_scheme(program, args);
    let mut session_state = SessionState::open(program_name, args, &items, scheme)?;
    let session_headers = session_state
        .as_ref()
        .map(|s| s.session.headers().to_vec())
        .unwrap_or_default();
    let session_authorization = session_state
        .as_ref()
        .and_then(|s| s.session.auth())
        .and_then(session_auth_header);

    // -- netrc fallback ---------------------------------------------------------
    // With no `-a` and netrc not suppressed, credentials come from the
    // user's netrc file for the request host. A URL-userinfo request
    // already resolves auth inside build(), which takes priority.
    let netrc_authorization = if args.auth.is_none() && !args.ignore_netrc {
        let host = crate::request::host_for_prompt(&args.url, scheme);
        let host = crate::session::port_stripped_host(&host).to_string();
        crate::netrc::lookup(&host)
            .map(|auth| crate::request::basic_authorization(&auth.login, &auth.password))
    } else {
        None
    };

    // -- Build ------------------------------------------------------------------
    let request = build(&BuildContext {
        args,
        items: &items,
        stdin_body,
        default_scheme: scheme,
        version: VERSION,
        session_headers: &session_headers,
        session_authorization,
        netrc_authorization,
    })
    .map_err(|error| match error {
        BuildError::Usage(message) => Failure::Usage(message),
        BuildError::InvalidUrl { url, reason } => {
            Failure::runtime("InvalidURL", format!("Invalid URL '{url}': {reason}"))
        }
        BuildError::NestedJson(error) => Failure::Annotated(error.to_string()),
        BuildError::Body(message) => Failure::runtime("IOError", message),
        BuildError::PasswordRequired { user } => Failure::Usage(format!(
            "Unable to prompt for passwords because --ignore-stdin is set. \
             (username: {user})"
        )),
    })?;

    // Output formatting is resolved once — a malformed --format-options
    // is a usage error even in offline mode.
    let stdout_tty = std::io::stdout().is_terminal();
    let format_tty = stdout_tty && args.output.is_none() && !args.download;
    let mode = crate::output::format::PrettyMode::resolve(args.pretty.as_deref(), format_tty);
    let colors_group = matches!(
        mode,
        crate::output::format::PrettyMode::All | crate::output::format::PrettyMode::Colors
    );
    let color_depth = crate::output::color::detect_color_depth();
    let style = (colors_group && crate::output::color::colors_active(color_depth))
        .then(|| crate::output::color::resolve_style(&args.style, color_depth));
    let format = FormatContext {
        mode,
        options: crate::output::format::FormatOptions::from_occurrences(&args.format_options)
            .map_err(Failure::Usage)?,
        explicit_json: args.request_type == Some(crate::cli::args::RequestType::Json),
        response_mime: args.response_mime.clone(),
        style,
    };

    // -- Offline execution ---------------------------------------------------------
    if args.offline {
        let parts = offline_parts(args);
        let mut rendered: Vec<u8> = Vec::new();
        if parts.headers {
            let head = String::from_utf8_lossy(&render_request(
                &request,
                RequestParts {
                    headers: true,
                    body: false,
                },
            ))
            .trim_end_matches("\r\n\r\n")
            .to_string();
            rendered.extend_from_slice(format.head(&head, HeadKind::Request).as_bytes());
            rendered.extend_from_slice(b"\r\n\r\n");
        }
        if parts.body {
            if let Some(body) = &request.body {
                let content_type = request.headers.get("Content-Type").map(str::to_string);
                rendered
                    .extend_from_slice(&format.body(body.bytes.clone(), content_type.as_deref()));
            }
        }
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(&rendered).ok();
        if handle.is_terminal() && !rendered.is_empty() {
            handle.write_all(b"\n\n").ok();
        }
        handle.flush().ok();
        // Offline runs still persist request headers and auth (no cookies).
        if let Some(state) = &mut session_state {
            state.save(&request, &items, args, None);
        }
        return Ok(ExitStatus::Success);
    }

    execute_online(args, request, format, &items, session_state)
}

const BINARY_NOTICE: &[u8] = b"+-----------------------------------------+\n\
      | NOTE: binary data not shown in terminal |\n\
      +-----------------------------------------+";

/// Where terminal output goes: stdout, stderr (download mode routes the
/// non-body output there), an `--output` file, or nowhere.
enum Sink {
    Stdout,
    Stderr,
    File(std::fs::File),
    Null,
}

/// The sections a message may print.
#[derive(Default)]
struct Message {
    head: Option<String>,
    /// `Some` when the body section is selected (bytes may be empty).
    body: Option<Vec<u8>>,
    /// `Some(elapsed_text)` when the meta section is selected.
    meta: Option<String>,
}

/// Writes messages with the separator rules of the output pipeline.
struct Emitter {
    sink: Sink,
    tty: bool,
    /// The previous message selected its body section (drives the piped
    /// inter-message separator, even when the body was empty).
    previous_had_body: bool,
    /// A streamed request-body upload on a terminal forces the next
    /// separator even in tty mode.
    force_separator: bool,
    started: bool,
}

impl Emitter {
    fn write(&mut self, bytes: &[u8]) {
        match &mut self.sink {
            Sink::Stdout => {
                let stdout = std::io::stdout();
                stdout.lock().write_all(bytes).ok();
            }
            Sink::Stderr => {
                let stderr = std::io::stderr();
                stderr.lock().write_all(bytes).ok();
            }
            Sink::File(file) => {
                file.write_all(bytes).ok();
            }
            Sink::Null => {}
        }
    }

    /// Emit one message. Every message in the exchange stream passes
    /// through here — even ones that print nothing — because each updates
    /// the body-carry state that drives the inter-message separator.
    fn message(&mut self, message: Message) {
        let prints_anything =
            message.head.is_some() || message.body.is_some() || message.meta.is_some();
        // Inter-message separator: before a printing message when the
        // previous message selected a body section, if piped (or forced
        // by a streamed upload on a terminal).
        if self.previous_had_body && prints_anything && (self.force_separator || !self.tty) {
            self.write(b"\n\n");
        }
        self.force_separator = false;

        if let Some(head) = &message.head {
            self.write(head.clone().as_bytes());
            self.write(b"\r\n\r\n");
        }

        let mut printed_bytes = false;
        if let Some(body) = &message.body {
            if !body.is_empty() {
                if self.tty && body.contains(&0) {
                    if message.head.is_some() {
                        self.write(b"\n");
                    }
                    self.write(BINARY_NOTICE);
                } else {
                    self.write(body);
                }
                printed_bytes = true;
            }
        }

        if let Some(meta) = &message.meta {
            // The meta text already carries its "Elapsed time: …" label
            // (and any coloring); the writer only supplies the separators.
            self.write(format!("\n\n{meta}\n\n").as_bytes());
        } else if self.tty && printed_bytes {
            // On a terminal, a body-printing message ends with a blank
            // line (unless meta already supplied its trailing separator).
            self.write(b"\n\n");
        }

        // The body section being *selected* carries the separator, even
        // when the body was empty.
        self.previous_had_body = message.body.is_some();
        self.started = true;
    }

    fn finish(&mut self) {
        match &mut self.sink {
            Sink::Stdout => {
                std::io::stdout().flush().ok();
            }
            Sink::Stderr => {
                std::io::stderr().flush().ok();
            }
            Sink::File(file) => {
                file.flush().ok();
            }
            Sink::Null => {}
        }
    }
}

/// The effective print letters for online exchanges.
fn online_letters(args: &ParsedArgs, stdout_tty: bool) -> String {
    if let Some(explicit) = &args.print {
        return explicit.clone();
    }
    let mut letters = match args.verbose {
        0 if stdout_tty => "hb".to_string(),
        0 => "b".to_string(),
        1 => "HBhb".to_string(),
        _ => "HBhbm".to_string(),
    };
    if args.download {
        letters.retain(|c| c != 'b');
    }
    letters
}

fn resolve_tls(args: &ParsedArgs) -> tls::TlsOptions {
    let verify = match args.verify.to_ascii_lowercase().as_str() {
        "no" | "false" => tls::Verification::Insecure,
        "yes" | "true" => tls::Verification::Platform,
        _ => tls::Verification::CaBundle(crate::paths::expand_tilde(&args.verify)),
    };
    let version = match args.ssl.as_deref() {
        Some("tls1.2") => Some(tls::TlsVersion::Tls12),
        Some("tls1.3") => Some(tls::TlsVersion::Tls13),
        _ => None,
    };
    tls::TlsOptions {
        verify,
        version,
        client_cert: args.cert.as_deref().map(crate::paths::expand_tilde),
        client_key: args.cert_key.as_deref().map(crate::paths::expand_tilde),
    }
}

fn transport_failure(error: TransportError, timeout: f64) -> Failure {
    match error {
        TransportError::Timeout => Failure::Runtime {
            kind: "".to_string(),
            message: format!(
                "Request timed out ({}s).",
                crate::json::dumps(
                    &crate::json::Value::from(timeout),
                    &crate::json::DumpOptions::default()
                )
            ),
            status: ExitStatus::ErrorTimeout,
        },
        TransportError::Connection(message) => Failure::runtime("ConnectionError", message),
        TransportError::Tls(message) => Failure::runtime("SSLError", message),
        TransportError::Protocol(message) => Failure::runtime("ConnectionError", message),
        TransportError::TooManyHeaders(count) => {
            Failure::runtime("ConnectionError", format!("got more than {count} headers"))
        }
    }
}

fn is_redirect_status(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// The meta section's elapsed-time value: whole seconds with a
/// nine-digit fractional part, then `s` (e.g. `0.004482167s`).
fn format_elapsed(elapsed: std::time::Duration) -> String {
    format!("{}.{:09}s", elapsed.as_secs(), elapsed.subsec_nanos())
}

/// Rewrite the request for the next hop of a redirect chain.
fn rebuild_for_redirect(
    mut request: PreparedRequest,
    response: &RawResponse,
    jar: &Jar,
) -> Result<PreparedRequest, Failure> {
    let location = response
        .header("Location")
        .expect("redirect checked for Location");
    let previous_host = request.url.host_str().map(str::to_string);
    let previous_fragment = request.url.fragment().map(str::to_string);
    let target = request.url.join(&location).map_err(|error| {
        Failure::runtime("InvalidURL", format!("Invalid URL '{location}': {error}"))
    })?;
    request.url = target;
    if request.url.fragment().is_none() {
        if let Some(fragment) = previous_fragment {
            request.url.set_fragment(Some(&fragment));
        }
    }
    request.path_override = None;

    // Method rewriting: 303 (and browser-compatible 301/302 for POST)
    // turn into bodyless GETs.
    let method_changes = match response.status {
        303 => request.method != "HEAD",
        301 | 302 => request.method == "POST",
        _ => false,
    };
    if method_changes {
        request.method = "GET".to_string();
        request.body = None;
        request.chunked = false;
        request.headers.entries.retain(|(name, _)| {
            !(name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("content-type")
                || name.eq_ignore_ascii_case("transfer-encoding"))
        });
    }

    // Credentials never travel to a different host.
    let host = request.url.host_str().unwrap_or_default().to_string();
    if previous_host.as_deref() != Some(host.as_str()) {
        request
            .headers
            .entries
            .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
    }

    // The Cookie header is rebuilt from the jar on every hop.
    request
        .headers
        .entries
        .retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
    if let Some(value) = jar.header_for(request.url.scheme(), &host, request.url.path()) {
        request.headers.entries.push(("Cookie".to_string(), value));
    }

    // Host header value for the new target.
    let default_port = request.url.port_or_known_default();
    let explicit_port = request.url.port();
    request.host_netloc = match explicit_port {
        Some(port) if Some(port) != default_port => format!("{host}:{port}"),
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    Ok(request)
}

/// Which message a head belongs to (colored differently for requests
/// versus responses).
#[derive(Clone, Copy)]
enum HeadKind {
    Request,
    Response,
}

/// The resolved output-formatting configuration for one run.
struct FormatContext {
    mode: crate::output::format::PrettyMode,
    options: crate::output::format::FormatOptions,
    explicit_json: bool,
    response_mime: Option<String>,
    /// The resolved color style (from `--style` + terminal depth). `None`
    /// when the colors group is inactive or the terminal has no color.
    style: Option<crate::output::color::Style>,
}

impl FormatContext {
    fn format_active(&self) -> bool {
        crate::output::format::format_group_active(self.mode)
    }

    fn colors_active(&self) -> bool {
        self.style.is_some()
            && matches!(
                self.mode,
                crate::output::format::PrettyMode::All | crate::output::format::PrettyMode::Colors
            )
    }

    /// The format group sorts a head block's header lines; the colors
    /// group then highlights it.
    fn head(&self, rendered: &str, kind: HeadKind) -> String {
        let formatted = if self.format_active() {
            crate::output::format::sort_header_lines(rendered, &self.options)
        } else {
            rendered.to_string()
        };
        match &self.style {
            Some(style) if self.colors_active() => match kind {
                HeadKind::Request => crate::output::color::colorize_request_head(&formatted, style),
                HeadKind::Response => {
                    crate::output::color::colorize_response_head(&formatted, style)
                }
            },
            _ => formatted,
        }
    }

    /// Reformat and then highlight a body, decoding lossily for the
    /// processors and re-encoding to bytes.
    fn body(&self, bytes: Vec<u8>, content_type: Option<&str>) -> Vec<u8> {
        if !self.format_active() && !self.colors_active() {
            return bytes;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mime =
            crate::output::format::effective_mime(content_type, self.response_mime.as_deref());
        let formatted = if self.format_active() {
            crate::output::format::format_body(
                &text,
                mime.as_deref(),
                self.explicit_json,
                &self.options,
                self.mode,
            )
        } else {
            text.to_string()
        };
        match &self.style {
            Some(style) if self.colors_active() => {
                crate::output::color::colorize_body(&formatted, mime.as_deref(), style).into_bytes()
            }
            _ => formatted.into_bytes(),
        }
    }

    /// Highlight the meta section text when the colors group is active.
    fn meta(&self, text: &str) -> String {
        match &self.style {
            Some(style) if self.colors_active() => crate::output::color::colorize_meta(text, style),
            _ => text.to_string(),
        }
    }
}

fn execute_online(
    args: &ParsedArgs,
    request: PreparedRequest,
    format: FormatContext,
    items: &RequestItems,
    mut session_state: Option<SessionState>,
) -> Result<ExitStatus, Failure> {
    let options = TransportOptions {
        timeout: (args.timeout > 0.0).then(|| std::time::Duration::from_secs_f64(args.timeout)),
        tls: resolve_tls(args),
        max_headers: usize::try_from(args.max_headers.max(0)).unwrap_or(0),
    };
    let follow = args.follow || args.download;
    let show_all = args.all || args.verbose > 0;
    let stdout_tty = std::io::stdout().is_terminal();
    let mut letters = online_letters(args, stdout_tty);

    // Download mode routes every non-body part to stderr and sends the
    // body to a file instead of the message printer.
    let (sink, tty) = if args.download {
        letters.retain(|c| c != 'b');
        let stderr_tty = std::io::stderr().is_terminal();
        let sink = if args.quiet > 0 {
            Sink::Null
        } else {
            Sink::Stderr
        };
        (sink, stderr_tty && args.quiet == 0)
    } else {
        // Quiet nulls terminal output, except an explicit --output file
        // (without --download) still receives it.
        let quiet_nulls_output = args.quiet > 0 && args.output.is_none();
        let sink = if quiet_nulls_output {
            Sink::Null
        } else if let Some(path) = &args.output {
            let file = std::fs::File::create(path)
                .map_err(|error| Failure::runtime("IOError", format!("{path}: {error}")))?;
            Sink::File(file)
        } else {
            Sink::Stdout
        };
        let tty = stdout_tty && args.output.is_none() && args.quiet == 0;
        (sink, tty)
    };
    let mut emitter = Emitter {
        sink,
        tty,
        previous_had_body: false,
        force_separator: false,
        started: false,
    };

    let mut jar = Jar::new();
    // Seed the jar with the session's stored cookies, and put them on the
    // first request's Cookie header.
    if let Some(state) = &session_state {
        state.session.load_into_jar(&mut jar);
    }
    let mut current = request;
    if let Some(value) = jar.header_for(
        current.url.scheme(),
        current.url.host_str().unwrap_or_default(),
        current.url.path(),
    ) {
        current
            .headers
            .entries
            .retain(|(name, _)| !name.eq_ignore_ascii_case("cookie"));
        current.headers.entries.push(("Cookie".to_string(), value));
    }
    // The initial download URL drives the fallback filename.
    let download_url = current.url.clone();
    let resume_offset = download_resume_offset(args);
    if args.download {
        // Identity encoding keeps byte counts and resume offsets exact.
        current
            .headers
            .entries
            .retain(|(name, _)| !name.eq_ignore_ascii_case("accept-encoding"));
        current
            .headers
            .entries
            .push(("Accept-Encoding".to_string(), "identity".to_string()));
        if let Some(offset) = resume_offset {
            current
                .headers
                .entries
                .push(("Range".to_string(), format!("bytes={offset}-")));
        }
    }
    // Digest auth answers a 401 challenge with one computed retry.
    let digest_credentials = (args.auth_type.as_deref() == Some("digest"))
        .then_some(args.auth.as_deref())
        .flatten()
        .map(split_credentials);
    let mut digest_answered = false;

    let mut hops: i64 = 0;
    loop {
        let started_at = std::time::Instant::now();
        let response =
            transport::send(&current, &options).map_err(|e| transport_failure(e, args.timeout))?;
        let elapsed = started_at.elapsed();

        let host = current.url.host_str().unwrap_or_default().to_string();
        for (name, value) in &response.headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                jar.store(&host, &String::from_utf8_lossy(value));
            }
        }

        // A 401 Digest challenge is answered once, replaying the same
        // request with a computed Authorization header (skipped when the
        // challenge offers no quality of protection we can satisfy).
        let digest_authorization = (!digest_answered && response.status == 401)
            .then_some(digest_credentials.as_ref())
            .flatten()
            .and_then(|(user, password)| {
                let challenge = digest_challenge_header(&response)
                    .and_then(|value| crate::request::digest::parse_challenge(&value))?;
                crate::request::digest::authorization(
                    &challenge,
                    user,
                    password.as_deref().unwrap_or_default(),
                    &current.method,
                    &current.request_target(),
                    1,
                    &digest_cnonce(),
                )
            });
        let wants_digest = digest_authorization.is_some();

        let wants_redirect =
            follow && is_redirect_status(response.status) && response.header("Location").is_some();
        let over_limit = wants_redirect && args.max_redirects > 0 && hops >= args.max_redirects;
        if over_limit {
            emitter.finish();
            return Err(Failure::Runtime {
                kind: "TooManyRedirects".to_string(),
                message: format!(
                    "Too many redirects (--max-redirects={}).",
                    args.max_redirects
                ),
                status: ExitStatus::ErrorTooManyRedirects,
            });
        }
        let is_final = !wants_redirect && !wants_digest;

        // The request message is always in the stream — intermediary
        // requests print (when H/B are selected) even without --all, and
        // every message updates the inter-message separator state.
        let request_content_type = current.headers.get("Content-Type").map(str::to_string);
        emitter.message(Message {
            head: letters.contains('H').then(|| {
                format.head(
                    String::from_utf8_lossy(&render_request(
                        &current,
                        RequestParts {
                            headers: true,
                            body: false,
                        },
                    ))
                    .trim_end_matches("\r\n\r\n"),
                    HeadKind::Request,
                )
            }),
            body: letters.contains('B').then(|| {
                let bytes = current
                    .body
                    .as_ref()
                    .map(|b| b.bytes.clone())
                    .unwrap_or_default();
                format.body(bytes, request_content_type.as_deref())
            }),
            meta: None,
        });

        // Intermediary responses print only with --all; the final one
        // always does.
        if is_final || show_all {
            let response_content_type = response.header("Content-Type");
            emitter.message(Message {
                head: letters
                    .contains('h')
                    .then(|| format.head(&render_response_head(&response), HeadKind::Response)),
                body: letters.contains('b').then(|| {
                    let body = format.body(
                        transport::decoded_body(&response),
                        response_content_type.as_deref(),
                    );
                    // A HEAD response has no body, but the reference still
                    // runs its empty body through the colorizer, which
                    // emits a lone newline.
                    if body.is_empty() && current.method == "HEAD" && format.colors_active() {
                        b"\n".to_vec()
                    } else {
                        body
                    }
                }),
                meta: letters
                    .contains('m')
                    .then(|| format.meta(&format!("Elapsed time: {}", format_elapsed(elapsed)))),
            });
        }

        if is_final {
            emitter.finish();
            let mut exit = ExitStatus::Success;
            if args.check_status || args.download {
                exit = ExitStatus::from_http_status(response.status, follow);
                if exit != ExitStatus::Success {
                    let suppress = tty && args.quiet >= 2;
                    if !suppress {
                        eprint!(
                            "\nfurl: warning: HTTP {} {}\n\n",
                            response.status, response.reason
                        );
                    }
                }
            }
            // The body only downloads on a success status.
            if args.download && exit == ExitStatus::Success {
                write_download(args, &current, &download_url, &response, resume_offset)?;
            }
            if let Some(state) = &mut session_state {
                state.save(&current, items, args, Some(&jar));
            }
            return Ok(exit);
        }

        // Answer a digest challenge by replaying the same request with the
        // computed Authorization header (does not count as a redirect hop).
        if let Some(authorization) = digest_authorization {
            digest_answered = true;
            current
                .headers
                .entries
                .retain(|(name, _)| !name.eq_ignore_ascii_case("authorization"));
            current
                .headers
                .entries
                .push(("Authorization".to_string(), authorization));
            continue;
        }

        hops += 1;
        current = rebuild_for_redirect(current, &response, &jar)?;
    }
}

/// The `WWW-Authenticate: Digest …` challenge value (scheme prefix
/// stripped), if the response carries one.
fn digest_challenge_header(response: &RawResponse) -> Option<String> {
    response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
        .filter_map(|(_, value)| {
            let text = String::from_utf8_lossy(value);
            let trimmed = text.trim_start();
            trimmed
                .strip_prefix("Digest ")
                .or_else(|| trimmed.strip_prefix("digest "))
                .map(str::to_string)
        })
        .next()
}

/// A client nonce for digest auth: a hex string unique enough per request.
fn digest_cnonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::hash::RandomState::new().build_hasher();
    hasher.write_u64(0);
    format!("{:016x}", hasher.finish())
}

/// The byte offset to resume from: the size of an existing `--output`
/// file when `--continue` is set.
fn download_resume_offset(args: &ParsedArgs) -> Option<u64> {
    if !args.download_resume {
        return None;
    }
    let path = args.output.as_ref()?;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.len())
        .filter(|&n| n > 0)
}

/// Write the downloaded body to its file and report the destination on
/// stderr (unless quiet).
fn write_download(
    args: &ParsedArgs,
    request: &PreparedRequest,
    download_url: &url::Url,
    response: &RawResponse,
    resume_offset: Option<u64>,
) -> Result<(), Failure> {
    // The body is sent with Accept-Encoding: identity, so the raw bytes
    // are the file contents.
    let body = &response.body;

    let resuming = resume_offset.is_some() && response.status == 206;
    match &args.output {
        Some(path) => {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(resuming)
                .write(true)
                .truncate(!resuming)
                .open(path)
                .map_err(|error| Failure::runtime("IOError", format!("{path}: {error}")))?;
            file.write_all(body)
                .map_err(|error| Failure::runtime("IOError", format!("{path}: {error}")))?;
            if args.quiet == 0 {
                eprintln!("Done. {} written to {path}", human_size(body.len() as u64));
            }
        }
        None if !std::io::stdout().is_terminal() => {
            // Redirected stdout is itself the download target.
            std::io::stdout()
                .write_all(body)
                .map_err(|error| Failure::runtime("IOError", error.to_string()))?;
        }
        None => {
            let directory = std::env::current_dir().unwrap_or_else(|_| ".".into());
            let path = crate::download::derive_filename(response, download_url, &directory);
            std::fs::write(&path, body).map_err(|error| {
                Failure::runtime("IOError", format!("{}: {error}", path.display()))
            })?;
            if args.quiet == 0 {
                eprintln!(
                    "Done. {} written to {}",
                    human_size(body.len() as u64),
                    path.display()
                );
            }
        }
    }
    let _ = request;
    Ok(())
}

/// A short human-readable byte count for the download summary line.
fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "kB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{size:.2} {}", UNITS[unit])
    }
}

fn default_scheme(program: Program, args: &ParsedArgs) -> &str {
    match program {
        Program::Furls => "https",
        Program::Furl => args.default_scheme.as_str(),
    }
}

/// The current wall-clock time in epoch seconds, for cookie expiry.
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A loaded `--session` / `--session-read-only` and where to persist it.
struct SessionState {
    session: crate::session::Session,
    path: std::path::PathBuf,
    read_only: bool,
    existed_before: bool,
}

impl SessionState {
    /// Resolve and load the session named by the CLI, if any. A missing
    /// file loads an empty session; a corrupt file is fatal.
    fn open(
        program_name: &str,
        args: &ParsedArgs,
        items: &RequestItems,
        scheme: &str,
    ) -> Result<Option<SessionState>, Failure> {
        let (name, read_only) = match (&args.session, &args.session_read_only) {
            (Some(name), _) => (name.clone(), false),
            (None, Some(name)) => (name.clone(), true),
            (None, None) => return Ok(None),
        };

        let host_header = items
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("host"))
            .and_then(|h| h.value.clone());
        let netloc = crate::request::host_for_prompt(&args.url, scheme);
        let bound = crate::session::bound_host(host_header.as_deref(), &netloc);

        let has_separator =
            name.contains(std::path::MAIN_SEPARATOR) || (cfg!(windows) && name.contains('/'));
        let path = if has_separator {
            crate::paths::expand_tilde(&name)
        } else {
            crate::session::session_path(&name, &bound, &crate::config::config_dir())
        };

        let existed_before = path.exists();
        let session = if existed_before {
            crate::session::Session::load(&path, now_epoch())
                .map_err(|error| Failure::runtime("SessionError", error.to_string()))?
        } else {
            crate::session::Session::new()
        };

        let session_id = if has_separator {
            name.clone()
        } else {
            crate::session::port_stripped_host(&bound).to_string()
        };
        if let Some(warning) = session.legacy_warning(&session_id, &bound, !has_separator) {
            eprintln!("{program_name}: warning: {warning}");
        }

        Ok(Some(SessionState {
            session,
            path,
            read_only,
            existed_before,
        }))
    }

    /// Persist the session after the exchange, if it should be saved.
    fn save(
        &mut self,
        request: &PreparedRequest,
        items: &RequestItems,
        args: &ParsedArgs,
        jar: Option<&Jar>,
    ) {
        if !crate::session::should_save(self.read_only, self.existed_before) {
            return;
        }
        let unset: Vec<String> = items
            .headers
            .iter()
            .filter(|h| h.value.is_none())
            .map(|h| h.name.clone())
            .collect();
        // Persist the application-layer headers only: the engine's
        // Accept-Encoding/Connection/Host are not part of a session.
        self.session
            .update_headers_from_request(&request.app_headers, &unset);
        if let Some(jar) = jar {
            self.session.update_cookies_from_jar(jar, now_epoch());
        }
        if let Some(auth) = resolved_session_auth(args) {
            self.session.set_auth(auth);
        }
        if let Err(error) = self.session.save(&self.path) {
            eprintln!("furl: warning: could not save session: {error}");
        }
    }
}

/// The Authorization header a stored session auth record produces.
fn session_auth_header(auth: &crate::session::SessionAuth) -> Option<String> {
    let raw = auth
        .raw_auth
        .clone()
        .or_else(|| match (&auth.username, &auth.password) {
            (Some(u), Some(p)) => Some(format!("{u}:{p}")),
            _ => None,
        })?;
    match auth.auth_type.as_str() {
        "basic" => {
            let (user, password) = crate::request::split_credentials(&raw);
            Some(crate::request::basic_authorization(
                &user,
                &password.unwrap_or_default(),
            ))
        }
        "bearer" => Some(format!("Bearer {raw}")),
        _ => None,
    }
}

/// The auth record to store when the invocation resolved credentials.
fn resolved_session_auth(args: &ParsedArgs) -> Option<crate::session::SessionAuth> {
    let raw = args.auth.clone()?;
    Some(crate::session::SessionAuth {
        auth_type: args
            .auth_type
            .clone()
            .unwrap_or_else(|| "basic".to_string()),
        raw_auth: Some(raw),
        username: None,
        password: None,
    })
}

fn guess_method(has_input_data: bool, has_data_items: bool) -> String {
    if has_input_data || has_data_items {
        "POST".to_string()
    } else {
        "GET".to_string()
    }
}

/// The `H`/`B` request parts shown by `--offline`.
fn offline_parts(args: &ParsedArgs) -> RequestParts {
    let letters = match (&args.print, args.verbose) {
        (Some(letters), _) => letters.clone(),
        (None, 0) => "HB".to_string(),
        (None, _) => "HBhb".to_string(),
    };
    RequestParts {
        headers: letters.contains('H'),
        body: letters.contains('B'),
    }
}
