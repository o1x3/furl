//! Request body construction.

use crate::cli::items::{DataValue, MultipartEntry, RequestItems};
use crate::json;

use super::urlencode::urlencode;

/// A finished request body.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub bytes: Vec<u8>,
    /// Boundary used, when the body is multipart.
    pub boundary: Option<String>,
}

impl Body {
    fn plain(bytes: Vec<u8>) -> Body {
        Body {
            bytes,
            boundary: None,
        }
    }
}

#[derive(Debug)]
pub enum BodyError {
    File { message: String },
}

/// Serialize the pre-folded JSON-mode data body.
pub fn json_body(items: &RequestItems) -> Option<Body> {
    let value = items.json_data.as_ref()?;
    let text = json::dumps(value, &json::DumpOptions::default());
    Some(Body::plain(text.into_bytes()))
}

/// Serialize form data items as urlencoded pairs.
pub fn form_body(items: &RequestItems) -> Option<Body> {
    if items.data.is_empty() {
        return None;
    }
    let pairs: Vec<(String, String)> = items
        .data
        .iter()
        .map(|(key, value)| {
            let text = match value {
                DataValue::Text(text) => text.clone(),
                // Form mode stringifies typed values during item
                // processing; a structured value cannot reach here.
                DataValue::Json(value) => value.to_string(),
            };
            (key.clone(), text)
        })
        .collect();
    Some(Body::plain(urlencode(&pairs).into_bytes()))
}

/// Frame the multipart body from the ordered part sequence.
pub fn multipart_body(items: &RequestItems, boundary: String) -> Result<Body, BodyError> {
    let mut bytes: Vec<u8> = Vec::new();
    for entry in &items.multipart_sequence {
        bytes.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match entry {
            MultipartEntry::Text { name, value } => {
                bytes.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                bytes.extend_from_slice(value.as_bytes());
            }
            MultipartEntry::File(field) => {
                let filename = field
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                bytes.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{}\"; filename=\"{filename}\"\r\n",
                        field.name
                    )
                    .as_bytes(),
                );
                let mime = field.mime.clone().or_else(|| guess_mime(&field.path));
                if let Some(mime) = mime {
                    bytes.extend_from_slice(format!("Content-Type: {mime}\r\n").as_bytes());
                }
                bytes.extend_from_slice(b"\r\n");
                let contents = std::fs::read(&field.path).map_err(|error| BodyError::File {
                    message: format!("{}: {error}", field.path.display()),
                })?;
                bytes.extend_from_slice(&contents);
            }
        }
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(Body {
        bytes,
        boundary: Some(boundary),
    })
}

/// A random 32-character lowercase-hex multipart boundary.
pub fn random_boundary() -> String {
    // Hash process-unique state through the standard hasher; no fixed
    // seed, no cryptographic requirement — collisions only need to be
    // unlikely within one request body.
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::with_capacity(32);
    while out.len() < 32 {
        let mut hasher = std::hash::RandomState::new().build_hasher();
        hasher.write_u64(out.len() as u64);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.truncate(32);
    out
}

pub fn guess_mime(path: &std::path::Path) -> Option<String> {
    mime_guess::from_path(path)
        .first()
        .map(|m| m.essence_str().to_string())
}

/// Compress bytes in zlib format (RFC 1950) — the correct wire meaning of
/// HTTP `Content-Encoding: deflate`.
pub fn zlib_compress(bytes: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("in-memory zlib write");
    encoder.finish().expect("in-memory zlib finish")
}
