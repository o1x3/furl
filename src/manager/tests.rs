use super::{dispatch, sessions};
use std::path::{Path, PathBuf};

#[track_caller]
fn ok(argv: &[&str]) -> String {
    dispatch(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>()).expect("expected success")
}

#[track_caller]
fn err(argv: &[&str]) -> String {
    match dispatch(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>()) {
        Ok(output) => panic!("expected an error, got: {output}"),
        Err(error) => error.render(),
    }
}

#[test]
fn naked_invocation_shows_confusion_hint() {
    let message = err(&[]);
    assert!(message.contains("Please specify one of these: 'cli', 'plugins'"));
    assert!(message.contains("$ furl POST pie.dev/post hello=world"));
    assert!(message.contains("$ furls POST pie.dev/post hello=world"));
}

#[test]
fn request_shaped_invocation_shows_hint() {
    let message = err(&["POST", "pie.dev/post", "hello=world"]);
    assert!(message.contains("furl/furls commands"));
}

#[test]
fn version_flag() {
    assert_eq!(ok(&["--version"]), format!("{}\n", crate::VERSION));
}

#[test]
fn export_args_is_valid_schema() {
    let output = ok(&["cli", "export-args"]);
    let value = crate::json::parse(output.trim()).expect("export-args emits valid JSON");
    assert_eq!(
        value.get("version").and_then(|v| v.as_str()),
        Some("0.0.1a0")
    );
    let spec = value.get("spec").expect("spec present");
    assert_eq!(spec.get("name").and_then(|v| v.as_str()), Some("furl"));
    let groups = spec
        .get("groups")
        .and_then(|v| v.as_array())
        .expect("groups");
    assert_eq!(
        groups[0].get("name").and_then(|v| v.as_str()),
        Some("Positional arguments")
    );
    // The first positional is METHOD with a metavar and optional flag.
    let first_arg = groups[0]
        .get("args")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .expect("first positional");
    assert_eq!(
        first_arg.get("metavar").and_then(|v| v.as_str()),
        Some("METHOD")
    );
    assert_eq!(
        first_arg.get("is_positional"),
        Some(&crate::json::Value::Bool(true))
    );
    // An option group carries flag aliases and choices where applicable.
    let has_auth_type = groups.iter().any(|g| {
        g.get("args")
            .and_then(|v| v.as_array())
            .is_some_and(|args| {
                args.iter().any(|a| {
                    a.get("options")
                        .and_then(|v| v.as_array())
                        .is_some_and(|opts| opts.iter().any(|o| o.as_str() == Some("--auth-type")))
                        && a.get("choices").is_some()
                })
            })
    });
    assert!(has_auth_type, "--auth-type should export its choices");
}

#[test]
fn export_args_default_and_explicit_format() {
    let default = ok(&["cli", "export-args"]);
    let explicit = ok(&["cli", "export-args", "--format", "json"]);
    assert_eq!(default, explicit);
}

#[test]
fn export_args_rejects_unknown_format() {
    let message = err(&["cli", "export-args", "--format", "xml"]);
    assert!(message.contains("invalid choice: 'xml'"), "{message}");
}

#[test]
fn check_updates_reports_up_to_date() {
    assert_eq!(
        ok(&["cli", "check-updates"]),
        "You are already up-to-date.\n"
    );
}

#[test]
fn plugins_explains_the_deviation() {
    let message = err(&["plugins", "list"]);
    assert!(message.contains("single static binary"));
    assert!(message.contains("basic, bearer, digest"));
    let via_cli = err(&["cli", "plugins", "install", "x"]);
    assert!(via_cli.contains("single static binary"));
}

#[test]
fn cli_without_subcommand() {
    let message = err(&["cli"]);
    assert!(message.contains("'export-args'"));
    // The internal help key is not leaked (a fixed deviation).
    assert!(!message.contains("'help'"));
}

#[test]
fn sessions_subcommand_tree() {
    assert!(
        err(&["cli", "sessions"]).contains("Please specify one of these: 'upgrade', 'upgrade-all'")
    );
    assert!(err(&["cli", "sessions", "bogus"]).contains("invalid choice: 'bogus'"));
}

// ---- sessions upgrade: argument parsing (no filesystem access) -----------

#[test]
fn sessions_upgrade_requires_both_positionals() {
    let message = err(&["cli", "sessions", "upgrade"]);
    assert!(
        message.contains("the following arguments are required: HOSTNAME, SESSION_NAME_OR_PATH"),
        "{message}"
    );
    let message = err(&["cli", "sessions", "upgrade", "example.org"]);
    assert!(
        message.contains("the following arguments are required: SESSION_NAME_OR_PATH"),
        "{message}"
    );
}

#[test]
fn sessions_upgrade_rejects_extra_and_unknown_arguments() {
    let message = err(&["cli", "sessions", "upgrade", "example.org", "api", "extra"]);
    assert!(
        message.contains("unrecognized arguments: extra"),
        "{message}"
    );
    let message = err(&[
        "cli",
        "sessions",
        "upgrade",
        "--frobnicate",
        "example.org",
        "api",
    ]);
    assert!(
        message.contains("unrecognized arguments: --frobnicate"),
        "{message}"
    );
    let message = err(&["cli", "sessions", "upgrade-all", "extra"]);
    assert!(
        message.contains("unrecognized arguments: extra"),
        "{message}"
    );
}

// ---- sessions upgrade: behavior against a temp config dir ----------------

/// A dict-cookie-layout session: `baz` is domainless, `bound` is bound.
const LEGACY_COOKIES: &str = concat!(
    "{\"__meta__\": {\"furl\": \"0.0.1\"},\n",
    " \"cookies\": {\n",
    "   \"baz\": {\"value\": \"quux\", \"path\": \"/\", ",
    "\"expires\": null, \"secure\": false},\n",
    "   \"bound\": {\"value\": \"b\", \"domain\": \"example.org\", ",
    "\"path\": \"/\", \"expires\": null, \"secure\": false}\n",
    " }}\n",
);

/// A dict-header-layout session.
const LEGACY_HEADERS: &str =
    "{\"__meta__\": {\"furl\": \"0.0.1\"}, \"headers\": {\"X-Data\": \"value\"}}\n";

/// A session already in the modern list layouts.
const MODERN: &str = "{\"__meta__\": {\"furl\": \"0.0.1\"}, \"cookies\": [], \"headers\": []}\n";

fn write_session(config: &Path, host: &str, name: &str, text: &str) -> PathBuf {
    let dir = config.join("sessions").join(host);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, text).unwrap();
    path
}

fn args(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|a| a.to_string()).collect()
}

#[test]
fn upgrade_nonexistent_session_errors() {
    let dir = tempfile::tempdir().unwrap();
    let error = sessions::upgrade_in(&args(&["example.org", "nope"]), dir.path()).unwrap_err();
    assert_eq!(
        error.render(),
        "furl-manager: error: 'nope' @ 'example.org' does not exist.\n"
    );
}

#[test]
fn upgrade_modern_session_is_already_up_to_date() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "api", MODERN);
    let before = std::fs::read_to_string(&path).unwrap();
    let output = sessions::upgrade_in(&args(&["example.org", "api"]), dir.path()).unwrap();
    assert_eq!(output, "'api' @ 'example.org' is already up to date.\n");
    // The file is left byte-for-byte untouched (no version bump either).
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn upgrade_cookie_dict_without_bind_leaves_domainless_null() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "api", LEGACY_COOKIES);
    let output = sessions::upgrade_in(&args(&["example.org", "api"]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!("Upgraded 'api' @ 'example.org' to v{}\n", crate::VERSION)
    );
    let text = std::fs::read_to_string(&path).unwrap();
    // List layout now, with the dict key materialized as `name`.
    assert!(text.contains("\"cookies\": ["), "{text}");
    assert!(text.contains("\"name\": \"baz\""), "{text}");
    // Domainless cookie becomes an explicit-none (null) domain...
    assert!(text.contains("\"domain\": null"), "{text}");
    // ...while an already-bound cookie keeps its domain.
    assert!(text.contains("\"domain\": \"example.org\""), "{text}");
    // The version stamp is bumped to the current program version.
    assert!(
        text.contains(&format!("\"furl\": \"{}\"", crate::VERSION)),
        "{text}"
    );
    assert!(!text.contains("0.0.1"), "{text}");
}

#[test]
fn upgrade_cookie_dict_with_bind_binds_domainless_to_hostname() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "api", LEGACY_COOKIES);
    let output =
        sessions::upgrade_in(&args(&["--bind-cookies", "example.org", "api"]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!("Upgraded 'api' @ 'example.org' to v{}\n", crate::VERSION)
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\"domain\": null"), "{text}");
    // Both cookies now carry the hostname.
    assert_eq!(
        text.matches("\"domain\": \"example.org\"").count(),
        2,
        "{text}"
    );
}

#[test]
fn upgrade_flag_may_follow_positionals() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "api", LEGACY_COOKIES);
    sessions::upgrade_in(&args(&["example.org", "api", "--bind-cookies"]), dir.path()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\"domain\": null"), "{text}");
}

#[test]
fn upgrade_header_dict_becomes_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "api", LEGACY_HEADERS);
    let output = sessions::upgrade_in(&args(&["example.org", "api"]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!("Upgraded 'api' @ 'example.org' to v{}\n", crate::VERSION)
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"headers\": ["), "{text}");
    assert!(text.contains("\"name\": \"X-Data\""), "{text}");
    assert!(text.contains("\"value\": \"value\""), "{text}");
}

#[test]
fn upgraded_file_loads_without_legacy_warning() {
    let dir = tempfile::tempdir().unwrap();
    let both = concat!(
        "{\"headers\": {\"X\": \"v\"},",
        "\"cookies\": {\"s\": {\"value\": \"1\", \"domain\": \"\", ",
        "\"path\": \"/\", \"expires\": null, \"secure\": false}}}",
    );
    let path = write_session(dir.path(), "example.org", "api", both);
    let output = sessions::upgrade_in(&args(&["example.org", "api"]), dir.path()).unwrap();
    assert!(output.starts_with("Upgraded 'api'"), "{output}");
    let reloaded = crate::session::Session::load(&path, 0).unwrap();
    assert!(!reloaded.needs_upgrade());
    assert!(
        reloaded
            .legacy_warning("api", "example.org", true)
            .is_none()
    );
    // A second upgrade reports up to date.
    let output = sessions::upgrade_in(&args(&["example.org", "api"]), dir.path()).unwrap();
    assert_eq!(output, "'api' @ 'example.org' is already up to date.\n");
}

#[test]
fn upgrade_resolves_path_based_session_directly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anon-session.json");
    std::fs::write(&path, LEGACY_HEADERS).unwrap();
    // A value containing a path separator bypasses the sessions dir; the
    // reported name is the file stem.
    let output =
        sessions::upgrade_in(&args(&["example.org", path.to_str().unwrap()]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!(
            "Upgraded 'anon-session' @ 'example.org' to v{}\n",
            crate::VERSION
        )
    );
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"headers\": [")
    );
}

#[test]
fn upgrade_host_with_port_uses_underscored_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_session(dir.path(), "example.org_8080", "api", LEGACY_HEADERS);
    let output = sessions::upgrade_in(&args(&["example.org:8080", "api"]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!(
            "Upgraded 'api' @ 'example.org:8080' to v{}\n",
            crate::VERSION
        )
    );
}

#[test]
fn upgrade_corrupt_session_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "example.org", "bad", "{not json");
    let error = sessions::upgrade_in(&args(&["example.org", "bad"]), dir.path()).unwrap_err();
    let message = error.render();
    assert!(message.contains("invalid session file"), "{message}");
    assert!(message.contains(path.to_str().unwrap()), "{message}");
}

#[test]
fn upgrade_all_walks_hosts_in_sorted_order() {
    let dir = tempfile::tempdir().unwrap();
    write_session(dir.path(), "b.example", "old", LEGACY_COOKIES);
    write_session(dir.path(), "a.example", "fresh", MODERN);
    write_session(dir.path(), "a.example", "old", LEGACY_HEADERS);
    // Stray non-JSON entries are ignored.
    std::fs::write(dir.path().join("sessions").join("stray.txt"), "x").unwrap();
    std::fs::write(
        dir.path()
            .join("sessions")
            .join("a.example")
            .join("notes.txt"),
        "x",
    )
    .unwrap();

    let output = sessions::upgrade_all_in(&args(&[]), dir.path()).unwrap();
    assert_eq!(
        output,
        format!(
            "'fresh' @ 'a.example' is already up to date.\n\
             Upgraded 'old' @ 'a.example' to v{v}\n\
             Upgraded 'old' @ 'b.example' to v{v}\n",
            v = crate::VERSION
        )
    );

    // Everything is up to date on the second pass.
    let output = sessions::upgrade_all_in(&args(&["--bind-cookies"]), dir.path()).unwrap();
    assert_eq!(
        output,
        "'fresh' @ 'a.example' is already up to date.\n\
         'old' @ 'a.example' is already up to date.\n\
         'old' @ 'b.example' is already up to date.\n"
    );
}

#[test]
fn upgrade_all_bind_uses_host_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_session(dir.path(), "h.example", "api", LEGACY_COOKIES);
    sessions::upgrade_all_in(&args(&["--bind-cookies"]), dir.path()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"domain\": \"h.example\""), "{text}");
    assert!(!text.contains("\"domain\": null"), "{text}");
}

#[test]
fn upgrade_all_without_sessions_directory_errors() {
    let dir = tempfile::tempdir().unwrap();
    let error = sessions::upgrade_all_in(&args(&[]), dir.path()).unwrap_err();
    let message = error.render();
    assert!(
        message.contains("cannot read sessions directory"),
        "{message}"
    );
    assert!(message.contains("sessions"), "{message}");
}

#[test]
fn upgrade_all_stops_at_first_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    write_session(dir.path(), "a.example", "bad", "{not json");
    let good = write_session(dir.path(), "b.example", "old", LEGACY_HEADERS);
    let error = sessions::upgrade_all_in(&args(&[]), dir.path()).unwrap_err();
    assert!(error.render().contains("invalid session file"));
    // The walk aborted before reaching the later host.
    assert_eq!(std::fs::read_to_string(&good).unwrap(), LEGACY_HEADERS);
}
