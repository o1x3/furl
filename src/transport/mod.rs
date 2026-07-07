//! The blocking HTTP/1.1 transport.
//!
//! One request per connection, written byte-for-byte from the prepared
//! request; responses parse with the received header case, order, and
//! reason phrase intact (all three are user-visible in rendered output).

pub mod tls;

#[cfg(test)]
mod tests;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::request::PreparedRequest;

/// A parsed response with raw fidelity.
#[derive(Debug)]
pub struct RawResponse {
    /// `0.9` / `1.0` / `1.1` (requests are always HTTP/1.1).
    pub http_version: &'static str,
    pub status: u16,
    /// Reason phrase exactly as the server sent it (may be empty).
    pub reason: String,
    /// Headers in received order with received name case.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The body, transfer-decoded (chunked reassembled) but NOT
    /// content-decoded; see [`decoded_body`].
    pub body: Vec<u8>,
}

impl RawResponse {
    /// The effective value of a header (last occurrence), lossily decoded.
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
    }
}

#[derive(Debug)]
pub enum TransportError {
    /// Name resolution failed: the `getaddrinfo` code and its
    /// `gai_strerror` text.
    Dns { code: i32, text: String },
    /// Every TCP connect attempt failed: the last OS errno and its
    /// `strerror` text.
    ConnectFailed { errno: i32, text: String },
    /// The server closed the connection before sending any response bytes.
    ClosedWithoutResponse,
    /// An OS error tore the connection down mid-exchange.
    Aborted {
        errno: i32,
        kind: std::io::ErrorKind,
        text: String,
    },
    /// Connection-level problems with no OS errno behind them.
    Connection(String),
    /// The configured timeout elapsed.
    Timeout,
    /// TLS setup or handshake problems → `SSLError`.
    Tls(String),
    /// The server response could not be parsed.
    Protocol(String),
    /// More headers than `--max-headers` allows.
    TooManyHeaders(usize),
}

/// Connection-level options resolved from the CLI.
pub struct TransportOptions {
    /// `--timeout`; zero means no timeout.
    pub timeout: Option<Duration>,
    pub tls: tls::TlsOptions,
    /// `--max-headers`; zero means unlimited.
    pub max_headers: usize,
}

/// Send one request and read the full response.
pub fn send(
    request: &PreparedRequest,
    options: &TransportOptions,
) -> Result<RawResponse, TransportError> {
    let host = request
        .url
        .host_str()
        .expect("URL host validated at build time")
        .to_string();
    let port = request
        .url
        .port_or_known_default()
        .ok_or_else(|| TransportError::Connection("no port for URL scheme".to_string()))?;
    let https = request.url.scheme() == "https";

    let stream = connect(&host, port, options.timeout)?;
    let head = wire_head(request);

    if https {
        let mut stream = tls::wrap(stream, &host, &options.tls)?;
        write_request(&mut stream, &head, request)?;
        read_response(&mut stream, request, options)
    } else {
        let mut stream = stream;
        write_request(&mut stream, &head, request)?;
        read_response(&mut stream, request, options)
    }
}

/// `getaddrinfo` failure codes as the platform defines them.
pub const EAI_NONAME: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    8
} else {
    -2
};
pub const EAI_AGAIN: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    2
} else {
    -3
};
/// Non-recoverable resolver failure: the fallback code for texts the
/// mapping below does not recognize.
const EAI_FAIL: i32 = if cfg!(any(target_os = "macos", target_os = "ios")) {
    4
} else {
    -4
};

fn connect(host: &str, port: u16, timeout: Option<Duration>) -> Result<TcpStream, TransportError> {
    use std::net::ToSocketAddrs;
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| dns_failure(&error))?;
    let mut last_error = None;
    for address in addresses {
        let attempt = match timeout {
            Some(limit) => TcpStream::connect_timeout(&address, limit),
            None => TcpStream::connect(address),
        };
        match attempt {
            Ok(stream) => {
                stream
                    .set_read_timeout(timeout)
                    .and_then(|()| stream.set_write_timeout(timeout))
                    .map_err(|error| TransportError::Connection(error.to_string()))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(match last_error {
        Some(error) if error.kind() == std::io::ErrorKind::TimedOut => TransportError::Timeout,
        Some(error) => match error.raw_os_error() {
            Some(errno) => TransportError::ConnectFailed {
                errno,
                text: crate::errors::os_error_text(errno),
            },
            None => TransportError::Connection(error.to_string()),
        },
        // Resolution succeeded with zero usable addresses: report it the
        // way a name-not-known failure reports.
        None => TransportError::Dns {
            code: EAI_NONAME,
            text: "no addresses found".to_string(),
        },
    })
}

/// Classify a resolution failure. The standard library renders resolver
/// errors as `failed to lookup address information: {gai_strerror text}`;
/// the text keys the platform's EAI code (user-visible in the message).
fn dns_failure(error: &std::io::Error) -> TransportError {
    let rendered = error.to_string();
    let text = rendered
        .rsplit_once("failed to lookup address information: ")
        .map(|(_, tail)| tail)
        .unwrap_or(&rendered)
        .to_string();
    let code = match text.as_str() {
        // macOS and glibc spellings of not-known / try-again.
        "nodename nor servname provided, or not known" | "Name or service not known" => EAI_NONAME,
        "Temporary failure in name resolution" => EAI_AGAIN,
        "No address associated with nodename" | "No address associated with hostname" => {
            if cfg!(any(target_os = "macos", target_os = "ios")) {
                7
            } else {
                -5
            }
        }
        _ => EAI_FAIL,
    };
    TransportError::Dns { code, text }
}

/// The wire head: request line, then Host, then the prepared headers.
/// (Display order keeps Host last; the wire leads with it.)
fn wire_head(request: &PreparedRequest) -> Vec<u8> {
    let mut head = format!(
        "{} {} HTTP/1.1\r\n",
        request.method,
        request.request_target()
    );
    let explicit_host = request
        .headers
        .entries
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("host"));
    match explicit_host {
        Some((_, value)) => head.push_str(&format!("Host: {value}\r\n")),
        None if request.headers.skip_host => {}
        None => head.push_str(&format!("Host: {}\r\n", request.host_netloc)),
    }
    for (name, value) in &request.headers.entries {
        if name.eq_ignore_ascii_case("host") {
            continue;
        }
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    head.into_bytes()
}

fn write_request<S: Write>(
    stream: &mut S,
    head: &[u8],
    request: &PreparedRequest,
) -> Result<(), TransportError> {
    stream.write_all(head).map_err(stream_error)?;
    if let Some(body) = &request.body {
        if request.chunked {
            // The whole body as one chunk plus the terminator.
            if !body.bytes.is_empty() {
                stream
                    .write_all(format!("{:x}\r\n", body.bytes.len()).as_bytes())
                    .map_err(stream_error)?;
                stream.write_all(&body.bytes).map_err(stream_error)?;
                stream.write_all(b"\r\n").map_err(stream_error)?;
            }
            stream.write_all(b"0\r\n\r\n").map_err(stream_error)?;
        } else {
            stream.write_all(&body.bytes).map_err(stream_error)?;
        }
    } else if request.chunked {
        stream.write_all(b"0\r\n\r\n").map_err(stream_error)?;
    }
    stream.flush().map_err(stream_error)?;
    Ok(())
}

/// Read/write errors mid-exchange: timeouts stay timeouts; OS errors keep
/// their errno for the rendered message; the rest keep their text.
fn stream_error(error: std::io::Error) -> TransportError {
    if error.kind() == std::io::ErrorKind::TimedOut
        || error.kind() == std::io::ErrorKind::WouldBlock
    {
        return TransportError::Timeout;
    }
    match error.raw_os_error() {
        Some(errno) => TransportError::Aborted {
            errno,
            kind: error.kind(),
            text: crate::errors::os_error_text(errno),
        },
        None => TransportError::Connection(error.to_string()),
    }
}

const MAX_HEAD_BYTES: usize = 1024 * 1024;

fn read_response<S: Read>(
    stream: &mut S,
    request: &PreparedRequest,
    options: &TransportOptions,
) -> Result<RawResponse, TransportError> {
    let mut reader = BufReader::new(stream);
    loop {
        let head_bytes = read_head(&mut reader)?;
        let mut parsed = parse_head(&head_bytes, options)?;
        // Interim 1xx responses (e.g. 100 Continue) are skipped.
        if parsed.status / 100 == 1 && parsed.status != 101 {
            continue;
        }
        read_body(&mut reader, request, &mut parsed)?;
        return Ok(parsed);
    }
}

/// Read up to and including the `\r\n\r\n` head terminator.
fn read_head<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, TransportError> {
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        read_line(reader, &mut line)?;
        if line.is_empty() {
            return Err(TransportError::ClosedWithoutResponse);
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEAD_BYTES {
            return Err(TransportError::Protocol("response head too large".into()));
        }
        if line == b"\r\n" || line == b"\n" {
            // Blank line: end of head — unless it is the very first line.
            if head.len() == line.len() {
                head.clear();
                continue;
            }
            return Ok(head);
        }
    }
}

fn read_line<R: BufRead>(reader: &mut R, out: &mut Vec<u8>) -> Result<(), TransportError> {
    reader.read_until(b'\n', out).map_err(stream_error)?;
    Ok(())
}

fn parse_head(
    head_bytes: &[u8],
    options: &TransportOptions,
) -> Result<RawResponse, TransportError> {
    let mut header_storage = vec![httparse::EMPTY_HEADER; 1024];
    let mut response = httparse::Response::new(&mut header_storage);
    let parsed = httparse::ParserConfig::default()
        .allow_obsolete_multiline_headers_in_responses(true)
        .allow_spaces_after_header_name_in_responses(true)
        .parse_response(&mut response, head_bytes)
        .map_err(|error| TransportError::Protocol(error.to_string()))?;
    if parsed.is_partial() {
        return Err(TransportError::Protocol("truncated response head".into()));
    }
    let header_count = response.headers.len();
    if options.max_headers > 0 && header_count > options.max_headers {
        return Err(TransportError::TooManyHeaders(header_count));
    }
    let http_version = match response.version {
        Some(0) => "1.0",
        _ => "1.1",
    };
    Ok(RawResponse {
        http_version,
        status: response.code.unwrap_or(0),
        reason: response.reason.unwrap_or("").to_string(),
        headers: response
            .headers
            .iter()
            .map(|h| (h.name.to_string(), h.value.to_vec()))
            .collect(),
        body: Vec::new(),
    })
}

/// Read the body per the framing rules: no body for HEAD/204/304,
/// chunked, Content-Length, or read-to-close.
fn read_body<R: BufRead>(
    reader: &mut R,
    request: &PreparedRequest,
    response: &mut RawResponse,
) -> Result<(), TransportError> {
    if request.method == "HEAD" || matches!(response.status, 204 | 304) {
        return Ok(());
    }
    let transfer_encoding = response.header("Transfer-Encoding").unwrap_or_default();
    if transfer_encoding.to_ascii_lowercase().contains("chunked") {
        loop {
            let mut size_line = Vec::new();
            read_line(reader, &mut size_line)?;
            let size_text = String::from_utf8_lossy(&size_line);
            let size_text = size_text.trim().split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_text, 16)
                .map_err(|_| TransportError::Protocol("invalid chunk size".into()))?;
            if size == 0 {
                // Trailers (if any) up to the final blank line.
                loop {
                    let mut trailer = Vec::new();
                    read_line(reader, &mut trailer)?;
                    if trailer == b"\r\n" || trailer == b"\n" || trailer.is_empty() {
                        break;
                    }
                }
                return Ok(());
            }
            let mut chunk = vec![0u8; size];
            reader.read_exact(&mut chunk).map_err(stream_error)?;
            response.body.extend_from_slice(&chunk);
            let mut separator = Vec::new();
            read_line(reader, &mut separator)?;
        }
    }
    if let Some(length_text) = response.header("Content-Length") {
        let length: usize = length_text
            .trim()
            .parse()
            .map_err(|_| TransportError::Protocol("invalid Content-Length".into()))?;
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(stream_error)?;
        response.body = body;
        return Ok(());
    }
    // No framing: the body runs to connection close.
    reader
        .read_to_end(&mut response.body)
        .map_err(stream_error)?;
    Ok(())
}

/// Content-decode the body for display per `Content-Encoding`.
pub fn decoded_body(response: &RawResponse) -> Vec<u8> {
    let encoding = response
        .header("Content-Encoding")
        .unwrap_or_default()
        .to_ascii_lowercase();
    match encoding.trim() {
        "gzip" => {
            let mut out = Vec::new();
            let mut decoder = flate2::read::MultiGzDecoder::new(response.body.as_slice());
            if decoder.read_to_end(&mut out).is_ok() {
                out
            } else {
                response.body.clone()
            }
        }
        "deflate" => {
            // Try zlib-wrapped first, then raw deflate.
            let mut out = Vec::new();
            let mut zlib = flate2::read::ZlibDecoder::new(response.body.as_slice());
            if zlib.read_to_end(&mut out).is_ok() {
                return out;
            }
            let mut out = Vec::new();
            let mut raw = flate2::read::DeflateDecoder::new(response.body.as_slice());
            if raw.read_to_end(&mut out).is_ok() {
                out
            } else {
                response.body.clone()
            }
        }
        _ => response.body.clone(),
    }
}
