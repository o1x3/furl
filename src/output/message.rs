//! Rendering HTTP messages for display.

use crate::request::PreparedRequest;

/// Which request parts to include.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestParts {
    pub headers: bool,
    pub body: bool,
}

/// Render the request the way it would go on the wire: request line and
/// headers joined with CRLF, a blank line, then the body bytes verbatim.
/// A `Host` header is synthesized last when none was given.
pub fn render_request(request: &PreparedRequest, parts: RequestParts) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    if parts.headers {
        let mut head = format!(
            "{} {} HTTP/1.1\r\n",
            request.method,
            request.request_target()
        );
        for (name, value) in &request.headers.entries {
            head.push_str(name);
            head.push_str(": ");
            head.push_str(value);
            head.push_str("\r\n");
        }
        let has_host = request
            .headers
            .entries
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("host"));
        if !has_host && !request.headers.skip_host {
            head.push_str(&format!("Host: {}\r\n", request.host_netloc));
        }
        out.extend_from_slice(head.trim_end().as_bytes());
        out.extend_from_slice(b"\r\n\r\n");
    }
    if parts.body {
        if let Some(body) = &request.body {
            out.extend_from_slice(&body.bytes);
        }
    }
    out
}
