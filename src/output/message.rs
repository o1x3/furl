//! Rendering HTTP messages for display.

use crate::request::PreparedRequest;
use crate::transport::RawResponse;

/// The response head: status line plus headers, received case and order.
///
/// Duplicate headers fold into one comma-joined line at the first
/// occurrence — except `Set-Cookie`, which renders one line per cookie,
/// pulled to the end of the block.
pub fn render_response_head(response: &RawResponse) -> String {
    let mut head = format!(
        "HTTP/{} {} {}",
        response.http_version, response.status, response.reason
    );
    let mut seen: Vec<&str> = Vec::new();
    let mut set_cookies: Vec<String> = Vec::new();
    for (name, value) in &response.headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            set_cookies.push(String::from_utf8_lossy(value).to_string());
            continue;
        }
        if seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            continue;
        }
        seen.push(name);
        let folded: Vec<String> = response
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| String::from_utf8_lossy(v).to_string())
            .collect();
        head.push_str("\r\n");
        head.push_str(name);
        head.push_str(": ");
        head.push_str(&folded.join(", "));
    }
    for cookie in set_cookies {
        head.push_str("\r\nSet-Cookie: ");
        head.push_str(&cookie);
    }
    head
}

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
