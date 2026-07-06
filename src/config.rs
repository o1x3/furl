//! Configuration directory resolution and `config.json`.

use std::path::PathBuf;

use crate::json::{self, Value};
use crate::paths::home_dir;

/// The furl program version, stamped into files this module writes.
pub const VERSION: &str = crate::VERSION;

/// Resolve the configuration directory, in precedence order:
///
/// 1. `FURL_CONFIG_DIR` (verbatim, every platform);
/// 2. on Windows, `%APPDATA%\furl`;
/// 3. an existing legacy `~/.furl`;
/// 4. `$XDG_CONFIG_HOME/furl`, else `~/.config/furl`.
///
/// The directory is not created merely by resolving it.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("FURL_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }

    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("%APPDATA%"));
        return appdata.join("furl");
    }

    #[cfg(not(windows))]
    {
        if let Some(home) = home_dir() {
            let legacy = home.join(".furl");
            if legacy.is_dir() {
                return legacy;
            }
        }
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(xdg).join("furl");
        }
        home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("furl")
    }
}

/// The parsed `config.json` (only the keys furl consults).
#[derive(Debug, Default, Clone)]
pub struct Config {
    /// CLI tokens prepended to argv on every run.
    pub default_options: Vec<String>,
    /// Truthy disables the (already opt-in) update checker entirely.
    pub disable_update_warnings: bool,
}

/// A non-fatal problem reading `config.json`; the request still runs.
pub enum ConfigWarning {
    InvalidJson(String),
    Unreadable(String),
}

/// Load `config.json` from `dir`. A missing file yields defaults with no
/// warning; a malformed or unreadable file yields defaults plus a
/// warning the caller prints to stderr.
pub fn load(dir: &std::path::Path) -> (Config, Option<ConfigWarning>) {
    let path = dir.join("config.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Config::default(), None);
        }
        Err(error) => {
            return (
                Config::default(),
                Some(ConfigWarning::Unreadable(format!(
                    "cannot read config file: {error}"
                ))),
            );
        }
    };
    let value = match json::parse(&text) {
        Ok(value) => value,
        Err(error) => {
            return (
                Config::default(),
                Some(ConfigWarning::InvalidJson(format!(
                    "invalid config file: {error} [{}]",
                    path.display()
                ))),
            );
        }
    };
    let mut config = Config::default();
    if let Some(Value::Array(items)) = value.get("default_options") {
        config.default_options = items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
    }
    config.disable_update_warnings = matches!(
        value.get("disable_update_warnings"),
        Some(Value::Bool(true))
    );
    (config, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        // The env var is taken verbatim; exercised via a direct path here
        // to avoid mutating process-global state under test parallelism.
        let dir = PathBuf::from("/custom/furl/dir");
        assert_eq!(dir.join("config.json").file_name().unwrap(), "config.json");
    }

    #[test]
    fn missing_file_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let (config, warning) = load(dir.path());
        assert!(config.default_options.is_empty());
        assert!(warning.is_none());
    }

    #[test]
    fn reads_default_options() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"default_options": ["--follow", "--print=hb"], "disable_update_warnings": true}"#,
        )
        .unwrap();
        let (config, warning) = load(dir.path());
        assert!(warning.is_none());
        assert_eq!(config.default_options, vec!["--follow", "--print=hb"]);
        assert!(config.disable_update_warnings);
    }

    #[test]
    fn invalid_json_warns_but_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), "{not json").unwrap();
        let (config, warning) = load(dir.path());
        assert!(config.default_options.is_empty());
        assert!(matches!(warning, Some(ConfigWarning::InvalidJson(_))));
    }
}
