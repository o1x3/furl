use crate::cli::items::process_items;
use crate::cli::parser::{Outcome, parse};

use super::{BuildContext, BuildError, PreparedRequest, build};

/// Parse a full command line and build; the method must already be
/// explicit (method guessing lives in the run flow).
#[track_caller]
fn prepared_with_stdin(
    tokens: &[&str],
    stdin: Option<&[u8]>,
) -> Result<PreparedRequest, BuildError> {
    let argv: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
    let Ok(Outcome::Args(mut args)) = parse(&argv) else {
        panic!("test command line must parse");
    };
    if args.method.is_none() {
        args.method = Some("GET".to_string());
    }
    let items = process_items(&args.request_items, args.request_type).expect("items");
    build(&BuildContext {
        args: &args,
        items: &items,
        stdin_body: stdin.map(|b| b.to_vec()),
        default_scheme: "http",
        session_headers: &[],
        session_authorization: None,
        netrc_authorization: None,
        version: "0.1.0",
    })
}

#[track_caller]
fn prepared(tokens: &[&str]) -> PreparedRequest {
    match prepared_with_stdin(tokens, None) {
        Ok(request) => request,
        Err(error) => panic!("build failed: {error:?}"),
    }
}

fn header_names(request: &PreparedRequest) -> Vec<&str> {
    request
        .headers
        .entries
        .iter()
        .map(|(n, _)| n.as_str())
        .collect()
}

#[test]
fn bare_get_header_order() {
    let request = prepared(&["GET", "example.org"]);
    assert_eq!(request.method, "GET");
    assert_eq!(
        header_names(&request),
        vec!["Accept-Encoding", "Accept", "Connection", "User-Agent"]
    );
    assert_eq!(request.headers.get("Accept"), Some("*/*"));
    assert_eq!(request.headers.get("User-Agent"), Some("furl/0.1.0"));
    assert!(request.body.is_none());
    assert_eq!(request.host_netloc, "example.org");
}

#[test]
fn json_post_header_order_and_body() {
    let request = prepared(&["POST", "example.org", "name=furl", "num:=3"]);
    assert_eq!(
        header_names(&request),
        vec![
            "Accept-Encoding",
            "Connection",
            "Content-Length",
            "User-Agent",
            "Accept",
            "Content-Type"
        ]
    );
    assert_eq!(
        request.headers.get("Accept"),
        Some("application/json, */*;q=0.5")
    );
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/json")
    );
    let body = request.body.as_ref().unwrap();
    assert_eq!(body.bytes, br#"{"name": "furl", "num": 3}"#);
    assert_eq!(request.headers.get("Content-Length"), Some("26"));
}

#[test]
fn form_body_and_content_type() {
    let request = prepared(&["POST", "example.org", "-f", "a b=c d"]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/x-www-form-urlencoded; charset=utf-8")
    );
    assert_eq!(request.body.as_ref().unwrap().bytes, b"a+b=c+d");
}

#[test]
fn bodiless_post_has_content_length_zero() {
    let request = prepared(&["POST", "example.org"]);
    assert_eq!(request.headers.get("Content-Length"), Some("0"));
    // …and stays on the engine side of the ordering.
    assert_eq!(
        header_names(&request),
        vec![
            "Accept-Encoding",
            "Accept",
            "Connection",
            "Content-Length",
            "User-Agent"
        ]
    );
}

#[test]
fn options_drops_the_implied_content_length() {
    let request = prepared(&["OPTIONS", "example.org"]);
    assert_eq!(request.headers.get("Content-Length"), None);
    let request = prepared(&["options", "example.org"]);
    assert_eq!(request.method, "OPTIONS");
    assert_eq!(request.headers.get("Content-Length"), None);
}

#[test]
fn bodiless_get_has_no_content_length() {
    let request = prepared(&["GET", "example.org"]);
    assert_eq!(request.headers.get("Content-Length"), None);
}

#[test]
fn raw_body_gets_json_defaults() {
    let request = prepared(&["POST", "example.org", "--raw=xyz"]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/json")
    );
    assert_eq!(
        request.headers.get("Accept"),
        Some("application/json, */*;q=0.5")
    );
    assert_eq!(request.body.as_ref().unwrap().bytes, b"xyz");
    assert_eq!(request.headers.get("Content-Length"), Some("3"));
}

#[test]
fn stdin_body_counts_as_data() {
    let request = prepared_with_stdin(&["POST", "example.org"], Some(b"piped")).unwrap();
    assert_eq!(request.body.as_ref().unwrap().bytes, b"piped");
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/json")
    );
}

#[test]
fn explicit_json_without_body_sets_headers() {
    let request = prepared(&["GET", "example.org", "--json"]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("application/json")
    );
    assert!(request.body.is_none());
}

#[test]
fn user_headers_override_defaults_in_place() {
    let request = prepared(&[
        "GET",
        "example.org",
        "User-Agent:custom",
        "Accept:text/plain",
    ]);
    assert_eq!(request.headers.get("User-Agent"), Some("custom"));
    assert_eq!(request.headers.get("Accept"), Some("text/plain"));
    // No engine Accept slot when the application layer holds one.
    assert_eq!(
        header_names(&request),
        vec!["Accept-Encoding", "Connection", "User-Agent", "Accept"]
    );
}

#[test]
fn deleted_headers_suppress_defaults() {
    let request = prepared(&[
        "GET",
        "example.org",
        "Accept:",
        "Accept-Encoding:",
        "User-Agent:",
    ]);
    assert_eq!(header_names(&request), vec!["Connection"]);
    let request = prepared(&["GET", "example.org", "Host:"]);
    assert!(request.headers.skip_host);
}

#[test]
fn multi_value_headers_accumulate_after_deletion_wipe() {
    let request = prepared(&["GET", "example.org", "Foo:bar", "Foo:baz"]);
    let foo: Vec<&str> = request
        .headers
        .entries
        .iter()
        .filter(|(n, _)| n == "Foo")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(foo, vec!["bar", "baz"]);

    let request = prepared(&["GET", "example.org", "Foo:bar", "Foo:", "Foo:quux"]);
    let foo: Vec<&str> = request
        .headers
        .entries
        .iter()
        .filter(|(n, _)| n == "Foo")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(foo, vec!["quux"]);
}

#[test]
fn empty_value_header_is_sent_empty() {
    let request = prepared(&["GET", "example.org", "Empty;"]);
    assert_eq!(request.headers.get("Empty"), Some(""));
}

#[test]
fn query_params_merge_after_existing_query() {
    let request = prepared(&["GET", "example.org/p?x=1", "q==a b", "q==2"]);
    assert_eq!(request.request_target(), "/p?x=1&q=a+b&q=2");
    let request = prepared(&["GET", "example.org", "q==1"]);
    assert_eq!(request.request_target(), "/?q=1");
}

#[test]
fn dot_segments_squash_unless_path_as_is() {
    let request = prepared(&["GET", "example.org/../../etc/password"]);
    assert_eq!(request.request_target(), "/etc/password");
    let request = prepared(&[
        "GET",
        "example.org/../../etc/password",
        "--path-as-is",
        "param==value",
    ]);
    assert_eq!(request.request_target(), "/../../etc/password?param=value");
}

#[test]
fn method_is_uppercased() {
    assert_eq!(prepared(&["get", "example.org"]).method, "GET");
}

#[test]
fn userinfo_becomes_basic_auth_and_leaves_host() {
    let request = prepared(&["GET", "http://user:pw@example.org:8080/p"]);
    assert_eq!(
        request.headers.get("Authorization"),
        Some("Basic dXNlcjpwdw==")
    );
    assert_eq!(request.host_netloc, "example.org:8080");
    // Percent-encoded credentials decode before encoding.
    let request = prepared(&["GET", "http://user%40x:p%23w@example.org/"]);
    let expected = super::basic_authorization("user@x", "p#w");
    assert_eq!(
        request.headers.get("Authorization"),
        Some(expected.as_str())
    );
}

#[test]
fn explicit_auth_beats_userinfo() {
    let request = prepared(&["GET", "http://a:b@example.org/", "-a", "c:d"]);
    let expected = super::basic_authorization("c", "d");
    assert_eq!(
        request.headers.get("Authorization"),
        Some(expected.as_str())
    );
}

#[test]
fn bearer_auth() {
    let request = prepared(&["GET", "example.org", "-A", "bearer", "-a", "to:ken"]);
    assert_eq!(request.headers.get("Authorization"), Some("Bearer to:ken"));
}

#[test]
fn digest_sends_no_preemptive_header() {
    let request = prepared(&["GET", "example.org", "-A", "digest", "-a", "u:p"]);
    assert_eq!(request.headers.get("Authorization"), None);
}

#[test]
fn missing_password_requests_a_prompt() {
    let error = prepared_with_stdin(&["GET", "example.org", "-a", "useronly"], None).unwrap_err();
    assert!(matches!(
        error,
        BuildError::PasswordRequired { user } if user == "useronly"
    ));
}

#[test]
fn trailing_colon_means_empty_password() {
    let request = prepared(&["GET", "example.org", "-a", "user:"]);
    let expected = super::basic_authorization("user", "");
    assert_eq!(
        request.headers.get("Authorization"),
        Some(expected.as_str())
    );
}

#[test]
fn body_source_mixing_is_rejected() {
    let error = prepared_with_stdin(&["POST", "example.org", "a=b"], Some(b"stdin")).unwrap_err();
    assert!(matches!(error, BuildError::Usage(m) if m.contains("cannot be mixed")));
    let error =
        prepared_with_stdin(&["POST", "example.org", "--raw=x"], Some(b"stdin")).unwrap_err();
    assert!(matches!(error, BuildError::Usage(m) if m.contains("cannot be mixed")));
    let error = prepared_with_stdin(&["POST", "example.org", "--raw=x", "a=b"], None).unwrap_err();
    assert!(matches!(error, BuildError::Usage(m) if m.contains("cannot be mixed")));
}

#[test]
fn compress_content_encoding_follows_cli_headers() {
    // Content-Encoding lands after the CLI-applied headers, not before.
    let request = prepared(&["POST", "example.org", "-xx", "X-Foo:bar", "data=x"]);
    let names: Vec<&str> = request
        .headers
        .entries
        .iter()
        .map(|(n, _)| n.as_str())
        .collect();
    let foo = names.iter().position(|&n| n == "X-Foo").unwrap();
    let enc = names.iter().position(|&n| n == "Content-Encoding").unwrap();
    assert!(
        foo < enc,
        "X-Foo should precede Content-Encoding: {names:?}"
    );
}

#[test]
fn computed_auth_overrides_raw_authorization_header() {
    // -a credentials replace a raw Authorization header rather than the
    // other way around.
    let request = prepared(&[
        "GET",
        "example.org",
        "-a",
        "u:p",
        "Authorization:Bearer raw",
    ]);
    let expected = super::basic_authorization("u", "p");
    assert_eq!(
        request.headers.get("Authorization"),
        Some(expected.as_str())
    );
    let count = request
        .headers
        .entries
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        .count();
    assert_eq!(count, 1, "only the computed Authorization survives");
}

#[test]
fn raw_authorization_header_stands_without_computed_auth() {
    let request = prepared(&["GET", "example.org", "Authorization:Bearer raw"]);
    assert_eq!(request.headers.get("Authorization"), Some("Bearer raw"));
}

#[test]
fn compress_conflicts() {
    let error =
        prepared_with_stdin(&["POST", "example.org", "-x", "--chunked", "a=b"], None).unwrap_err();
    assert!(matches!(error, BuildError::Usage(m) if m.contains("--chunked")));
    let error = prepared_with_stdin(&["POST", "example.org", "-x", "--multipart", "a=b"], None)
        .unwrap_err();
    assert!(matches!(error, BuildError::Usage(m) if m.contains("--multipart")));
}

#[test]
fn chunked_adds_transfer_encoding_after_everything() {
    // Content-Length is still computed; the chunked marker rides last.
    let request = prepared(&["POST", "example.org", "--chunked", "a=b"]);
    assert_eq!(request.headers.get("Transfer-Encoding"), Some("chunked"));
    assert_eq!(request.headers.get("Content-Length"), Some("10"));
    assert_eq!(
        request.headers.entries.last().map(|(n, _)| n.as_str()),
        Some("Transfer-Encoding")
    );
}

#[test]
fn multipart_body_and_boundary() {
    let request = prepared(&[
        "POST",
        "example.org",
        "--multipart",
        "--boundary=BBB",
        "a=1",
    ]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("multipart/form-data; boundary=BBB")
    );
    let body = String::from_utf8(request.body.as_ref().unwrap().bytes.clone()).unwrap();
    assert!(body.starts_with("--BBB\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n1\r\n"));
    assert!(body.ends_with("--BBB--\r\n"));
}

#[test]
fn multipart_user_content_type_gains_boundary() {
    let request = prepared(&[
        "POST",
        "example.org",
        "--multipart",
        "--boundary=BBB",
        "Content-Type:multipart/magic",
        "a=1",
    ]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("multipart/magic; boundary=BBB")
    );
    let request = prepared(&[
        "POST",
        "example.org",
        "--multipart",
        "--boundary=BBB",
        "Content-Type:multipart/x; boundary=OTHER",
        "a=1",
    ]);
    assert_eq!(
        request.headers.get("Content-Type"),
        Some("multipart/x; boundary=OTHER")
    );
}

#[test]
fn invalid_urls_error() {
    let error = prepared_with_stdin(&["GET", "://"], None).unwrap_err();
    assert!(
        matches!(&error, BuildError::InvalidUrl { url, reason }
            if url == "http://" && reason == "No host supplied"),
        "got {error:?}"
    );
}

#[test]
fn top_level_array_body() {
    let request = prepared(&["POST", "example.org", "[]:=1", "[]:=2"]);
    assert_eq!(request.body.as_ref().unwrap().bytes, b"[1, 2]");
}

#[test]
fn compress_twice_forces_deflate() {
    let request = prepared(&["POST", "example.org", "-xx", "data=x"]);
    assert_eq!(request.headers.get("Content-Encoding"), Some("deflate"));
    let body = &request.body.as_ref().unwrap().bytes;
    assert_eq!(&body[..2], &[0x78, 0x9c]);
    assert_eq!(
        request.headers.get("Content-Length"),
        Some(body.len().to_string().as_str())
    );
}

#[test]
fn compress_once_skips_when_not_smaller() {
    let request = prepared(&["POST", "example.org", "-x", "a=b"]);
    assert_eq!(request.headers.get("Content-Encoding"), None);
    assert_eq!(request.body.as_ref().unwrap().bytes, br#"{"a": "b"}"#);
}

#[test]
fn compress_once_applies_when_smaller() {
    let big = format!("data={}", "a".repeat(500));
    let request = prepared(&["POST", "example.org", "-x", big.as_str()]);
    assert_eq!(request.headers.get("Content-Encoding"), Some("deflate"));
    assert!(request.body.as_ref().unwrap().bytes.len() < 500);
}

#[test]
fn unicode_json_body_is_ascii_escaped_on_the_wire() {
    let request = prepared(&["POST", "example.org", "name=é"]);
    assert_eq!(
        request.body.as_ref().unwrap().bytes,
        br#"{"name": "\u00e9"}"#
    );
}

#[test]
fn requote_encodes_unsafe_and_normalizes_escapes() {
    // Brackets and pipes in path and query are percent-encoded.
    assert_eq!(
        prepared(&["GET", "http://x.com/[p]?q={v}"]).request_target(),
        "/%5Bp%5D?q=%7Bv%7D"
    );
    assert_eq!(
        prepared(&["GET", "http://x.com/x?arr[]=1&arr[]=2"]).request_target(),
        "/x?arr%5B%5D=1&arr%5B%5D=2"
    );
    // Existing escapes uppercase; unreserved escapes decode.
    assert_eq!(
        prepared(&["GET", "http://x.com/a%2fb?c%2fd"]).request_target(),
        "/a%2Fb?c%2Fd"
    );
    assert_eq!(
        prepared(&["GET", "http://x.com/%7euser"]).request_target(),
        "/~user"
    );
    // A lone or malformed `%` becomes `%25`.
    assert_eq!(
        prepared(&["GET", "http://x.com/pa th/%zz"]).request_target(),
        "/pa%20th/%25zz"
    );
    assert_eq!(
        prepared(&["GET", "http://x.com/100%done"]).request_target(),
        "/100%25done"
    );
    // `?` stays inside the query but a bare trailing `?` drops.
    assert_eq!(
        prepared(&["GET", "http://x.com/a?q=a b&r=c|d"]).request_target(),
        "/a?q=a%20b&r=c%7Cd"
    );
}
