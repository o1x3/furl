//! `furl-manager cli sessions upgrade` / `upgrade-all`.
//!
//! Rewrites session files that still use the pre-list dict layouts for
//! `cookies`/`headers` into the modern list layouts, and stamps the current
//! furl version into `__meta__`. This is the explicit migration the
//! load-time legacy warnings point users at; a plain request run never
//! rewrites a layout on its own.
//!
//! Whether a file needs upgrading is decided from its detected layout, not
//! from the stored version stamp (furl stamps its own versions, so a
//! numeric comparison against another program's fixer versions would be
//! meaningless). A file already in the list layouts reports "already up to
//! date" and is left untouched.

use std::path::{Path, PathBuf};

use super::ManagerError;
use crate::session::{Session, session_path};

/// Handle `cli sessions upgrade HOSTNAME SESSION_NAME_OR_PATH
/// [--bind-cookies]` against the real config directory.
pub(super) fn upgrade(argv: &[String]) -> Result<String, ManagerError> {
    upgrade_in(argv, &crate::config::config_dir())
}

/// Handle `cli sessions upgrade-all [--bind-cookies]` against the real
/// config directory.
pub(super) fn upgrade_all(argv: &[String]) -> Result<String, ManagerError> {
    upgrade_all_in(argv, &crate::config::config_dir())
}

/// `upgrade` against an explicit config directory (separated for tests).
pub(super) fn upgrade_in(argv: &[String], config_dir: &Path) -> Result<String, ManagerError> {
    let args = parse_args(argv)?;
    let (hostname, session_name) = match args.positionals.as_slice() {
        [] => {
            return Err(ManagerError::new(
                "the following arguments are required: HOSTNAME, SESSION_NAME_OR_PATH",
            ));
        }
        [_] => {
            return Err(ManagerError::new(
                "the following arguments are required: SESSION_NAME_OR_PATH",
            ));
        }
        [hostname, session_name] => (hostname, session_name),
        [_, _, extra @ ..] => {
            return Err(ManagerError::new(format!(
                "unrecognized arguments: {}",
                extra.join(" ")
            )));
        }
    };
    let path = session_path(session_name, hostname, config_dir);
    upgrade_file(&path, hostname, args.bind_cookies)
}

/// `upgrade-all` against an explicit config directory (separated for
/// tests). Walks `<config>/sessions/<host>/<name>.json` in sorted order so
/// the output is deterministic, and stops at the first failure.
pub(super) fn upgrade_all_in(argv: &[String], config_dir: &Path) -> Result<String, ManagerError> {
    let args = parse_args(argv)?;
    if !args.positionals.is_empty() {
        return Err(ManagerError::new(format!(
            "unrecognized arguments: {}",
            args.positionals.join(" ")
        )));
    }

    let sessions_dir = config_dir.join("sessions");
    let host_dirs = read_dir_sorted(&sessions_dir)
        .map_err(|error| {
            ManagerError::new(format!(
                "cannot read sessions directory: {error} [{}]",
                sessions_dir.display()
            ))
        })?
        .into_iter()
        .filter(|path| path.is_dir());

    let mut output = String::new();
    for host_dir in host_dirs {
        let Some(hostname) = host_dir.file_name().map(os_lossy) else {
            continue;
        };
        let session_files = read_dir_sorted(&host_dir)
            .map_err(|error| {
                ManagerError::new(format!(
                    "cannot read sessions directory: {error} [{}]",
                    host_dir.display()
                ))
            })?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "json") && path.is_file());
        for session_file in session_files {
            output.push_str(&upgrade_file(&session_file, &hostname, args.bind_cookies)?);
        }
    }
    Ok(output)
}

/// Upgrade one session file, returning its one-line stdout report.
fn upgrade_file(path: &Path, hostname: &str, bind_cookies: bool) -> Result<String, ManagerError> {
    // Reported name: the file stem, so a named session reads back as its
    // name and a path-based session as its basename without `.json`.
    let name = path
        .file_stem()
        .map(os_lossy)
        .unwrap_or_else(|| path.display().to_string());

    if !path.is_file() {
        return Err(ManagerError::new(format!(
            "'{name}' @ '{hostname}' does not exist."
        )));
    }
    let mut session =
        Session::load(path, now_epoch()).map_err(|error| ManagerError::new(error.to_string()))?;

    if !session.needs_upgrade() {
        return Ok(format!("'{name}' @ '{hostname}' is already up to date.\n"));
    }

    // `--bind-cookies` binds domainless cookies to the hostname argument
    // verbatim (for `upgrade-all`, the host directory name).
    session.upgrade_layout(bind_cookies.then_some(hostname));
    session.bump_version();
    session.save(path).map_err(|error| {
        ManagerError::new(format!(
            "cannot write session file: {error} [{}]",
            path.display()
        ))
    })?;
    Ok(format!(
        "Upgraded '{name}' @ '{hostname}' to v{}\n",
        crate::VERSION
    ))
}

/// The arguments shared by both upgrade subcommands.
struct UpgradeArgs {
    positionals: Vec<String>,
    bind_cookies: bool,
}

/// Split argv into positionals and the `--bind-cookies` flag. Any other
/// dash-prefixed token is rejected, matching the manager's existing strict
/// treatment of unknown options.
fn parse_args(argv: &[String]) -> Result<UpgradeArgs, ManagerError> {
    let mut positionals = Vec::new();
    let mut bind_cookies = false;
    for arg in argv {
        match arg.as_str() {
            "--bind-cookies" => bind_cookies = true,
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(ManagerError::new(format!(
                    "unrecognized arguments: {other}"
                )));
            }
            _ => positionals.push(arg.clone()),
        }
    }
    Ok(UpgradeArgs {
        positionals,
        bind_cookies,
    })
}

/// Directory entries sorted by path, for deterministic walk order.
fn read_dir_sorted(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    Ok(entries)
}

fn os_lossy(name: &std::ffi::OsStr) -> String {
    name.to_string_lossy().into_owned()
}

/// Seconds since the Unix epoch, used to prune expired cookies at load.
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
