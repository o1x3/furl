use super::dispatch;

#[track_caller]
fn ok(argv: &[&str]) -> String {
    dispatch(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>()).expect("expected success")
}

#[track_caller]
fn err(argv: &[&str]) -> String {
    dispatch(&argv.iter().map(|a| a.to_string()).collect::<Vec<_>>())
        .err()
        .expect("expected error")
        .render()
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
    assert!(err(&["cli", "sessions"]).contains("'upgrade', 'upgrade-all'"));
    assert!(err(&["cli", "sessions", "upgrade"]).contains("not yet implemented"));
}
