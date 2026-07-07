use std::io::{Read, Write};
use std::net::TcpListener;

use crate::cli::items::process_items;
use crate::cli::parser::{Outcome, parse};
use crate::request::{BuildContext, build};

use super::{TransportOptions, send};

/// Serve one canned response on a local socket, returning what the
/// client sent and the transport's parsed result.
fn roundtrip(
    canned: &[u8],
    argv_tail: &[&str],
) -> (Vec<u8>, Result<super::RawResponse, super::TransportError>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().unwrap().port();
    let canned = canned.to_vec();
    // The server reports what it read over a channel so the test can wait
    // with a bound and never hang on a stuck thread.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        // Reply first, so the client's read never waits on a server that
        // is itself waiting to finish reading the request (which would
        // deadlock a one-connection exchange). Then drain the request for
        // the caller's assertions, bounded by a short read timeout.
        socket.write_all(&canned).ok();
        socket
            .set_read_timeout(Some(std::time::Duration::from_millis(400)))
            .ok();
        let mut received = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match socket.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => received.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        socket.shutdown(std::net::Shutdown::Both).ok();
        let _ = tx.send(received);
    });

    // Split the tail into flags and request items, then build an
    // unambiguous `[flags…, METHOD, URL, items…]` argv so items never
    // land in the METHOD/URL positional slots.
    let (flags, items): (Vec<&str>, Vec<&str>) = argv_tail.iter().partition(|t| t.starts_with('-'));
    let method = if items.is_empty() { "GET" } else { "POST" };
    let mut argv: Vec<String> = flags.iter().map(|t| t.to_string()).collect();
    argv.push(method.to_string());
    argv.push(format!("127.0.0.1:{port}"));
    argv.extend(items.iter().map(|t| t.to_string()));
    let Ok(Outcome::Args(args)) = parse(&argv) else {
        panic!("test argv must parse");
    };
    let items = process_items(&args.request_items, args.request_type).expect("items");
    let request = build(&BuildContext {
        args: &args,
        items: &items,
        stdin_body: None,
        default_scheme: "http",
        session_headers: &[],
        session_authorization: None,
        netrc_authorization: None,
        version: "0.1.0",
    })
    .expect("build");

    let result = send(
        &request,
        &TransportOptions {
            timeout: Some(std::time::Duration::from_secs(5)),
            tls: super::tls::TlsOptions::default(),
            max_headers: 0,
            proxy: None,
        },
    );
    let received = rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("server thread did not report in time");
    (received, result)
}

#[test]
fn basic_roundtrip_preserves_raw_response_details() {
    let (received, result) = roundtrip(
        b"HTTP/1.1 200 Custom Reason\r\nX-MiXeD-CaSe: value\r\nContent-Length: 5\r\n\r\nhello",
        &[],
    );
    let text = String::from_utf8_lossy(&received);
    assert!(
        text.starts_with("GET / HTTP/1.1\r\nHost: 127.0.0.1:"),
        "{text}"
    );
    assert!(text.contains("\r\nAccept-Encoding: gzip, deflate\r\n"));
    assert!(text.ends_with("\r\n\r\n"));

    let response = result.expect("response");
    assert_eq!(response.status, 200);
    assert_eq!(response.reason, "Custom Reason");
    assert_eq!(response.http_version, "1.1");
    assert_eq!(response.headers[0].0, "X-MiXeD-CaSe");
    assert_eq!(response.body, b"hello");
}

#[test]
fn chunked_response_bodies_reassemble() {
    let (_, result) = roundtrip(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        &[],
    );
    assert_eq!(result.expect("response").body, b"hello world");
}

#[test]
fn read_to_close_bodies_work() {
    let (_, result) = roundtrip(b"HTTP/1.0 200 OK\r\n\r\nuntil close", &[]);
    let response = result.expect("response");
    assert_eq!(response.http_version, "1.0");
    assert_eq!(response.body, b"until close");
}

#[test]
fn interim_100_responses_are_skipped() {
    let (_, result) = roundtrip(
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n",
        &[],
    );
    let response = result.expect("response");
    assert_eq!(response.status, 204);
    assert!(response.body.is_empty());
}

#[test]
fn request_body_and_chunked_upload() {
    let (received, _) = roundtrip(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", &["a=b"]);
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("POST / HTTP/1.1\r\n"), "{text}");
    assert!(text.ends_with("\r\n\r\n{\"a\": \"b\"}"), "{text}");

    let (received, _) = roundtrip(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        &["--chunked", "a=b"],
    );
    let text = String::from_utf8_lossy(&received);
    assert!(text.contains("Transfer-Encoding: chunked"));
    assert!(text.ends_with("a\r\n{\"a\": \"b\"}\r\n0\r\n\r\n"), "{text}");
}

#[test]
fn gzip_bodies_decode_for_display() {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(b"compressed payload").unwrap();
    let gz = encoder.finish().unwrap();
    let mut canned = format!(
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
        gz.len()
    )
    .into_bytes();
    canned.extend_from_slice(&gz);

    let (_, result) = roundtrip(&canned, &[]);
    let response = result.expect("response");
    assert_ne!(response.body, b"compressed payload");
    assert_eq!(super::decoded_body(&response), b"compressed payload");
}

#[test]
fn max_headers_is_enforced() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer);
        let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
        for i in 0..20 {
            response.extend_from_slice(format!("X-H{i}: v\r\n").as_bytes());
        }
        response.extend_from_slice(b"Content-Length: 0\r\n\r\n");
        socket.write_all(&response).unwrap();
    });

    let argv = vec![format!("127.0.0.1:{port}")];
    let Ok(Outcome::Args(mut args)) = parse(&argv) else {
        panic!()
    };
    args.method = Some("GET".to_string());
    let items = process_items(&[], None).unwrap();
    let request = build(&BuildContext {
        args: &args,
        items: &items,
        stdin_body: None,
        default_scheme: "http",
        session_headers: &[],
        session_authorization: None,
        netrc_authorization: None,
        version: "0.1.0",
    })
    .unwrap();
    let result = send(
        &request,
        &TransportOptions {
            timeout: Some(std::time::Duration::from_secs(5)),
            tls: super::tls::TlsOptions::default(),
            max_headers: 10,
            proxy: None,
        },
    );
    assert!(matches!(
        result,
        Err(super::TransportError::TooManyHeaders(n)) if n > 10
    ));
}
