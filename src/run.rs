//! The main program flow for `furl` and `furls`.

use std::io::{IsTerminal, Read, Write};

use crate::cli::args::ParsedArgs;
use crate::cli::items::{RequestItems, process_items};
use crate::cli::parser::{Outcome, UsageError, parse};
use crate::cookies::Jar;
use crate::errors::usage_error_block;
use crate::output::message::{RequestParts, render_request, render_response_head};
use crate::request::{BuildContext, BuildError, PreparedRequest, build, split_credentials};
use crate::status::ExitStatus;
use crate::transport::{self, RawResponse, TransportError, TransportOptions, tls};
use crate::{Program, VERSION};

/// Valid `--print` letters: request headers/body, response
/// headers/body, metadata.
const PRINT_LETTERS: &str = "HBhbm";

pub fn run(program: Program) -> i32 {
    install_sigint_handler();
    let cli_argv: Vec<String> = std::env::args().skip(1).collect();
    let program_name = match program {
        Program::Furl => "furl",
        Program::Furls => "furls",
    };

    // Config `default_options` are prepended to the user's argv, so CLI
    // tokens (coming later) win for last-wins options and accumulate for
    // counts and appends. A malformed config file is a warning, not fatal.
    // It surfaces before parsing, so quiet/style are still at their
    // defaults — exactly the state the warning renders under.
    let config_dir = crate::config::config_dir();
    let (config, config_warning) = crate::config::load(&config_dir);
    if let Some(warning) = config_warning {
        let message = match warning {
            crate::config::ConfigWarning::InvalidJson(m)
            | crate::config::ConfigWarning::Unreadable(m) => m,
        };
        let reporter = Reporter {
            program: program_name.to_string(),
            quiet: 0,
            stdout_tty: std::io::stdout().is_terminal(),
            colors: stderr_colors("auto"),
        };
        reporter.warning(&message);
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

    // The interrupt newline follows the stderr-nulling of -q.
    SIGINT_QUIET.store(args.quiet > 0, std::sync::atomic::Ordering::Relaxed);
    // Suppression tracks where terminal output lands: --download routes
    // it to stderr; --output takes it off the terminal entirely.
    let stdout_tty = if args.download {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal() && args.output.is_none()
    };
    let reporter = Reporter {
        program: program_name.to_string(),
        quiet: args.quiet,
        stdout_tty,
        colors: stderr_colors(&args.style),
    };

    match execute(program, program_name, &reporter, &mut args) {
        Ok(status) => status.code(),
        Err(failure) => report_failure(&reporter, failure),
    }
}

static SIGINT_QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// An interrupt exits 130 after a bare newline on stderr (suppressed by
/// `-q`, which nulls stderr).
fn install_sigint_handler() {
    let _ = ctrlc::set_handler(|| {
        if !SIGINT_QUIET.load(std::sync::atomic::Ordering::Relaxed) {
            let stderr = std::io::stderr();
            let _ = stderr.lock().write_all(b"\n");
        }
        std::process::exit(ExitStatus::ErrorCtrlC.code());
    });
}

/// Renders warnings and errors to stderr with the standard framing,
/// coloring, and quiet-suppression rules.
struct Reporter {
    program: String,
    quiet: u32,
    /// False once `--output` redirects stdout.
    stdout_tty: bool,
    /// `(error, warning)` SGR parameters when stderr gets colors.
    colors: Option<(&'static str, &'static str)>,
}

impl Reporter {
    fn error(&self, message: &str) {
        // Errors always show.
        self.log(crate::errors::LogLevel::Error, message);
    }

    fn warning(&self, message: &str) {
        // Warnings disappear at -qq when stdout is a terminal; when piped,
        // they stay visible at any quiet level.
        if self.stdout_tty && self.quiet >= 2 {
            return;
        }
        self.log(crate::errors::LogLevel::Warning, message);
    }

    fn log(&self, level: crate::errors::LogLevel, message: &str) {
        let color = self.colors.map(|(error, warning)| match level {
            crate::errors::LogLevel::Error => error,
            crate::errors::LogLevel::Warning => warning,
        });
        let block = crate::errors::log_block(&self.program, level, message, color);
        // stderr going away must not turn into a panic.
        let stderr = std::io::stderr();
        let _ = stderr.lock().write_all(block.as_bytes());
    }
}

/// SGR parameters for stderr messages: the pie styles carry their palette
/// (on 256-color terminals); everything else colors with the standard
/// red/yellow pair. No terminal, `NO_COLOR`, or a zero-color TERM → none.
fn stderr_colors(style: &str) -> Option<(&'static str, &'static str)> {
    if !std::io::stderr().is_terminal() {
        return None;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return None;
    }
    let depth = crate::output::color::detect_color_depth();
    if !crate::output::color::colors_active(depth) {
        return None;
    }
    let full = depth == crate::output::color::ColorDepth::Ansi256;
    Some(match style {
        "pie" if full => ("38;5;167", "38;5;209"),
        "pie-dark" if full => ("38;5;203", "38;5;215"),
        "pie-light" if full => ("38;5;166", "38;5;172"),
        _ => ("31", "33"),
    })
}

/// A failure on the way to (or during) the request.
enum Failure {
    Usage(String),
    Runtime {
        /// The error-kind prefix (`ConnectionError: …`); bare messages
        /// (timeouts, redirect limits) carry none.
        kind: Option<String>,
        message: String,
        status: ExitStatus,
    },
    Annotated(String),
}

impl Failure {
    fn runtime(kind: &str, message: impl Into<String>) -> Failure {
        Failure::Runtime {
            kind: Some(kind.to_string()),
            message: message.into(),
            status: ExitStatus::Error,
        }
    }

    fn bare(message: impl Into<String>, status: ExitStatus) -> Failure {
        Failure::Runtime {
            kind: None,
            message: message.into(),
            status,
        }
    }
}

fn report_failure(reporter: &Reporter, failure: Failure) -> i32 {
    match failure {
        Failure::Usage(message) => {
            let error = UsageError {
                message,
                option: None,
            };
            exit_usage(&reporter.program, &error, 1)
        }
        Failure::Runtime {
            kind,
            message,
            status,
        } => {
            let rendered = match kind {
                Some(kind) => format!("{kind}: {message}"),
                None => message,
            };
            reporter.error(&rendered);
            status.code()
        }
        Failure::Annotated(rendered) => {
            eprintln!("{rendered}");
            ExitStatus::Error.code()
        }
    }
}

/// Read the piped-stdin body. Online, a watcher thread nudges toward
/// --ignore-stdin on stderr when no data (not even EOF) shows up within
/// the warn threshold; the read itself never stops waiting.
/// `FURL_STDIN_READ_WARN_THRESHOLD` overrides the 10s default; `0`
/// disables. The nudge is plain stderr text (nulled by -q), not a framed
/// warning.
fn read_stdin_body(offline: bool, quiet: bool) -> std::io::Result<Vec<u8>> {
    let threshold = std::env::var("FURL_STDIN_READ_WARN_THRESHOLD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .unwrap_or(10.0);

    let watch = !offline && threshold > 0.0 && threshold.is_finite();
    let seen_data = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if watch {
        let seen_data = std::sync::Arc::clone(&seen_data);
        let wait = std::time::Duration::from_secs_f64(threshold);
        std::thread::spawn(move || {
            std::thread::sleep(wait);
            if seen_data.load(std::sync::atomic::Ordering::Relaxed) || quiet {
                return;
            }
            let seconds = crate::json::dumps(
                &crate::json::Value::from(threshold),
                &crate::json::DumpOptions::default(),
            );
            let text = format!(
                "> warning: no stdin data read in {seconds}s \
                 (perhaps you want to --ignore-stdin)\n\
                 > See: {}\n",
                crate::errors::DOCS_URL
            );
            let stderr = std::io::stderr();
            let _ = stderr.lock().write_all(text.as_bytes());
        });
    }

    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let read = handle.read(&mut chunk)?;
        // The first read result — even an immediate EOF — counts as the
        // pipe answering.
        seen_data.store(true, std::sync::atomic::Ordering::Relaxed);
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
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
    reporter: &Reporter,
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

    // Reject header names/values that carry reserved or return characters
    // before they can reach the wire (header injection). Values are
    // validated after the same surrounding-whitespace strip the wire uses.
    validate_headers(&items)?;

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
        let bytes = read_stdin_body(args.offline, args.quiet > 0)
            .map_err(|error| Failure::runtime("IOError", error.to_string()))?;
        Some(bytes)
    } else {
        None
    };

    // -- Session load -----------------------------------------------------------
    let scheme = default_scheme(program, args);
    let mut session_state = SessionState::open(reporter, args, &items, scheme)?;
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
        // The text pipeline runs on a terminal or under any prettifying;
        // raw piped output stays byte-exact.
        encoded: format_tty || mode != crate::output::format::PrettyMode::None,
        terminal: format_tty,
        stream: args.stream,
        response_charset: args.response_charset.clone(),
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
                match format.body(body.bytes.clone(), content_type.as_deref(), false) {
                    RenderedBody::Bytes(bytes) => rendered.extend_from_slice(&bytes),
                    RenderedBody::SuppressedBinary => {
                        if parts.headers {
                            rendered.push(b'\n');
                        }
                        rendered.extend_from_slice(BINARY_NOTICE);
                    }
                }
            }
        }
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let mut sink_error = handle.write_all(&rendered).err();
        if sink_error.is_none() && handle.is_terminal() && !rendered.is_empty() {
            sink_error = handle.write_all(b"\n\n").err();
        }
        if sink_error.is_none() {
            sink_error = handle.flush().err();
        }
        drop(handle);
        // Offline runs still persist request headers and auth (no cookies).
        if let Some(state) = &mut session_state {
            state.save(&request, &items, args, None)?;
        }
        if let Some(error) = sink_error {
            if args.traceback && error.kind() == std::io::ErrorKind::BrokenPipe {
                // A broken pipe under --traceback degrades to a bare
                // stderr newline and a clean exit.
                if args.quiet == 0 {
                    let stderr = std::io::stderr();
                    let _ = stderr.lock().write_all(b"\n");
                }
            } else {
                let (errno, text) = crate::errors::os_error_parts(&error);
                return Err(Failure::runtime(
                    crate::errors::os_error_class(error.kind()),
                    format!("[Errno {errno}] {text}"),
                ));
            }
        }
        return Ok(ExitStatus::Success);
    }

    execute_online(args, reporter, request, format, &items, session_state)
}

// A leading newline is part of the notice itself; the head section adds
// one more when it printed.
const BINARY_NOTICE: &[u8] = b"\n+-----------------------------------------+\n\
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
    body: Option<RenderedBody>,
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
    /// The first sink error's (errno, kind); once set, writes stop and
    /// the online loop turns it into the exit.
    failed: Option<(i32, std::io::ErrorKind)>,
    /// --traceback swallows broken pipes (a bare stderr newline per
    /// message instead of an error exit).
    traceback: bool,
    /// -q nulls stderr, taking the swallowed-pipe newline with it.
    quiet: bool,
}

impl Emitter {
    fn write(&mut self, bytes: &[u8]) {
        if self.failed.is_some() {
            return;
        }
        let result = match &mut self.sink {
            Sink::Stdout => {
                let stdout = std::io::stdout();
                let mut handle = stdout.lock();
                handle.write_all(bytes)
            }
            Sink::Stderr => {
                let stderr = std::io::stderr();
                let mut handle = stderr.lock();
                handle.write_all(bytes)
            }
            Sink::File(file) => file.write_all(bytes),
            Sink::Null => Ok(()),
        };
        if let Err(error) = result {
            let (errno, _) = crate::errors::os_error_parts(&error);
            self.failed = Some((errno, error.kind()));
        }
    }

    /// Under --traceback a broken pipe degrades to one bare stderr newline
    /// per message that still tries to print.
    fn swallowed_pipe_newline(&self) {
        let broken_pipe = matches!(self.failed, Some((_, std::io::ErrorKind::BrokenPipe)));
        if broken_pipe && self.traceback && !self.quiet {
            let stderr = std::io::stderr();
            let _ = stderr.lock().write_all(b"\n");
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
        match &message.body {
            Some(RenderedBody::SuppressedBinary) => {
                if message.head.is_some() {
                    self.write(b"\n");
                }
                self.write(BINARY_NOTICE);
                printed_bytes = true;
            }
            Some(RenderedBody::Bytes(bytes)) if !bytes.is_empty() => {
                self.write(bytes);
                printed_bytes = true;
            }
            _ => {}
        }

        if let Some(meta) = &message.meta {
            // The meta text already carries its "Elapsed time: …" label
            // (and any coloring); the writer only supplies the separators.
            // The leading separator belongs to the body *section*: it
            // appears only when that section was selected.
            if message.body.is_some() {
                self.write(b"\n\n");
            }
            self.write(meta.as_bytes());
            self.write(b"\n\n");
        } else if self.tty && printed_bytes {
            // On a terminal, a body-printing message ends with a blank
            // line (unless meta already supplied its trailing separator).
            self.write(b"\n\n");
        }

        // The body section being *selected* carries the separator, even
        // when the body was empty.
        self.previous_had_body = message.body.is_some();
        self.started = true;
        self.swallowed_pipe_newline();
    }

    fn finish(&mut self) {
        let result = match &mut self.sink {
            Sink::Stdout => std::io::stdout().flush(),
            Sink::Stderr => std::io::stderr().flush(),
            Sink::File(file) => file.flush(),
            Sink::Null => Ok(()),
        };
        if let Err(error) = result {
            if self.failed.is_none() {
                let (errno, _) = crate::errors::os_error_parts(&error);
                self.failed = Some((errno, error.kind()));
                self.swallowed_pipe_newline();
            }
        }
    }

    /// The failure this sink ends the run with, unless --traceback
    /// swallowed a broken pipe.
    fn failure(&self) -> Option<Failure> {
        let (errno, kind) = self.failed?;
        if self.traceback && kind == std::io::ErrorKind::BrokenPipe {
            return None;
        }
        Some(Failure::runtime(
            crate::errors::os_error_class(kind),
            format!("[Errno {errno}] {}", crate::errors::os_error_text(errno)),
        ))
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
    let client_key = args.cert_key.as_deref().map(crate::paths::expand_tilde);
    // An encrypted client key prompts for its passphrase here, once,
    // before any connection; `--ignore-stdin` suppresses the prompt and
    // the missing passphrase surfaces as an SSLError at connect time.
    let cert_key_pass = tls::resolve_key_passphrase(
        client_key.as_deref(),
        args.cert_key_pass.clone(),
        !args.ignore_stdin,
    );
    tls::TlsOptions {
        verify,
        version,
        client_cert: args.cert.as_deref().map(crate::paths::expand_tilde),
        client_key,
        ciphers: args.ciphers.clone(),
        cert_key_pass,
    }
}

/// Reject any header item whose name or (stripped) value contains
/// leading whitespace, a reserved character, or a return character —
/// mirroring the reference's validation and its `InvalidHeader` message
/// with the offending part shown Python-style (name as `str`, value as
/// `bytes`).
fn validate_headers(items: &RequestItems) -> Result<(), Failure> {
    const INVALID: &str =
        "Invalid leading whitespace, reserved character(s), or return character(s) in header";
    for header in &items.headers {
        // Name: first character neither `:` nor whitespace, and no `:`,
        // CR, or LF anywhere.
        let name = &header.name;
        let name_ok = name
            .chars()
            .next()
            .is_some_and(|c| c != ':' && !c.is_whitespace())
            && !name.contains([':', '\r', '\n']);
        if !name_ok {
            return Err(Failure::runtime(
                "InvalidHeader",
                format!("{INVALID} name: {}", crate::errors::py_str_repr(name)),
            ));
        }
        // Value: validated after the same whitespace strip the wire
        // applies; an empty value passes, otherwise no CR/LF may remain.
        if let Some(value) = &header.value {
            let stripped = value.trim_matches(|c: char| c.is_ascii_whitespace());
            if !stripped.is_empty() && stripped.contains(['\r', '\n']) {
                return Err(Failure::runtime(
                    "InvalidHeader",
                    format!(
                        "{INVALID} value: {}",
                        crate::errors::py_bytes_repr(stripped.as_bytes())
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// An unusable proxy URL, before any connection is attempted.
fn proxy_failure(error: crate::proxy::ProxyError) -> Failure {
    match error {
        crate::proxy::ProxyError::Socks => {
            Failure::runtime("InvalidSchema", "SOCKS proxies are not supported")
        }
        crate::proxy::ProxyError::UnsupportedScheme(scheme) => Failure::runtime(
            "ProxySchemeUnknown",
            format!("Proxy URL had unsupported scheme {scheme}, should use http:// or https://"),
        ),
        crate::proxy::ProxyError::Invalid(_) => Failure::runtime(
            "InvalidProxyURL",
            "Please check proxy URL. It is malformed and could be missing the host.",
        ),
    }
}

fn transport_failure(
    error: TransportError,
    timeout: f64,
    request: &PreparedRequest,
    proxy: Option<&crate::proxy::ProxyRoute>,
) -> Failure {
    // Connection-level failures carry the request they interrupted.
    let suffix = format!(
        " while doing a {} request to URL: {}",
        request.method,
        request.url.as_str()
    );
    // Pool naming: effective port, bare IPv6 host (no brackets).
    let host = request.url.host_str().unwrap_or_default();
    let host = host
        .strip_prefix('[')
        .map_or(host, |rest| rest.strip_suffix(']').unwrap_or(rest));
    let port = request.url.port_or_known_default().unwrap_or(0);
    let https = request.url.scheme() == "https";
    let (pool, connection) = if https {
        ("HTTPSConnectionPool", "HTTPSConnection")
    } else {
        ("HTTPConnectionPool", "HTTPConnection")
    };

    match error {
        TransportError::Timeout => Failure::bare(
            format!(
                "Request timed out ({}s).",
                crate::json::dumps(
                    &crate::json::Value::from(timeout),
                    &crate::json::DumpOptions::default()
                )
            ),
            ExitStatus::ErrorTimeout,
        ),
        TransportError::Dns { code, text } => {
            let annotation = if code == transport::EAI_NONAME {
                "\nCouldn't resolve the given hostname. Please check the URL and try again."
            } else if code == transport::EAI_AGAIN {
                "\nCouldn't connect to a DNS server. Please check your connection and try again."
            } else {
                ""
            };
            Failure::runtime("gaierror", format!("[Errno {code}] {text}{annotation}"))
        }
        TransportError::ConnectFailed { errno, text } => {
            let inner = format!(
                "{connection}(host='{host}', port={port}): \
                 Failed to establish a new connection: [Errno {errno}] {text}"
            );
            Failure::runtime(
                "ConnectionError",
                format!(
                    "{pool}(host='{host}', port={port}): Max retries exceeded with url: {target} \
                     (Caused by NewConnectionError(\"{inner}\")){suffix}",
                    target = request.request_target()
                ),
            )
        }
        TransportError::ClosedWithoutResponse => Failure::runtime(
            "ConnectionError",
            format!(
                "('Connection aborted.', \
                 RemoteDisconnected('Remote end closed connection without response')){suffix}"
            ),
        ),
        TransportError::Aborted { errno, kind, text } => Failure::runtime(
            "ConnectionError",
            format!(
                "('Connection aborted.', {class}({errno}, '{text}')){suffix}",
                class = crate::errors::os_error_class(kind)
            ),
        ),
        TransportError::Connection(message) => {
            Failure::runtime("ConnectionError", format!("{message}{suffix}"))
        }
        TransportError::Tls(message) => Failure::runtime("SSLError", format!("{message}{suffix}")),
        TransportError::Protocol(message) => {
            Failure::runtime("ConnectionError", format!("{message}{suffix}"))
        }
        TransportError::TooManyHeaders(count) => Failure::runtime(
            "ConnectionError",
            format!("got more than {count} headers{suffix}"),
        ),
        TransportError::Proxy(inner) => match *inner {
            // Resolver and timeout failures on the proxy hop read exactly
            // like the same failures on a direct connection.
            TransportError::Dns { .. } | TransportError::Timeout => {
                transport_failure(*inner, timeout, request, None)
            }
            other => proxied_failure(other, request, proxy, &suffix),
        },
        TransportError::TunnelFailed { status, reason } => {
            let detail = format!("OSError('Tunnel connection failed: {status} {reason}')");
            Failure::runtime(
                "ProxyError",
                format!(
                    "{}: Max retries exceeded with url: {} \
                     (Caused by ProxyError('Unable to connect to proxy', {detail})){suffix}",
                    proxied_pool(request, proxy),
                    proxied_url(request),
                ),
            )
        }
    }
}

/// The pool naming for a proxied failure: https targets name the target
/// pool; plain-http targets name the proxy's pool.
fn proxied_pool(request: &PreparedRequest, proxy: Option<&crate::proxy::ProxyRoute>) -> String {
    let bare = |value: &str| -> String {
        value
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    };
    if request.url.scheme() == "https" {
        let host = bare(request.url.host_str().unwrap_or_default());
        let port = request.url.port_or_known_default().unwrap_or(0);
        format!("HTTPSConnectionPool(host='{host}', port={port})")
    } else {
        let (host, port) = proxy
            .map(|route| (bare(&route.host), route.port))
            .unwrap_or_default();
        format!("HTTPConnectionPool(host='{host}', port={port})")
    }
}

/// The `Max retries exceeded with url:` value for a proxied failure:
/// origin-form for https targets, absolute for plain-http ones.
fn proxied_url(request: &PreparedRequest) -> String {
    if request.url.scheme() == "https" {
        request.request_target()
    } else {
        format!(
            "{}://{}{}",
            request.url.scheme(),
            request.host_netloc,
            request.request_target()
        )
    }
}

/// A failure to reach (or be admitted by) the proxy itself.
fn proxied_failure(
    inner: TransportError,
    request: &PreparedRequest,
    proxy: Option<&crate::proxy::ProxyRoute>,
    suffix: &str,
) -> Failure {
    let bare = |value: &str| -> String {
        value
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string()
    };
    let (proxy_host, proxy_port) = proxy
        .map(|route| (bare(&route.host), route.port))
        .unwrap_or_default();
    let connection_class = if request.url.scheme() == "https" {
        "HTTPSConnection"
    } else {
        "HTTPConnection"
    };
    let detail = match inner {
        TransportError::ConnectFailed { errno, text } => format!(
            "NewConnectionError(\"{connection_class}(host='{proxy_host}', port={proxy_port}): \
             Failed to establish a new connection: [Errno {errno}] {text}\")"
        ),
        TransportError::Tls(message) => format!("SSLError({message:?})"),
        other => format!("{other:?}"),
    };
    Failure::runtime(
        "ProxyError",
        format!(
            "{}: Max retries exceeded with url: {} \
             (Caused by ProxyError('Unable to connect to proxy', {detail})){suffix}",
            proxied_pool(request, proxy),
            proxied_url(request),
        ),
    )
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

    // Method rewriting (RFC 7231 §6.4.4 plus long-standing browser
    // behavior): 303 and 302 turn any non-HEAD method into GET; a 301
    // rewrites only POST.
    let becomes_get = match response.status {
        303 | 302 => request.method != "HEAD",
        301 => request.method == "POST",
        _ => false,
    };
    if becomes_get {
        request.method = "GET".to_string();
    }
    // Every redirect except 307/308 drops the request body and its
    // framing headers, regardless of whether the method changed.
    if !matches!(response.status, 307 | 308) {
        request.body = None;
        request.chunked = false;
        request.headers.entries.retain(|(name, _)| {
            !(name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("content-type")
                || name.eq_ignore_ascii_case("transfer-encoding"))
        });
        // A now-bodyless body-method still carries `Content-Length: 0`
        // (GET/HEAD/OPTIONS do not), matching a freshly built request.
        if !matches!(request.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
            request
                .headers
                .entries
                .push(("Content-Length".to_string(), "0".to_string()));
        }
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
    if let Some(value) =
        jar.header_for(request.url.scheme(), &host, request.url.path(), now_epoch())
    {
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
    /// `--response-charset`: forces the response-body text encoding.
    response_charset: Option<String>,
    /// The resolved color style (from `--style` + terminal depth). `None`
    /// when the colors group is inactive or the terminal has no color.
    style: Option<crate::output::color::Style>,
    /// The text pipeline runs (terminal output or any prettifying);
    /// otherwise bodies pass through raw.
    encoded: bool,
    /// Output lands on a terminal: re-encode decoded text as UTF-8
    /// instead of the body's own declared encoding.
    terminal: bool,
    /// `--stream`: prettified bodies process line by line.
    stream: bool,
}

/// A body section after formatting: printable bytes, or the binary
/// notice.
enum RenderedBody {
    Bytes(Vec<u8>),
    SuppressedBinary,
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
    fn body(&self, bytes: Vec<u8>, content_type: Option<&str>, is_response: bool) -> RenderedBody {
        // Raw path: piped with no prettifying — bytes pass untouched.
        if !self.encoded {
            return RenderedBody::Bytes(bytes);
        }
        // The text pipeline suppresses binary bodies wholesale.
        if bytes.contains(&0) {
            return RenderedBody::SuppressedBinary;
        }
        let declared = crate::output::format::charset_from_content_type(content_type);
        let source = if is_response {
            self.response_charset.as_deref().or(declared.as_deref())
        } else {
            declared.as_deref()
        };
        let text = crate::encoding::decode_body(&bytes, source);
        let mime = crate::output::format::effective_mime(
            content_type,
            if is_response {
                self.response_mime.as_deref()
            } else {
                None
            },
        );
        let processed = if self.stream && (self.format_active() || self.colors_active()) {
            // --stream: each line runs the pipeline on its own.
            let mut out = String::new();
            for segment in text.split_inclusive('\n') {
                let (line, newline) = match segment.strip_suffix('\n') {
                    Some(line) => (line, "\n"),
                    None => (segment, ""),
                };
                out.push_str(&self.process_text(line, mime.as_deref()));
                out.push_str(newline);
            }
            out
        } else {
            self.process_text(&text, mime.as_deref())
        };
        RenderedBody::Bytes(crate::encoding::encode_body(
            &processed,
            declared.as_deref(),
            self.terminal,
        ))
    }

    /// The format group then the colors group, over decoded text.
    fn process_text(&self, text: &str, mime: Option<&str>) -> String {
        let formatted = if self.format_active() {
            crate::output::format::format_body(
                text,
                mime,
                self.explicit_json,
                &self.options,
                self.mode,
            )
        } else {
            text.to_string()
        };
        match &self.style {
            Some(style) if self.colors_active() => {
                crate::output::color::colorize_body(&formatted, mime, style)
            }
            _ => formatted,
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
    reporter: &Reporter,
    request: PreparedRequest,
    format: FormatContext,
    items: &RequestItems,
    mut session_state: Option<SessionState>,
) -> Result<ExitStatus, Failure> {
    let mut options = TransportOptions {
        timeout: (args.timeout > 0.0).then(|| std::time::Duration::from_secs_f64(args.timeout)),
        tls: resolve_tls(args),
        max_headers: usize::try_from(args.max_headers.max(0)).unwrap_or(0),
        proxy: None,
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
        failed: None,
        traceback: args.traceback,
        quiet: args.quiet > 0,
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
        now_epoch(),
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
        // Only http/https reach the wire; any other scheme (ftp, file,
        // mailto, data, …) — as typed or arrived at via a redirect — is
        // rejected the way the reference rejects a missing adapter, before
        // any connection is attempted.
        if !matches!(current.url.scheme(), "http" | "https") {
            return Err(Failure::runtime(
                "InvalidSchema",
                format!(
                    "No connection adapters were found for '{}'",
                    current.url.as_str()
                ),
            ));
        }
        // A hostless http(s) URL (e.g. a `http:///path` redirect target)
        // would panic the transport's host expectation; reject it first.
        if current.url.host_str().is_none() {
            return Err(Failure::runtime(
                "InvalidURL",
                format!("Invalid URL '{}': No host supplied", current.url.as_str()),
            ));
        }
        // The proxy route follows the current hop's URL (a redirect may
        // change the scheme or leave a no_proxy host).
        options.proxy =
            crate::proxy::route_for(&current.url, &args.proxy).map_err(proxy_failure)?;
        let started_at = std::time::Instant::now();
        let response = transport::send(&current, &options)
            .map_err(|e| transport_failure(e, args.timeout, &current, options.proxy.as_ref()))?;
        let elapsed = started_at.elapsed();

        let host = current.url.host_str().unwrap_or_default().to_string();
        let request_path = current.url.path().to_string();
        for (name, value) in &response.headers {
            if name.eq_ignore_ascii_case("set-cookie") {
                jar.store(
                    &host,
                    &request_path,
                    &String::from_utf8_lossy(value),
                    now_epoch(),
                );
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
            return Err(Failure::bare(
                format!(
                    "Too many redirects (--max-redirects={}).",
                    args.max_redirects
                ),
                ExitStatus::ErrorTooManyRedirects,
            ));
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
                format.body(bytes, request_content_type.as_deref(), false)
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
                        true,
                    );
                    // A HEAD response has no body, but the reference still
                    // runs its empty body through the colorizer, which
                    // emits a lone newline.
                    match body {
                        RenderedBody::Bytes(bytes)
                            if bytes.is_empty()
                                && current.method == "HEAD"
                                && format.colors_active() =>
                        {
                            RenderedBody::Bytes(b"\n".to_vec())
                        }
                        other => other,
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
                // On a terminal the colored status line already shows the
                // failure; the warning only appears piped — or at exactly
                // -q, where that line was silenced.
                if exit != ExitStatus::Success && (!reporter.stdout_tty || args.quiet == 1) {
                    reporter.warning(&format!("HTTP {} {}", response.status, response.reason));
                }
            }
            // A dead sink ends the run (cookies still persist) — unless
            // --traceback swallowed a broken pipe.
            if let Some(failure) = emitter.failure() {
                if let Some(state) = &mut session_state {
                    state.save(&current, items, args, Some(&jar))?;
                }
                return Err(failure);
            }
            // The body only downloads on a success status.
            if args.download && exit == ExitStatus::Success {
                write_download(args, &current, &download_url, &response, resume_offset)?;
            }
            if let Some(state) = &mut session_state {
                state.save(&current, items, args, Some(&jar))?;
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
        reporter: &Reporter,
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
            reporter.warning(&warning);
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
    ) -> Result<(), Failure> {
        if !crate::session::should_save(self.read_only, self.existed_before) {
            return Ok(());
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
            // A session that cannot persist ends the run the way any
            // other file error does, with the failing path named.
            let (errno, text) = crate::errors::os_error_parts(&error);
            return Err(Failure::runtime(
                crate::errors::os_error_class(error.kind()),
                format!("[Errno {errno}] {text}: '{}'", self.path.display()),
            ));
        }
        Ok(())
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
