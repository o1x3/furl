//! The main program flow for `furl` and `furls`.

use std::io::{IsTerminal, Read, Write};

use crate::cli::args::ParsedArgs;
use crate::cli::items::process_items;
use crate::cli::parser::{Outcome, UsageError, parse};
use crate::errors::{runtime_error_line, usage_error_block};
use crate::output::message::{RequestParts, render_request};
use crate::request::{BuildContext, BuildError, build, split_credentials};
use crate::status::ExitStatus;
use crate::{Program, VERSION};

/// Valid `--print` letters: request headers/body, response
/// headers/body, metadata.
const PRINT_LETTERS: &str = "HBhbm";

pub fn run(program: Program) -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let program_name = match program {
        Program::Furl => "furl",
        Program::Furls => "furls",
    };
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
    Runtime { kind: String, message: String },
    Annotated(String),
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
        Failure::Runtime { kind, message } => {
            eprint!("{}", runtime_error_line(program_name, &kind, &message));
            ExitStatus::Error.code()
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
            ParsedArgs::validate_item_token(&item)
                .map_err(|detail| Failure::Usage(format!("argument REQUEST_ITEM: {detail}")))?;
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
    let items = process_items(&args.request_items, args.request_type)
        .map_err(|error| Failure::Usage(error.message))?;

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
    let stdin_body = if stdin_available && args.raw.is_none() {
        let mut bytes = Vec::new();
        std::io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| Failure::Runtime {
                kind: "IOError".to_string(),
                message: error.to_string(),
            })?;
        if bytes.is_empty() { None } else { Some(bytes) }
    } else {
        None
    };

    // -- Build ------------------------------------------------------------------
    let scheme = default_scheme(program, args);
    let request = build(&BuildContext {
        args,
        items: &items,
        stdin_body,
        default_scheme: scheme,
        version: VERSION,
    })
    .map_err(|error| match error {
        BuildError::Usage(message) => Failure::Usage(message),
        BuildError::InvalidUrl { url, reason } => Failure::Runtime {
            kind: "InvalidURL".to_string(),
            message: format!("Invalid URL '{url}': {reason}"),
        },
        BuildError::NestedJson(error) => Failure::Annotated(error.to_string()),
        BuildError::Body(message) => Failure::Runtime {
            kind: "IOError".to_string(),
            message,
        },
        BuildError::PasswordRequired { user } => Failure::Usage(format!(
            "Unable to prompt for passwords because --ignore-stdin is set. \
             (username: {user})"
        )),
    })?;

    // -- Offline execution ---------------------------------------------------------
    if args.offline {
        let parts = offline_parts(args);
        let rendered = render_request(&request, parts);
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(&rendered).ok();
        if handle.is_terminal() && !rendered.is_empty() {
            handle.write_all(b"\n\n").ok();
        }
        handle.flush().ok();
        return Ok(ExitStatus::Success);
    }

    Err(Failure::Runtime {
        kind: "NotImplemented".to_string(),
        message: "sending requests over the network is not wired up yet; \
                  use --offline"
            .to_string(),
    })
}

fn default_scheme(program: Program, args: &ParsedArgs) -> &str {
    match program {
        Program::Furls => "https",
        Program::Furl => args.default_scheme.as_str(),
    }
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
