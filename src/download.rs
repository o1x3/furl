//! Download-mode filename derivation and file writing.

use std::path::{Path, PathBuf};

use crate::transport::RawResponse;

/// Derive the output filename when `--output` was not given.
///
/// Precedence: the response `Content-Disposition` filename, then the
/// initial URL's path basename (or `index`), then an extension guessed
/// from the final `Content-Type`. The candidate is uniquified against
/// existing files with `-1`, `-2`, … suffixes.
pub fn derive_filename(response: &RawResponse, url: &url::Url, directory: &Path) -> PathBuf {
    let mut name = response
        .header("Content-Disposition")
        .and_then(|value| filename_from_disposition(&value))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| url_basename(url));

    if !name.contains('.') {
        if let Some(extension) = guess_extension(response) {
            name.push_str(&extension);
        }
    }

    uniquify(directory, &name)
}

/// Extract and sanitize the `filename` parameter of a
/// `Content-Disposition` header.
fn filename_from_disposition(header: &str) -> Option<String> {
    let raw = extended_filename(header).or_else(|| plain_filename(header))?;
    // Strip any path components, leading dots, and surrounding space.
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&raw)
        .trim()
        .trim_start_matches('.')
        .trim();
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

/// `filename*=UTF-8''percent%20encoded` (RFC 5987/6266).
fn extended_filename(header: &str) -> Option<String> {
    let start = header.to_ascii_lowercase().find("filename*=")?;
    let value = header[start + "filename*=".len()..]
        .split(';')
        .next()?
        .trim();
    // Drop the charset''  prefix (e.g. `UTF-8''`).
    let encoded = value.rsplit_once("''").map(|(_, v)| v).unwrap_or(value);
    Some(percent_decode(encoded))
}

/// `filename=token` or `filename="quoted \" string"`.
fn plain_filename(header: &str) -> Option<String> {
    let lower = header.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let idx = lower[search_from..].find("filename")? + search_from;
        let after = &header[idx + "filename".len()..];
        // Skip the `filename*` extended form handled elsewhere.
        if after.starts_with('*') {
            search_from = idx + "filename".len();
            continue;
        }
        let after = after.trim_start();
        let after = after.strip_prefix('=')?.trim_start();
        return Some(if let Some(rest) = after.strip_prefix('"') {
            // Quoted string with backslash escapes.
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        if let Some(next) = chars.next() {
                            out.push(next);
                        }
                    }
                    other => out.push(other),
                }
            }
            out
        } else {
            after.split(';').next().unwrap_or(after).trim().to_string()
        });
    }
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn url_basename(url: &url::Url) -> String {
    let path = url.path().trim_end_matches('/');
    let base = path.rsplit('/').next().unwrap_or("");
    if base.is_empty() {
        "index".to_string()
    } else {
        base.to_string()
    }
}

/// Guess a file extension from the final response's Content-Type.
fn guess_extension(response: &RawResponse) -> Option<String> {
    let content_type = response.header("Content-Type")?;
    let mime = content_type.split(';').next()?.trim().to_ascii_lowercase();
    if mime.is_empty() {
        return None;
    }
    // text/plain resolves to .ksh in the mime table, so it is special-cased.
    if mime == "text/plain" {
        return Some(".txt".to_string());
    }
    let extension = mime_guess::get_mime_extensions_str(&mime)
        .and_then(|exts| exts.first())
        .map(|ext| format!(".{ext}"))?;
    // `.htm` is upgraded to `.html`.
    Some(if extension == ".htm" {
        ".html".to_string()
    } else {
        extension
    })
}

/// Append `-1`, `-2`, … until the path does not exist.
fn uniquify(directory: &Path, name: &str) -> PathBuf {
    let candidate = directory.join(name);
    if !candidate.exists() {
        return candidate;
    }
    for attempt in 1.. {
        let candidate = directory.join(format!("{name}-{attempt}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("an unused filename always exists eventually")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_with(headers: &[(&str, &str)]) -> RawResponse {
        RawResponse {
            http_version: "1.1",
            status: 200,
            reason: "OK".to_string(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.as_bytes().to_vec()))
                .collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn content_disposition_names() {
        let cases = [
            (
                "attachment; filename=hello-WORLD_123.txt",
                "hello-WORLD_123.txt",
            ),
            (
                "attachment; filename=\".hello-WORLD_123.txt\"",
                "hello-WORLD_123.txt",
            ),
            (
                "attachment; filename=\"white space.txt\"",
                "white space.txt",
            ),
            (
                "attachment; filename=\"\\\"quotes\\\".txt\"",
                "\"quotes\".txt",
            ),
            ("attachment; filename=/etc/hosts", "hosts"),
        ];
        for (header, expected) in cases {
            assert_eq!(
                filename_from_disposition(header).as_deref(),
                Some(expected),
                "header: {header}"
            );
        }
        assert_eq!(filename_from_disposition("attachment; filename="), None);
    }

    #[test]
    fn extended_filename_is_percent_decoded() {
        assert_eq!(
            filename_from_disposition("attachment; filename*=UTF-8''na%C3%AFve%20file.txt")
                .as_deref(),
            Some("naïve file.txt")
        );
    }

    #[test]
    fn url_basename_fallback() {
        let url = url::Url::parse("http://x/a/b/file.bin").unwrap();
        assert_eq!(url_basename(&url), "file.bin");
        let url = url::Url::parse("http://x/").unwrap();
        assert_eq!(url_basename(&url), "index");
        let url = url::Url::parse("http://x/dir/").unwrap();
        assert_eq!(url_basename(&url), "dir");
    }

    #[test]
    fn extension_guessing() {
        let url = url::Url::parse("http://x/foo").unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            derive_filename(
                &response_with(&[("Content-Type", "text/plain")]),
                &url,
                dir.path()
            )
            .file_name()
            .unwrap(),
            "foo.txt"
        );
        assert_eq!(
            derive_filename(
                &response_with(&[("Content-Type", "text/html; charset=UTF-8")]),
                &url,
                dir.path()
            )
            .file_name()
            .unwrap(),
            "foo.html"
        );
        assert_eq!(
            derive_filename(&response_with(&[]), &url, dir.path())
                .file_name()
                .unwrap(),
            "foo"
        );
    }

    #[test]
    fn uniquification_appends_suffix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f.txt"), b"x").unwrap();
        let response = response_with(&[("Content-Disposition", "attachment; filename=f.txt")]);
        let url = url::Url::parse("http://x/other").unwrap();
        assert_eq!(
            derive_filename(&response, &url, dir.path())
                .file_name()
                .unwrap(),
            "f.txt-1"
        );
    }
}
