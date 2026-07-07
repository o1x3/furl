//! User-facing error rendering.

use crate::cli::options::OptionSpec;

pub const DOCS_URL: &str = "https://github.com/o1x3/furl";

/// Severity of a stderr log line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogLevel {
    Warning,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }
}

/// The standard stderr log block: a blank line, `program: level: message`,
/// then two blank lines. With a color, every non-empty line of the text is
/// wrapped in the escape individually (newlines stay uncolored).
pub fn log_block(program: &str, level: LogLevel, message: &str, color: Option<&str>) -> String {
    let text = format!("{program}: {}: {message}", level.label());
    let mut out = String::with_capacity(text.len() + 16);
    out.push('\n');
    match color {
        None => out.push_str(&text),
        Some(params) => {
            for (index, line) in text.split('\n').enumerate() {
                if index > 0 {
                    out.push('\n');
                }
                if line.is_empty() {
                    continue;
                }
                out.push_str("\x1b[");
                out.push_str(params);
                out.push('m');
                out.push_str(line);
                out.push_str("\x1b[0m");
            }
        }
    }
    out.push_str("\n\n\n");
    out
}

/// The OS error text for an errno (what `strerror` reports). The standard
/// library renders os errors as `{strerror text} (os error {n})`; this
/// peels the parenthetical back off.
pub fn os_error_text(errno: i32) -> String {
    let rendered = std::io::Error::from_raw_os_error(errno).to_string();
    let suffix = format!(" (os error {errno})");
    rendered
        .strip_suffix(&suffix)
        .map(str::to_string)
        .unwrap_or(rendered)
}

/// The exception-class name an OS error renders under (the conventional
/// errno taxonomy: broken pipes, refused/reset/aborted connections, …).
pub fn os_error_class(kind: std::io::ErrorKind) -> &'static str {
    use std::io::ErrorKind;
    match kind {
        ErrorKind::BrokenPipe => "BrokenPipeError",
        ErrorKind::ConnectionReset => "ConnectionResetError",
        ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        ErrorKind::TimedOut => "TimeoutError",
        ErrorKind::Interrupted => "InterruptedError",
        ErrorKind::PermissionDenied => "PermissionError",
        ErrorKind::NotFound => "FileNotFoundError",
        ErrorKind::AlreadyExists => "FileExistsError",
        _ => "OSError",
    }
}

/// The errno and text of an I/O error, with a broken-pipe fallback for
/// synthetic errors that carry no errno (`32` is `EPIPE` everywhere).
pub fn os_error_parts(error: &std::io::Error) -> (i32, String) {
    match error.raw_os_error() {
        Some(errno) => (errno, os_error_text(errno)),
        None if error.kind() == std::io::ErrorKind::BrokenPipe => (32, os_error_text(32)),
        None => (5, error.to_string()),
    }
}

/// Render bytes the way Python's `repr(bytes)` does: `b'…'`, with
/// `\t\n\r\\` and the quote escaped, printable ASCII shown literally, and
/// every other byte as `\xHH`. Quote choice matches Python: single quotes
/// unless the content has a `'` and no `"`.
pub fn py_bytes_repr(bytes: &[u8]) -> String {
    let quote = pick_quote(bytes.iter().copied());
    let mut out = String::with_capacity(bytes.len() + 3);
    out.push('b');
    out.push(quote);
    for &byte in bytes {
        push_repr_byte(&mut out, byte, quote);
    }
    out.push(quote);
    out
}

/// Render a string the way Python's `repr(str)` does for the ASCII range
/// (printable non-ASCII passes through; control bytes escape). Adequate
/// for header names, which are ASCII in practice.
pub fn py_str_repr(text: &str) -> String {
    let quote = pick_quote(text.bytes());
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(quote);
            }
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn pick_quote(mut bytes: impl Iterator<Item = u8>) -> char {
    let mut has_single = false;
    let mut has_double = false;
    for byte in bytes.by_ref() {
        if byte == b'\'' {
            has_single = true;
        } else if byte == b'"' {
            has_double = true;
        }
    }
    if has_single && !has_double { '"' } else { '\'' }
}

fn push_repr_byte(out: &mut String, byte: u8, quote: char) {
    match byte {
        b'\\' => out.push_str("\\\\"),
        b'\t' => out.push_str("\\t"),
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        b if b == quote as u8 => {
            out.push('\\');
            out.push(quote);
        }
        0x20..=0x7e => out.push(byte as char),
        b => out.push_str(&format!("\\x{b:02x}")),
    }
}

/// The three-block usage error: usage line, message, help pointer.
///
/// When the error came from a specific option, that option is shown in
/// the usage line before the positional grammar.
pub fn usage_error_block(program: &str, message: &str, option: Option<&OptionSpec>) -> String {
    let option_part = option
        .map(|spec| {
            let name = spec.long_alias().unwrap_or(spec.aliases[0]);
            match spec.choices {
                Some(choices) => format!("{name} {{{}}} ", choices.join(", ")),
                None => format!("{name} "),
            }
        })
        .unwrap_or_default();
    format!(
        "usage:\n    {program} {option_part}[METHOD] URL [REQUEST_ITEM ...]\n\
         \n\
         error:\n    {message}\n\
         \n\
         for more information:\n    run '{program} --help' or visit {DOCS_URL}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::find_exact;

    #[test]
    fn py_repr_matches_python() {
        assert_eq!(py_str_repr(""), "''");
        assert_eq!(py_str_repr(" X"), "' X'");
        assert_eq!(py_str_repr("X\rY"), "'X\\rY'");
        // A single quote with no double quote flips to double quotes.
        assert_eq!(py_str_repr("a'b"), "\"a'b\"");
        assert_eq!(py_str_repr("a\"b"), "'a\"b'");
        assert_eq!(py_str_repr("a'\"b"), "'a\\'\"b'");
        assert_eq!(py_bytes_repr(b"a\r\nEvil: 1"), "b'a\\r\\nEvil: 1'");
        assert_eq!(py_bytes_repr(b"a'b"), "b\"a'b\"");
        assert_eq!(py_bytes_repr(&[0x00, 0x7f, 0x80]), "b'\\x00\\x7f\\x80'");
    }

    #[test]
    fn log_block_frames_with_blank_lines() {
        let block = log_block("furl", LogLevel::Error, "boom", None);
        assert_eq!(block, "\nfurl: error: boom\n\n\n");
        let block = log_block("furl", LogLevel::Warning, "HTTP 404 Not Found", None);
        assert_eq!(block, "\nfurl: warning: HTTP 404 Not Found\n\n\n");
    }

    #[test]
    fn log_block_colors_each_line_separately() {
        let block = log_block("furl", LogLevel::Error, "first\nsecond", Some("31"));
        assert_eq!(
            block,
            "\n\x1b[31mfurl: error: first\x1b[0m\n\x1b[31msecond\x1b[0m\n\n\n"
        );
    }

    #[test]
    fn log_block_skips_escapes_on_empty_lines() {
        let block = log_block("furl", LogLevel::Error, "a\n\nb", Some("31"));
        assert_eq!(
            block,
            "\n\x1b[31mfurl: error: a\x1b[0m\n\n\x1b[31mb\x1b[0m\n\n\n"
        );
    }

    #[test]
    fn os_error_text_strips_the_os_error_suffix() {
        // EPIPE is 32 everywhere this builds.
        assert_eq!(os_error_text(32), "Broken pipe");
    }

    #[test]
    fn os_error_class_maps_the_common_kinds() {
        use std::io::ErrorKind;
        assert_eq!(os_error_class(ErrorKind::BrokenPipe), "BrokenPipeError");
        assert_eq!(
            os_error_class(ErrorKind::ConnectionRefused),
            "ConnectionRefusedError"
        );
        assert_eq!(os_error_class(ErrorKind::OutOfMemory), "OSError");
    }

    #[test]
    fn os_error_parts_default_to_broken_pipe_for_synthetic_pipe_errors() {
        let synthetic = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "gone");
        assert_eq!(os_error_parts(&synthetic), (32, "Broken pipe".to_string()));
    }

    #[test]
    fn plain_usage_block() {
        let block = usage_error_block("furl", "boom", None);
        assert_eq!(
            block,
            "usage:\n    furl [METHOD] URL [REQUEST_ITEM ...]\n\n\
             error:\n    boom\n\n\
             for more information:\n    run 'furl --help' or visit https://github.com/o1x3/furl\n"
        );
    }

    #[test]
    fn option_with_choices_in_usage_line() {
        let spec = find_exact("--pretty").unwrap();
        let block = usage_error_block("furl", "bad", Some(spec));
        assert!(
            block.contains("furl --pretty {all, colors, format, none} [METHOD] URL"),
            "got: {block}"
        );
    }

    #[test]
    fn option_without_choices_shows_bare_name() {
        let spec = find_exact("--max-redirects").unwrap();
        let block = usage_error_block("furl", "bad", Some(spec));
        assert!(
            block.contains("furl --max-redirects [METHOD] URL"),
            "got: {block}"
        );
    }
}
