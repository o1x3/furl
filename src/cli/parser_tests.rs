use super::args::{ParsedArgs, RequestType};
use super::options::{SORTED_FORMAT_OPTIONS, UNSORTED_FORMAT_OPTIONS};
use super::parser::{Outcome, parse};

fn argv(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|t| t.to_string()).collect()
}

#[track_caller]
fn ok(tokens: &[&str]) -> ParsedArgs {
    match parse(&argv(tokens)) {
        Ok(Outcome::Args(args)) => *args,
        Ok(other) => panic!("expected parsed args, got {other:?}"),
        Err(error) => panic!("expected success, got: {}", error.message),
    }
}

#[track_caller]
fn err(tokens: &[&str]) -> String {
    match parse(&argv(tokens)) {
        Err(error) => error.message,
        Ok(_) => panic!("expected a usage error"),
    }
}

#[test]
fn positional_assignment() {
    let args = ok(&["example.org"]);
    assert_eq!(args.method, None);
    assert_eq!(args.url, "example.org");

    let args = ok(&["POST", "example.org", "a=b", "X-Hdr:v"]);
    assert_eq!(args.method.as_deref(), Some("POST"));
    assert_eq!(args.url, "example.org");
    assert_eq!(args.request_items, vec!["a=b", "X-Hdr:v"]);
}

#[test]
fn options_between_positionals_do_not_skew_assignment() {
    let args = ok(&["POST", "--follow", "example.org", "--all", "a=b"]);
    assert_eq!(args.method.as_deref(), Some("POST"));
    assert_eq!(args.url, "example.org");
    assert_eq!(args.request_items, vec!["a=b"]);
    assert!(args.follow);
    assert!(args.all);
}

#[test]
fn missing_url() {
    assert_eq!(err(&[]), "the following arguments are required: URL");
    assert_eq!(err(&["-v"]), "the following arguments are required: URL");
}

#[test]
fn invalid_item_token() {
    assert_eq!(
        err(&["GET", "example.org", "plain"]),
        "argument REQUEST_ITEM: 'plain' is not a valid value"
    );
}

#[test]
fn long_option_value_forms() {
    assert_eq!(ok(&["x", "--print", "Hb"]).print.as_deref(), Some("Hb"));
    assert_eq!(ok(&["x", "--print=Hb"]).print.as_deref(), Some("Hb"));
    assert_eq!(ok(&["x", "--print="]).print.as_deref(), Some(""));
}

#[test]
fn short_option_value_forms() {
    assert_eq!(ok(&["x", "-p", "Hb"]).print.as_deref(), Some("Hb"));
    assert_eq!(ok(&["x", "-pHb"]).print.as_deref(), Some("Hb"));
    assert_eq!(ok(&["x", "-p=Hb"]).print.as_deref(), Some("Hb"));
}

#[test]
fn short_flag_clustering() {
    let args = ok(&["x", "-Iv"]);
    assert!(args.ignore_stdin);
    assert_eq!(args.verbose, 1);
    // A value-taking option inside a cluster eats the rest.
    let args = ok(&["x", "-Ip", "Hb"]);
    assert!(args.ignore_stdin);
    assert_eq!(args.print.as_deref(), Some("Hb"));
}

#[test]
fn abbreviations_match_unambiguous_prefixes() {
    assert!(ok(&["x", "--fol"]).follow);
    assert_eq!(ok(&["x", "--time", "3"]).timeout, 3.0);
    assert_eq!(ok(&["x", "--time=2.5"]).timeout, 2.5);
}

#[test]
fn exact_match_beats_longer_candidates() {
    let args = ok(&["x", "--session", "foo"]);
    assert_eq!(args.session.as_deref(), Some("foo"));
}

#[test]
fn ambiguous_abbreviation_is_an_error() {
    let message = err(&["x", "--se", "foo"]);
    assert!(
        message.starts_with("ambiguous option: --se could match"),
        "got: {message}"
    );
    assert!(message.contains("--session"));
    assert!(message.contains("--session-read-only"));
}

#[test]
fn counting_flags() {
    assert_eq!(ok(&["x", "-vv"]).verbose, 2);
    assert_eq!(ok(&["x", "-v", "-v"]).verbose, 2);
    assert_eq!(ok(&["x", "-q", "--quiet"]).quiet, 2);
    assert_eq!(ok(&["x", "-xx"]).compress, 2);
}

#[test]
fn last_value_wins() {
    assert_eq!(
        ok(&["x", "--pretty=all", "--pretty=none"])
            .pretty
            .as_deref(),
        Some("none")
    );
}

#[test]
fn print_shortcuts_share_one_destination() {
    assert_eq!(ok(&["x", "-h", "-b"]).print.as_deref(), Some("b"));
    assert_eq!(ok(&["x", "--print=HB", "-h"]).print.as_deref(), Some("h"));
    assert_eq!(ok(&["x", "-m"]).print.as_deref(), Some("m"));
}

#[test]
fn request_type_slot_last_wins_without_error() {
    assert_eq!(
        ok(&["x", "--form", "--json"]).request_type,
        Some(RequestType::Json)
    );
    assert_eq!(
        ok(&["x", "--json", "--multipart"]).request_type,
        Some(RequestType::Multipart)
    );
    assert_eq!(ok(&["x"]).request_type, None);
}

#[test]
fn negation_resets_to_default() {
    assert_eq!(ok(&["x", "--verbose", "--no-verbose"]).verbose, 0);
    assert!(!ok(&["x", "--follow", "--no-follow"]).follow);
    assert_eq!(ok(&["x", "--pretty=none", "--no-pretty"]).pretty, None);
    assert_eq!(ok(&["x", "--timeout=5", "--no-timeout"]).timeout, 0.0);
}

#[test]
fn negation_wins_regardless_of_order() {
    assert_eq!(ok(&["x", "--no-verbose", "--verbose"]).verbose, 0);
    assert!(!ok(&["x", "--no-follow", "--follow"]).follow);
}

#[test]
fn negating_any_of_a_shared_destination_resets_it() {
    assert_eq!(ok(&["x", "--form", "--no-json"]).request_type, None);
    assert_eq!(ok(&["x", "--json", "--no-multipart"]).request_type, None);
    assert_eq!(ok(&["x", "-h", "--no-print"]).print, None);
}

#[test]
fn negation_requires_exact_names() {
    assert_eq!(
        err(&["x", "--no-verb"]),
        "unrecognized arguments: --no-verb"
    );
    assert_eq!(
        err(&["x", "--no-verbose=1"]),
        "unrecognized arguments: --no-verbose=1"
    );
    // Short aliases are not negatable.
    assert_eq!(err(&["x", "--no-v"]), "unrecognized arguments: --no-v");
}

#[test]
fn unknown_options_are_unrecognized_arguments() {
    assert_eq!(err(&["x", "--no-war"]), "unrecognized arguments: --no-war");
    assert_eq!(err(&["x", "--bogus"]), "unrecognized arguments: --bogus");
    assert_eq!(err(&["x", "-Z"]), "unrecognized arguments: -Z");
    assert_eq!(
        err(&["x", "--bogus", "-Z"]),
        "unrecognized arguments: --bogus -Z"
    );
}

#[test]
fn negating_terminal_options_is_a_noop() {
    let args = ok(&["x", "--no-help", "--no-version"]);
    assert_eq!(args.url, "x");
}

#[test]
fn invalid_choice_messages() {
    assert_eq!(
        err(&["x", "--pretty=bogus"]),
        "argument --pretty: invalid choice: 'bogus' \
         (choose from 'all', 'colors', 'format', 'none')"
    );
    assert_eq!(
        err(&["x", "-A", "hoba"]),
        "argument -A/--auth-type: invalid choice: 'hoba' \
         (choose from 'basic', 'bearer', 'digest')"
    );
}

#[test]
fn typed_value_errors() {
    assert_eq!(
        err(&["x", "--max-redirects=many"]),
        "argument --max-redirects: invalid int value: 'many'"
    );
    assert_eq!(
        err(&["x", "--timeout=soon"]),
        "argument --timeout: invalid float value: 'soon'"
    );
}

#[test]
fn response_overrides_validate() {
    assert_eq!(
        err(&["x", "--response-mime=weird"]),
        "argument --response-mime: 'weird' doesn't look like a mime type; use type/subtype"
    );
    assert_eq!(
        err(&["x", "--response-charset=utf-64"]),
        "argument --response-charset: 'utf-64' is not a supported encoding"
    );
    let args = ok(&[
        "x",
        "--response-mime=application/json",
        "--response-charset=big5",
    ]);
    assert_eq!(args.response_mime.as_deref(), Some("application/json"));
    assert_eq!(args.response_charset.as_deref(), Some("big5"));
}

#[test]
fn session_group_is_mutually_exclusive() {
    assert_eq!(
        err(&["x", "--session", "a", "--session-read-only", "b"]),
        "argument --session-read-only: not allowed with argument --session"
    );
    assert_eq!(
        err(&["x", "--session-read-only", "b", "--session", "a"]),
        "argument --session: not allowed with argument --session-read-only"
    );
}

#[test]
fn session_name_validation() {
    assert_eq!(
        err(&["x", "--session", "bad name!"]),
        "argument --session: Session name contains invalid characters."
    );
    assert_eq!(
        ok(&["x", "--session", "ok_na.me-1"]).session.as_deref(),
        Some("ok_na.me-1")
    );
    // Values with a path separator skip the pattern.
    assert_eq!(
        ok(&["x", "--session", "./spaced name.json"])
            .session
            .as_deref(),
        Some("./spaced name.json")
    );
}

#[test]
fn proxy_validates_and_accumulates() {
    assert_eq!(
        err(&["x", "--proxy", "noseparator"]),
        "argument --proxy: 'noseparator' is not a valid value"
    );
    let args = ok(&[
        "x",
        "--proxy=http:http://foo:3128",
        "--proxy=https:http://bar:8080",
    ]);
    assert_eq!(
        args.proxy,
        vec!["http:http://foo:3128", "https:http://bar:8080"]
    );
}

#[test]
fn missing_value_errors() {
    assert_eq!(
        err(&["x", "--print"]),
        "argument -p/--print: expected one argument"
    );
    assert_eq!(
        err(&["x", "--print", "--follow"]),
        "argument -p/--print: expected one argument"
    );
}

#[test]
fn explicit_argument_on_a_flag_is_an_error() {
    assert_eq!(
        err(&["x", "--follow=yes"]),
        "argument -F/--follow: ignored explicit argument 'yes'"
    );
}

#[test]
fn double_dash_ends_option_parsing() {
    let args = ok(&["example.org", "--", "--follow=x"]);
    assert_eq!(args.method.as_deref(), Some("example.org"));
    assert_eq!(args.url, "--follow=x");
    assert!(!args.follow);
}

#[test]
fn negative_numbers_are_positionals() {
    let args = ok(&["-5", "example.org"]);
    assert_eq!(args.method.as_deref(), Some("-5"));
    assert_eq!(args.url, "example.org");
    let args = ok(&["-12.5", "example.org"]);
    assert_eq!(args.method.as_deref(), Some("-12.5"));
    // Anything beyond a plain negative number is option-like again.
    assert_eq!(
        err(&["example.org", "-5.5=x"]),
        "unrecognized arguments: -5.5=x"
    );
}

#[test]
fn option_looking_items_need_double_dash() {
    assert_eq!(
        err(&["example.org", "-weird=x"]),
        "unrecognized arguments: -weird=x"
    );
    let args = ok(&["POST", "example.org", "--", "-weird=x"]);
    assert_eq!(args.request_items, vec!["-weird=x"]);
}

#[test]
fn terminal_actions() {
    assert!(matches!(parse(&argv(&["--version"])), Ok(Outcome::Version)));
    assert!(matches!(parse(&argv(&["--help"])), Ok(Outcome::Help)));
    assert!(matches!(parse(&argv(&["--manual"])), Ok(Outcome::Manual)));
    // Terminal actions fire mid-scan, before later validation problems.
    assert!(matches!(
        parse(&argv(&["--help", "--bogus"])),
        Ok(Outcome::Help)
    ));
}

#[test]
fn format_options_accumulate_in_order() {
    let args = ok(&["x", "--sorted", "--format-options", "a.b:c", "--unsorted"]);
    assert_eq!(
        args.format_options,
        vec![SORTED_FORMAT_OPTIONS, "a.b:c", UNSORTED_FORMAT_OPTIONS]
    );
}

#[test]
fn hidden_no_sorted_variants_are_real_options() {
    let args = ok(&["x", "--no-sorted"]);
    assert_eq!(args.format_options, vec![UNSORTED_FORMAT_OPTIONS]);
    let args = ok(&["x", "--no-unsorted"]);
    assert_eq!(args.format_options, vec![SORTED_FORMAT_OPTIONS]);
}

#[test]
fn no_format_options_resets_the_accumulated_list() {
    let args = ok(&[
        "x",
        "--sorted",
        "--format-options=a.b:c",
        "--no-format-options",
    ]);
    assert!(args.format_options.is_empty());
}

#[test]
fn config_style_prepending_lets_cli_override() {
    // Simulates config default_options followed by CLI args.
    let args = ok(&["--form", "x", "--json"]);
    assert_eq!(args.request_type, Some(RequestType::Json));
    let args = ok(&["--verbose", "x", "--verbose"]);
    assert_eq!(args.verbose, 2);
}

#[test]
fn unicode_values_pass_through() {
    let args = ok(&["x", "--auth", "usér:páss"]);
    assert_eq!(args.auth.as_deref(), Some("usér:páss"));
}
