//! The `furl-manager` maintenance CLI.
//!
//! Subcommand tree: `cli` (export-args, check-updates, sessions) and
//! `plugins`. Because furl is a single static binary, `plugins` explains
//! that dynamic plugin loading is unavailable (a documented deviation).

mod export_args;
mod sessions;

#[cfg(test)]
mod tests;

use crate::status::ExitStatus;

const CONFUSION_HINT: &str = "\
This command is only for managing furl.
To send a request, please use the furl/furls commands:

  $ furl POST pie.dev/post hello=world

  $ furls POST pie.dev/post hello=world";

/// Entry point for the `furl-manager` binary.
pub fn run() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&argv) {
        Ok(output) => {
            print!("{output}");
            ExitStatus::Success.code()
        }
        Err(error) => {
            eprint!("{}", error.render());
            ExitStatus::Error.code()
        }
    }
}

/// A manager usage error: a message plus the confusion hint.
#[derive(Debug)]
struct ManagerError {
    message: String,
    hint: bool,
}

impl ManagerError {
    fn new(message: impl Into<String>) -> ManagerError {
        ManagerError {
            message: message.into(),
            hint: false,
        }
    }

    fn with_hint(message: impl Into<String>) -> ManagerError {
        ManagerError {
            message: message.into(),
            hint: true,
        }
    }

    fn render(&self) -> String {
        let mut out = format!("furl-manager: error: {}\n", self.message);
        if self.hint {
            out.push('\n');
            out.push_str(CONFUSION_HINT);
            out.push('\n');
        }
        out
    }
}

/// Route the manager argv to a handler, returning stdout text or an error.
fn dispatch(argv: &[String]) -> Result<String, ManagerError> {
    if argv.iter().any(|a| a == "--version") {
        return Ok(format!("{}\n", crate::VERSION));
    }
    match argv.first().map(String::as_str) {
        None => Err(ManagerError::with_hint(
            "Please specify one of these: 'cli', 'plugins'",
        )),
        Some("cli") => dispatch_cli(&argv[1..]),
        Some("plugins") => dispatch_plugins(&argv[1..]),
        // A request-shaped invocation gets the confusion hint.
        Some(_) => Err(ManagerError::with_hint(
            "Please specify one of these: 'cli', 'plugins'",
        )),
    }
}

fn dispatch_cli(argv: &[String]) -> Result<String, ManagerError> {
    match argv.first().map(String::as_str) {
        None => Err(ManagerError::new(
            "Please specify one of these: 'export-args', 'check-updates', 'sessions', 'plugins'",
        )),
        Some("export-args") => {
            let format = parse_format(&argv[1..])?;
            if format != "json" {
                return Err(ManagerError::new(format!(
                    "argument -f/--format: invalid choice: '{format}' (choose from 'json')"
                )));
            }
            Ok(export_args::export_json())
        }
        Some("check-updates") => {
            // Update checks are off by default and never phone home; there
            // is nothing to report.
            Ok("You are already up-to-date.\n".to_string())
        }
        Some("sessions") => dispatch_sessions(&argv[1..]),
        Some("plugins") => dispatch_plugins(&argv[1..]),
        Some(other) => Err(ManagerError::new(format!(
            "argument COMMAND: invalid choice: '{other}' \
             (choose from 'export-args', 'check-updates', 'sessions', 'plugins')"
        ))),
    }
}

fn dispatch_sessions(argv: &[String]) -> Result<String, ManagerError> {
    match argv.first().map(String::as_str) {
        None => Err(ManagerError::new(
            "Please specify one of these: 'upgrade', 'upgrade-all'",
        )),
        Some("upgrade") => sessions::upgrade(&argv[1..]),
        Some("upgrade-all") => sessions::upgrade_all(&argv[1..]),
        Some(other) => Err(ManagerError::new(format!(
            "argument COMMAND: invalid choice: '{other}' \
             (choose from 'upgrade', 'upgrade-all')"
        ))),
    }
}

/// Plugin management is unavailable in a single static binary.
fn dispatch_plugins(_argv: &[String]) -> Result<String, ManagerError> {
    Err(ManagerError::new(
        "furl is a single static binary and does not support dynamic \
         plugins. The built-in auth schemes (basic, bearer, digest) are \
         always available. See the documented deviations for details.",
    ))
}

/// Parse an optional `-f`/`--format VALUE` (default `json`).
fn parse_format(argv: &[String]) -> Result<String, ManagerError> {
    let mut i = 0;
    let mut format = "json".to_string();
    while i < argv.len() {
        match argv[i].as_str() {
            "-f" | "--format" => {
                i += 1;
                format = argv.get(i).cloned().ok_or_else(|| {
                    ManagerError::new("argument -f/--format: expected one argument")
                })?;
            }
            other if other.starts_with("--format=") => {
                format = other["--format=".len()..].to_string();
            }
            other => {
                return Err(ManagerError::new(format!(
                    "unrecognized arguments: {other}"
                )));
            }
        }
        i += 1;
    }
    Ok(format)
}
