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
    // The error names the configured limit, not the count seen.
    assert!(matches!(
        result,
        Err(super::TransportError::TooManyHeaders(10))
    ));
}

// ---------------------------------------------------------------------------
// TLS: cipher restriction and client-certificate keys against a local
// TLS server built from the fixtures in `tls::fixtures`.
// ---------------------------------------------------------------------------

use super::tls::fixtures;

/// TLS fixture material written to disk for the path-based options.
struct TlsFiles {
    // Held for its Drop: deletes the directory with the tests' key material.
    _dir: tempfile::TempDir,
    ca: std::path::PathBuf,
    client_cert: std::path::PathBuf,
    client_key_encrypted: std::path::PathBuf,
}

fn write_tls_files() -> TlsFiles {
    let dir = tempfile::tempdir().expect("tempdir");
    let write = |name: &str, content: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, content).expect("write fixture");
        path
    };
    TlsFiles {
        ca: write("ca.pem", fixtures::SERVER_CERT),
        client_cert: write("client.crt", fixtures::CLIENT_CERT),
        client_key_encrypted: write("client-enc.key", fixtures::CLIENT_KEY_ENCRYPTED),
        _dir: dir,
    }
}

/// One HTTPS exchange against a local TLS server. The 200 response body
/// carries the negotiated cipher-suite name so tests can assert on it.
fn tls_exchange(
    tls: super::tls::TlsOptions,
    require_client_cert: bool,
) -> Result<super::RawResponse, super::TransportError> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || serve_tls_once(&listener, require_client_cert));

    let argv = vec!["GET".to_string(), format!("127.0.0.1:{port}")];
    let Ok(Outcome::Args(args)) = parse(&argv) else {
        panic!("test argv must parse");
    };
    let items = process_items(&[], None).expect("items");
    let request = build(&BuildContext {
        args: &args,
        items: &items,
        stdin_body: None,
        default_scheme: "https",
        session_headers: &[],
        session_authorization: None,
        netrc_authorization: None,
        version: "0.1.0",
    })
    .expect("build");
    send(
        &request,
        &TransportOptions {
            timeout: Some(std::time::Duration::from_secs(5)),
            tls,
            max_headers: 0,
            proxy: None,
        },
    )
}

/// Accept one connection, answer one request, report the negotiated
/// suite. Handshake failures just end the exchange: the client side's
/// error is what the tests assert on.
fn serve_tls_once(listener: &TcpListener, require_client_cert: bool) {
    use rustls_pki_types::pem::PemObject;
    let certs = vec![
        rustls_pki_types::CertificateDer::from_pem_slice(fixtures::SERVER_CERT.as_bytes())
            .expect("server cert"),
    ];
    let key = rustls_pki_types::PrivateKeyDer::from_pem_slice(fixtures::SERVER_KEY.as_bytes())
        .expect("server key");
    let builder = rustls::ServerConfig::builder();
    let config = if require_client_cert {
        builder.with_client_cert_verifier(std::sync::Arc::new(AnyClientCert))
    } else {
        builder.with_no_client_auth()
    }
    .with_single_cert(certs, key)
    .expect("server config");

    let Ok((tcp, _)) = listener.accept() else {
        return;
    };
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let Ok(connection) = rustls::ServerConnection::new(std::sync::Arc::new(config)) else {
        return;
    };
    let mut tls = rustls::StreamOwned::new(connection, tcp);
    // Read the request head; a handshake error surfaces here.
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tls.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                received.extend_from_slice(&chunk[..n]);
                if received.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let suite = tls
        .conn
        .negotiated_cipher_suite()
        .and_then(|suite| suite.suite().as_str())
        .unwrap_or("unknown");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{suite}",
        suite.len()
    );
    let _ = tls.write_all(response.as_bytes());
    let _ = tls.flush();
}

/// Test-only client-cert verifier: any presented certificate passes,
/// but one must be presented (client auth stays mandatory).
#[derive(Debug)]
struct AnyClientCert;

impl rustls::server::danger::ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &rustls_pki_types::CertificateDer<'_>,
        _intermediates: &[rustls_pki_types::CertificateDer<'_>],
        _now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls_pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[test]
fn tls_exchange_works_with_a_custom_ca() {
    let files = write_tls_files();
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        ..Default::default()
    };
    let response = tls_exchange(options, false).expect("TLS roundtrip");
    assert_eq!(response.status, 200);
    assert!(!response.body.is_empty());
}

#[test]
fn ciphers_restrict_the_negotiated_suite() {
    let files = write_tls_files();
    // The IANA name, exercising the prefix-insensitive match.
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        ciphers: Some("TLS_AES_256_GCM_SHA384".to_string()),
        ..Default::default()
    };
    let response = tls_exchange(options, false).expect("TLS roundtrip");
    assert_eq!(response.body, b"TLS13_AES_256_GCM_SHA384");

    // A TLS 1.2 ECDSA suite, exact rustls name.
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        ciphers: Some("TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256".to_string()),
        ..Default::default()
    };
    let response = tls_exchange(options, false).expect("TLS roundtrip");
    assert_eq!(response.body, b"TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256");
}

#[test]
fn unmatched_ciphers_fail_the_request_as_a_tls_error() {
    let files = write_tls_files();
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        ciphers: Some("BOGUS".to_string()),
        ..Default::default()
    };
    let error = tls_exchange(options, false).unwrap_err();
    let super::TransportError::Tls(message) = error else {
        panic!("expected a TLS error");
    };
    assert!(message.contains("no cipher can be selected"), "{message}");
    assert!(message.contains("BOGUS"), "{message}");
}

#[test]
fn client_cert_with_encrypted_key_authenticates() {
    let files = write_tls_files();
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        client_cert: Some(files.client_cert.clone()),
        client_key: Some(files.client_key_encrypted.clone()),
        cert_key_pass: Some(fixtures::CLIENT_KEY_PASSPHRASE.to_string()),
        ..Default::default()
    };
    let response = tls_exchange(options, true).expect("mutual TLS roundtrip");
    assert_eq!(response.status, 200);
}

#[test]
fn wrong_key_passphrase_fails_before_the_handshake() {
    let files = write_tls_files();
    let options = super::tls::TlsOptions {
        verify: super::tls::Verification::CaBundle(files.ca.clone()),
        client_cert: Some(files.client_cert.clone()),
        client_key: Some(files.client_key_encrypted.clone()),
        cert_key_pass: Some("not-the-passphrase".to_string()),
        ..Default::default()
    };
    let error = tls_exchange(options, true).unwrap_err();
    let super::TransportError::Tls(message) = error else {
        panic!("expected a TLS error");
    };
    assert!(message.contains("could not decrypt"), "{message}");
}
