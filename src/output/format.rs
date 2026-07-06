//! The `format` group of the output pipeline: reindenting and sorting the
//! decoded message *text* before it is (optionally) colorized.
//!
//! This module deliberately excludes the `colors` group; coloring is a
//! separate stage that runs after formatting. What lives here is the
//! reference tool's "prettify without ANSI" behavior: parse the format
//! options, decide whether the format group is active for this invocation,
//! and reformat JSON bodies, XML bodies, and the response header block.
//!
//! The guiding principle throughout is *fidelity over cleverness*: when a
//! body does not parse (invalid JSON, malformed or unsafe XML), it is left
//! byte-for-byte unchanged and silently passed through, exactly as the
//! reference tool does. Reformatting must never corrupt a body it cannot
//! fully understand.

use crate::json::{DumpOptions, dumps, parse};

/// The six built-in format options, already merged with their defaults.
///
/// The reference tool exposes these as a nested `section.key` namespace;
/// we flatten them into named fields because the set is fixed and small.
/// The defaults here mirror spec §4.2 exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOptions {
    /// Sort response header lines alphabetically (case-sensitively, per the
    /// reference) after the status line.
    pub headers_sort: bool,
    /// Reindent JSON bodies. When false the body is passed through as-is —
    /// no reindent and no key sort (the reference skips the whole JSON
    /// formatter, see §5.1).
    pub json_format: bool,
    /// Spaces of indentation per level when reindenting JSON.
    pub json_indent: usize,
    /// Sort JSON object keys by name (stable for duplicates).
    pub json_sort_keys: bool,
    /// Reindent XML bodies.
    pub xml_format: bool,
    /// Spaces of indentation per level when reindenting XML.
    pub xml_indent: usize,
}

impl Default for FormatOptions {
    fn default() -> Self {
        // Spec §4.2 defaults table.
        FormatOptions {
            headers_sort: true,
            json_format: true,
            json_indent: 4,
            json_sort_keys: true,
            xml_format: true,
            xml_indent: 2,
        }
    }
}

/// The declared type of a format option, used both to parse values and to
/// produce the reference tool's type-mismatch error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptType {
    Bool,
    Int,
}

impl OptType {
    /// The type name as it appears in `expected <t> got <t>` errors.
    fn name(self) -> &'static str {
        match self {
            OptType::Bool => "bool",
            OptType::Int => "int",
        }
    }
}

/// A parsed value from a `section.key:value` entry, before it is validated
/// against the option's declared type.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OptValue {
    Bool(bool),
    Int(i64),
    /// Any value that is neither `true`/`false` nor all-digits.
    Str(String),
}

impl OptValue {
    /// The type name for the value as parsed, for the `got <t>` half of a
    /// mismatch error. A bare string carries no numeric/bool type, so it is
    /// reported as `str` (matching the reference's Python `str`).
    fn type_name(&self) -> &'static str {
        match self {
            OptValue::Bool(_) => "bool",
            OptValue::Int(_) => "int",
            OptValue::Str(_) => "str",
        }
    }
}

impl FormatOptions {
    /// Parse and merge every `--format-options` occurrence in command-line
    /// order onto the defaults, later values overriding earlier ones per
    /// key (spec §4.1). Each occurrence is a comma-separated list of
    /// `section.key:value` entries; both keys and values are lowercased
    /// before parsing.
    ///
    /// Errors are the argparse-shaped fragments the reference emits; the
    /// caller is responsible for any `<prog>: error: ` prefixing.
    pub fn from_occurrences(occurrences: &[String]) -> Result<FormatOptions, String> {
        let mut options = FormatOptions::default();
        for occurrence in occurrences {
            for entry in occurrence.split(',') {
                options.apply_entry(entry)?;
            }
        }
        Ok(options)
    }

    /// Apply one `section.key:value` entry, mutating the relevant field.
    ///
    /// Splitting order matters: the reference validates the `:` split
    /// first (malformed entry → `invalid option`), then requires the key
    /// to contain a `.`, then looks the key up before coercing the value.
    fn apply_entry(&mut self, entry: &str) -> Result<(), String> {
        // Lowercase the whole entry: `JSON.Indent:4` == `json.indent:4`,
        // and string values are forced lowercase too (§4.1).
        let entry = entry.to_lowercase();

        // A well-formed entry has exactly one `:` separating key from value,
        // and the key itself carries a `.`. Missing either → `invalid
        // option '<entry>'` (§4.1). We split on the first `:` because a
        // value could conceivably contain one.
        let (key, value) = match entry.split_once(':') {
            Some(pair) => pair,
            None => return Err(format!("invalid option '{entry}'")),
        };
        if !key.contains('.') {
            return Err(format!("invalid option '{entry}'"));
        }

        // Value parsing precedes validation: `true`/`false` → bool,
        // all-digits → int, everything else → string (§4.1).
        let parsed = parse_value(value);

        // Look the key up in the fixed set; unknown → `invalid key`.
        let declared = declared_type(key).ok_or_else(|| format!("invalid key '{key}'"))?;

        // A value whose parsed type differs from the declared type is a
        // hard error, quoting the *original* option text (§4.1). Note that
        // an all-digits value parsed as int is a valid bool source? No —
        // the reference keeps the parsed type strict, so `json.format:1`
        // is int-vs-bool mismatch, not truthiness.
        match (declared, parsed) {
            (OptType::Bool, OptValue::Bool(b)) => self.set_bool(key, b),
            (OptType::Int, OptValue::Int(n)) => self.set_int(key, n),
            (_, other) => {
                return Err(format!(
                    "invalid value '{value}' in '{entry}' (expected {} got {})",
                    declared.name(),
                    other.type_name(),
                ));
            }
        }
        Ok(())
    }

    /// Assign a bool-typed option by key. The key is known-valid here.
    fn set_bool(&mut self, key: &str, value: bool) {
        match key {
            "headers.sort" => self.headers_sort = value,
            "json.format" => self.json_format = value,
            "json.sort_keys" => self.json_sort_keys = value,
            "xml.format" => self.xml_format = value,
            other => unreachable!("{other} is not a bool option"),
        }
    }

    /// Assign an int-typed option by key. The key is known-valid here.
    fn set_int(&mut self, key: &str, value: i64) {
        // Indentation cannot be negative; clamp to zero rather than error,
        // since the reference's Python `int` would accept a negative and
        // the serializer would treat it as no indent. Zero is the safe
        // floor for a repeat count.
        let value = value.max(0) as usize;
        match key {
            "json.indent" => self.json_indent = value,
            "xml.indent" => self.xml_indent = value,
            other => unreachable!("{other} is not an int option"),
        }
    }
}

/// Parse a raw (already-lowercased) value string into a typed value.
fn parse_value(value: &str) -> OptValue {
    match value {
        "true" => OptValue::Bool(true),
        "false" => OptValue::Bool(false),
        // All-digits (and non-empty) → integer. A leading `-` is *not*
        // treated as an int here to match the reference's `isdigit()`
        // check; a negative literal falls through to the string branch and
        // would fail type validation, which is acceptable.
        digits if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
            // Parse cannot realistically overflow for indent values, but be
            // safe: on overflow, fall back to a string so validation fails
            // rather than silently wrapping.
            match digits.parse::<i64>() {
                Ok(n) => OptValue::Int(n),
                Err(_) => OptValue::Str(value.to_string()),
            }
        }
        other => OptValue::Str(other.to_string()),
    }
}

/// The declared type of a `section.key`, or `None` if the key is unknown.
fn declared_type(key: &str) -> Option<OptType> {
    match key {
        "headers.sort" => Some(OptType::Bool),
        "json.format" => Some(OptType::Bool),
        "json.indent" => Some(OptType::Int),
        "json.sort_keys" => Some(OptType::Bool),
        "xml.format" => Some(OptType::Bool),
        "xml.indent" => Some(OptType::Int),
        _ => None,
    }
}

/// The resolved `--pretty` mode for an invocation.
///
/// Each maps to a set of processor groups (§3.1): `all` → format+colors,
/// `colors` → colors only, `format` → format only, `none` → nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrettyMode {
    All,
    Colors,
    Format,
    None,
}

impl PrettyMode {
    /// Resolve the mode from the parsed `--pretty` value and whether the
    /// effective stdout is a tty. An explicit value always wins; with no
    /// value the default is `all` on a tty and `none` otherwise (§3.1).
    ///
    /// An unrecognized explicit value falls back to `none`. In practice the
    /// CLI layer validates `--pretty` choices before this point, so this
    /// branch is defensive only.
    pub fn resolve(pretty: Option<&str>, stdout_tty: bool) -> PrettyMode {
        match pretty {
            Some("all") => PrettyMode::All,
            Some("colors") => PrettyMode::Colors,
            Some("format") => PrettyMode::Format,
            Some("none") => PrettyMode::None,
            Some(_) => PrettyMode::None,
            None => {
                if stdout_tty {
                    PrettyMode::All
                } else {
                    PrettyMode::None
                }
            }
        }
    }
}

/// Is the `format` processor group active for this mode? True for `all`
/// and `format`; format options only take effect when this is true (§4.3).
pub fn format_group_active(mode: PrettyMode) -> bool {
    matches!(mode, PrettyMode::All | PrettyMode::Format)
}

/// Reduce a raw `Content-Type` (or `--response-mime` override) to the bare
/// `type/subtype` media type used for format eligibility: parameters after
/// the first `;` are dropped and surrounding whitespace trimmed, then
/// lowercased for substring matching.
///
/// A caller that already has the override should pass it directly; this is
/// the param-stripping half of the "effective MIME" computation (§5).
pub fn effective_mime(
    content_type: Option<&str>,
    response_mime_override: Option<&str>,
) -> Option<String> {
    // The override, when present, replaces the media type wholesale (§5.6);
    // it is validated to look like `type/subtype` at the CLI layer.
    let raw = response_mime_override.or(content_type)?;
    let media = raw.split(';').next().unwrap_or(raw).trim().to_lowercase();
    if media.is_empty() { None } else { Some(media) }
}

/// Does a media type look like a syntactically valid `type/subtype`? Body
/// formatters only run against valid mimes (§5); otherwise the body passes
/// through untouched. This mirrors the reference's `^[^/]+/[^/]+$` check.
fn is_valid_mime(mime: &str) -> bool {
    match mime.split_once('/') {
        Some((t, s)) => !t.is_empty() && !s.contains('/') && !s.is_empty(),
        None => false,
    }
}

/// Run the active `format`-group body formatters over a decoded body:
/// JSON first, then XML (spec §5 order; header sorting is a separate
/// stage handled by [`sort_header_lines`], not here).
///
/// Returns the reformatted text, or the original body unchanged when the
/// format group is inactive, the mime is not a valid `type/subtype`, or no
/// formatter applies / succeeds.
pub fn format_body(
    body_text: &str,
    effective_mime: Option<&str>,
    explicit_json: bool,
    opts: &FormatOptions,
    mode: PrettyMode,
) -> String {
    if !format_group_active(mode) {
        return body_text.to_string();
    }

    // Body formatters only run when the mime is a valid `type/subtype`,
    // *except* that explicit `--json` forces JSON eligibility regardless of
    // (or in the absence of) a Content-Type. A present-but-invalid mime
    // still gates the non-explicit path off.
    let mime_valid = effective_mime.map(is_valid_mime).unwrap_or(false);
    if !explicit_json && !mime_valid {
        return body_text.to_string();
    }

    // JSON stage: reformats the body when eligible; on any miss it returns
    // the input unchanged, so chaining into XML is safe.
    let after_json = format_json(body_text, effective_mime, explicit_json, opts);

    // XML stage runs on the (possibly JSON-reformatted) text. A body that
    // JSON-parsed will not have `xml` in its mime, so this no-ops there.
    format_xml(&after_json, effective_mime, opts)
}

/// JSON body reformatting (spec §5.1).
///
/// Eligibility: explicit `--json`, or the effective mime *contains* any of
/// `json`, `javascript`, `text`. When `json.format` is false the formatter
/// is skipped entirely (body unchanged) — false disables reindent *and*
/// key sorting, since the reference bypasses the whole formatter.
///
/// Parse strategy: try the whole body; on failure, strip a leading run of
/// non-`{["` characters (an XSSI/anti-hijacking prefix) and re-parse the
/// remainder. If it still fails, the body is returned unchanged, silently.
/// A stripped prefix is re-attached verbatim before the formatted JSON.
fn format_json(
    body_text: &str,
    effective_mime: Option<&str>,
    explicit_json: bool,
    opts: &FormatOptions,
) -> String {
    if !opts.json_format {
        return body_text.to_string();
    }

    let eligible = explicit_json
        || effective_mime
            .map(|m| m.contains("json") || m.contains("javascript") || m.contains("text"))
            .unwrap_or(false);
    if !eligible {
        return body_text.to_string();
    }

    // Serialize with the resolved indent/sort. `ensure_ascii` is false:
    // formatted *display* output keeps unicode literal (§5.1 "no
    // ASCII-escaping"), unlike the compact wire serializer's default.
    let dump = DumpOptions {
        indent: Some(opts.json_indent),
        sort_keys: opts.json_sort_keys,
        ensure_ascii: false,
    };

    // Fast path: the whole body is JSON.
    if let Ok(value) = parse(body_text) {
        return dumps(&value, &dump);
    }

    // Slow path: peel a leading XSSI-style prefix — the run of characters
    // that are not `{`, `[`, or `"` — and re-parse the remainder. The
    // prefix is prepended back verbatim on success.
    let prefix_len = body_text
        .char_indices()
        .take_while(|&(_, c)| c != '{' && c != '[' && c != '"')
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    if prefix_len > 0 && prefix_len < body_text.len() {
        let (prefix, remainder) = body_text.split_at(prefix_len);
        if let Ok(value) = parse(remainder) {
            return format!("{prefix}{}", dumps(&value, &dump));
        }
    }

    // Neither parse succeeded: leave the body completely unchanged.
    body_text.to_string()
}

/// Sort the header lines of an already-rendered response head (spec §5.3).
///
/// The first line (the status line) is left untouched; the remaining lines
/// are sorted by the header name — the text before the first `:` — in a
/// stable sort so equal names keep their relative order (this preserves the
/// per-cookie ordering `render_response_head` already established). Line
/// endings stay `\r\n`.
///
/// The reference sorts case-*sensitively* (uppercase before lowercase);
/// we match that with a plain byte comparison of the name.
pub fn sort_header_lines(head: &str, opts: &FormatOptions) -> String {
    if !opts.headers_sort {
        return head.to_string();
    }

    // The block is CRLF-joined; split on `\r\n` so we can rejoin identically.
    let mut lines: Vec<&str> = head.split("\r\n").collect();
    if lines.len() <= 1 {
        return head.to_string();
    }

    let status = lines.remove(0);
    // Stable sort by the name portion (before the first `:`), case-sensitive
    // as the reference does. A line with no colon sorts by its whole text.
    lines.sort_by(|a, b| header_name(a).cmp(header_name(b)));

    let mut out = String::with_capacity(head.len());
    out.push_str(status);
    for line in lines {
        out.push_str("\r\n");
        out.push_str(line);
    }
    out
}

/// The header name of a rendered line: everything before the first `:`.
fn header_name(line: &str) -> &str {
    match line.split_once(':') {
        Some((name, _)) => name,
        None => line,
    }
}

/// XML body reformatting (spec §5.2).
///
/// Eligibility: the effective mime *contains* `xml` and `xml.format` is
/// true. Reindents with `xml.indent` spaces per level. If the body is not
/// well-formed XML, or contains a DTD/DOCTYPE or general entity references
/// (the entity-expansion / XXE danger surface), it is returned completely
/// unchanged — we never expand entities or fetch external resources, and
/// we prefer leaving a body untouched over risking a lossy round-trip.
///
/// The XML declaration is preserved: if the original body (after leading
/// whitespace) begins with `<?xml … ?>`, that exact declaration is kept as
/// the first line; a declaration-free body stays declaration-free.
fn format_xml(body_text: &str, effective_mime: Option<&str>, opts: &FormatOptions) -> String {
    if !opts.xml_format {
        return body_text.to_string();
    }
    let is_xml = effective_mime.map(|m| m.contains("xml")).unwrap_or(false);
    if !is_xml {
        return body_text.to_string();
    }

    match reindent_xml(body_text, opts.xml_indent) {
        Some(formatted) => formatted,
        // Malformed or unsafe XML: leave the body exactly as received.
        None => body_text.to_string(),
    }
}

/// Reindent XML text, returning `None` (leave unchanged) for anything that
/// is malformed or that we deliberately refuse to touch for safety.
///
/// Safety stance (spec §5.2, §11.5): we treat any DOCTYPE/DTD or general
/// entity reference as a signal to bail out entirely. quick-xml never
/// expands entities or fetches external DTDs on its own, so bailing is not
/// strictly required to be *safe*, but it keeps us from silently dropping
/// or altering constructs (entity bombs, external-entity payloads) whose
/// intended rendering we cannot reproduce faithfully.
fn reindent_xml(body_text: &str, indent: usize) -> Option<String> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    use quick_xml::writer::Writer;

    let mut reader = Reader::from_str(body_text);
    // Trim surrounding whitespace of text nodes so the writer can lay out
    // fresh indentation; keep tag-name checks on so malformed nesting is a
    // parse error (→ leave unchanged) rather than being silently accepted.
    let config = reader.config_mut();
    config.trim_text(true);

    let mut writer = Writer::new_with_indent(Vec::new(), b' ', indent);
    // Track whether the original body carried an `<?xml ?>` declaration so
    // we can preserve exactly its text and skip re-emitting a generated one.
    let mut original_decl: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            // A declaration is not reindented; capture it and emit later as
            // the first line so its attributes (version/encoding/standalone)
            // survive verbatim.
            Ok(Event::Decl(decl)) => {
                let bytes = decl.as_ref();
                let text = std::str::from_utf8(bytes).ok()?;
                original_decl = Some(format!("<?{text}?>"));
            }
            // DTDs and general entity references are our safety tripwire:
            // refuse to reformat rather than risk mangling entity semantics.
            Ok(Event::DocType(_)) | Ok(Event::GeneralRef(_)) => return None,
            Ok(event) => {
                // Any write failure (should not happen writing to a Vec)
                // means we cannot produce faithful output → leave unchanged.
                writer.write_event(event).ok()?;
            }
            // Any parse error → not well-formed → leave the body unchanged.
            Err(_) => return None,
        }
    }

    let body = String::from_utf8(writer.into_inner()).ok()?;
    let body = body.trim();
    if body.is_empty() {
        // Nothing element-like was written (e.g. text-only input): treat as
        // not-XML and leave the original untouched.
        return None;
    }

    // Preserve the original declaration verbatim on its own first line, but
    // only if the source actually started with one (§5.2).
    let declared_in_source = body_text.trim_start().starts_with("<?xml");
    match (declared_in_source, original_decl) {
        (true, Some(decl)) => Some(format!("{decl}\n{body}")),
        _ => Some(body.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Format-options parsing, merging, and validation (§4.1) ---

    #[test]
    fn defaults_match_spec_table() {
        let opts = FormatOptions::default();
        assert!(opts.headers_sort);
        assert!(opts.json_format);
        assert_eq!(opts.json_indent, 4);
        assert!(opts.json_sort_keys);
        assert!(opts.xml_format);
        assert_eq!(opts.xml_indent, 2);
    }

    #[test]
    fn no_occurrences_yields_defaults() {
        let opts = FormatOptions::from_occurrences(&[]).unwrap();
        assert_eq!(opts, FormatOptions::default());
    }

    #[test]
    fn single_bool_override() {
        let opts = FormatOptions::from_occurrences(&["headers.sort:false".to_string()]).unwrap();
        assert!(!opts.headers_sort);
        // Everything else stays default.
        assert!(opts.json_format);
    }

    #[test]
    fn single_int_override() {
        let opts = FormatOptions::from_occurrences(&["json.indent:2".to_string()]).unwrap();
        assert_eq!(opts.json_indent, 2);
    }

    #[test]
    fn comma_separated_entries_in_one_occurrence() {
        let opts = FormatOptions::from_occurrences(&[
            "headers.sort:false,json.sort_keys:false".to_string()
        ])
        .unwrap();
        assert!(!opts.headers_sort);
        assert!(!opts.json_sort_keys);
    }

    #[test]
    fn later_occurrence_wins_per_key() {
        // Mirrors `--format-options=headers.sort:true --unsorted
        // --format-options=headers.sort:true` netting headers.sort=true but
        // json.sort_keys=false (positional, per-key merge).
        let opts = FormatOptions::from_occurrences(&[
            "headers.sort:true".to_string(),
            "headers.sort:false,json.sort_keys:false".to_string(),
            "headers.sort:true".to_string(),
        ])
        .unwrap();
        assert!(opts.headers_sort);
        assert!(!opts.json_sort_keys);
    }

    #[test]
    fn keys_and_values_are_lowercased() {
        let opts = FormatOptions::from_occurrences(&["JSON.Indent:2".to_string()]).unwrap();
        assert_eq!(opts.json_indent, 2);
        let opts = FormatOptions::from_occurrences(&["Headers.Sort:FALSE".to_string()]).unwrap();
        assert!(!opts.headers_sort);
    }

    #[test]
    fn unknown_key_errors() {
        let err = FormatOptions::from_occurrences(&["json.nope:1".to_string()]).unwrap_err();
        assert_eq!(err, "invalid key 'json.nope'");
    }

    #[test]
    fn type_mismatch_int_expected_got_bool() {
        let err = FormatOptions::from_occurrences(&["json.indent:false".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "invalid value 'false' in 'json.indent:false' (expected int got bool)"
        );
    }

    #[test]
    fn type_mismatch_bool_expected_got_int() {
        let err = FormatOptions::from_occurrences(&["json.format:2".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "invalid value '2' in 'json.format:2' (expected bool got int)"
        );
    }

    #[test]
    fn type_mismatch_reports_str() {
        let err = FormatOptions::from_occurrences(&["json.indent:wide".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "invalid value 'wide' in 'json.indent:wide' (expected int got str)"
        );
    }

    #[test]
    fn malformed_no_colon_errors() {
        let err = FormatOptions::from_occurrences(&["json.indent".to_string()]).unwrap_err();
        assert_eq!(err, "invalid option 'json.indent'");
    }

    #[test]
    fn malformed_no_dot_errors() {
        // A key without a `.` is malformed even though it has a colon.
        let err = FormatOptions::from_occurrences(&["indent:2".to_string()]).unwrap_err();
        assert_eq!(err, "invalid option 'indent:2'");
    }

    #[test]
    fn unsorted_shortcut_expansion_parses() {
        // The `--unsorted` shortcut appends this literal occurrence.
        let opts = FormatOptions::from_occurrences(&[
            "headers.sort:false,json.sort_keys:false".to_string()
        ])
        .unwrap();
        assert!(!opts.headers_sort);
        assert!(!opts.json_sort_keys);
    }

    // --- JSON reindent (§5.1) ---

    fn opts_json(indent: usize, sort: bool, format: bool) -> FormatOptions {
        FormatOptions {
            json_indent: indent,
            json_sort_keys: sort,
            json_format: format,
            ..FormatOptions::default()
        }
    }

    #[test]
    fn json_reindent_sorted_indent4() {
        let out = format_body(
            r#"{"b":1,"a":2}"#,
            Some("application/json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n    \"a\": 2,\n    \"b\": 1\n}");
    }

    #[test]
    fn json_reindent_unsorted_indent2() {
        let out = format_body(
            r#"{"b":1,"a":2}"#,
            Some("application/json"),
            false,
            &opts_json(2, false, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n  \"b\": 1,\n  \"a\": 2\n}");
    }

    #[test]
    fn json_format_false_leaves_body_unchanged() {
        // json.format:false skips the formatter entirely — no reindent and
        // no key sort even though sort_keys is still true.
        let out = format_body(
            r#"{"b":1,"a":2}"#,
            Some("application/json"),
            false,
            &opts_json(4, true, false),
            PrettyMode::All,
        );
        assert_eq!(out, r#"{"b":1,"a":2}"#);
    }

    #[test]
    fn json_eligible_via_text_mime() {
        // `text/plain` contains "text" → JSON parse is attempted.
        let out = format_body(
            r#"{"a":1}"#,
            Some("text/plain"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n    \"a\": 1\n}");
    }

    #[test]
    fn json_eligible_via_explicit_json_no_mime() {
        // `--json` forces eligibility even with no Content-Type at all.
        let out = format_body(
            r#"{"a":1}"#,
            None,
            true,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n    \"a\": 1\n}");
    }

    #[test]
    fn json_not_eligible_leaves_unchanged() {
        // A mime with none of json/javascript/text and no `--json`.
        let out = format_body(
            r#"{"a":1}"#,
            Some("application/octet-stream"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, r#"{"a":1}"#);
    }

    #[test]
    fn json_top_level_scalar_reformats() {
        let out = format_body(
            "  true  ",
            Some("application/json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "true");
    }

    #[test]
    fn json_xssi_prefix_stripped_and_reattached() {
        // The leading anti-hijacking prefix is peeled off, the remainder
        // parsed and reformatted, and the prefix prepended verbatim.
        let out = format_body(
            ")]}',\n{\"b\":1,\"a\":2}",
            Some("application/json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, ")]}',\n{\n    \"a\": 2,\n    \"b\": 1\n}");
    }

    #[test]
    fn json_invalid_passes_through_unchanged() {
        let out = format_body(
            "{not valid json",
            Some("application/json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{not valid json");
    }

    #[test]
    fn json_unicode_preserved_not_ascii_escaped() {
        // ensure_ascii=false → the é and 世界 stay literal in output.
        let out = format_body(
            r#"{"msg":"café 世界"}"#,
            Some("application/json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n    \"msg\": \"café 世界\"\n}");
    }

    #[test]
    fn json_vendor_suffix_mime_eligible() {
        // `application/vnd.api+json` contains "json".
        let out = format_body(
            r#"{"a":1}"#,
            Some("application/vnd.api+json"),
            false,
            &opts_json(4, true, true),
            PrettyMode::All,
        );
        assert_eq!(out, "{\n    \"a\": 1\n}");
    }

    #[test]
    fn json_skipped_when_format_group_inactive() {
        // Under colors/none the format group is off; body untouched.
        for mode in [PrettyMode::Colors, PrettyMode::None] {
            let out = format_body(
                r#"{"b":1,"a":2}"#,
                Some("application/json"),
                false,
                &opts_json(4, true, true),
                mode,
            );
            assert_eq!(out, r#"{"b":1,"a":2}"#, "mode {mode:?}");
        }
    }

    // --- Mode resolution (§3.1) & format group (§4.3) ---

    #[test]
    fn mode_default_tty_is_all() {
        assert_eq!(PrettyMode::resolve(None, true), PrettyMode::All);
    }

    #[test]
    fn mode_default_non_tty_is_none() {
        assert_eq!(PrettyMode::resolve(None, false), PrettyMode::None);
    }

    #[test]
    fn mode_explicit_wins_over_tty() {
        assert_eq!(PrettyMode::resolve(Some("none"), true), PrettyMode::None);
        assert_eq!(PrettyMode::resolve(Some("all"), false), PrettyMode::All);
        assert_eq!(
            PrettyMode::resolve(Some("format"), false),
            PrettyMode::Format
        );
        assert_eq!(
            PrettyMode::resolve(Some("colors"), true),
            PrettyMode::Colors
        );
    }

    #[test]
    fn format_group_active_for_all_and_format_only() {
        assert!(format_group_active(PrettyMode::All));
        assert!(format_group_active(PrettyMode::Format));
        assert!(!format_group_active(PrettyMode::Colors));
        assert!(!format_group_active(PrettyMode::None));
    }

    // --- effective_mime helper ---

    #[test]
    fn effective_mime_strips_params_and_lowercases() {
        assert_eq!(
            effective_mime(Some("Application/JSON; charset=utf-8"), None).as_deref(),
            Some("application/json")
        );
    }

    #[test]
    fn effective_mime_override_replaces_content_type() {
        assert_eq!(
            effective_mime(Some("text/plain"), Some("application/xml")).as_deref(),
            Some("application/xml")
        );
    }

    #[test]
    fn effective_mime_none_when_absent() {
        assert_eq!(effective_mime(None, None), None);
    }

    // --- Header sorting (§5.3) ---

    #[test]
    fn header_sort_keeps_status_line_first() {
        let head = "HTTP/1.1 200 OK\r\nZZZ: foo\r\nXXX: foo";
        let out = sort_header_lines(head, &FormatOptions::default());
        assert_eq!(out, "HTTP/1.1 200 OK\r\nXXX: foo\r\nZZZ: foo");
    }

    #[test]
    fn header_sort_disabled_preserves_order() {
        let head = "HTTP/1.1 200 OK\r\nZZZ: foo\r\nXXX: foo";
        let opts = FormatOptions {
            headers_sort: false,
            ..FormatOptions::default()
        };
        let out = sort_header_lines(head, &opts);
        assert_eq!(out, head);
    }

    #[test]
    fn header_sort_is_stable_for_duplicates() {
        // Two headers with the same name keep their relative order (this is
        // what preserves per-cookie ordering from render_response_head).
        let head = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nAccept: x";
        let out = sort_header_lines(head, &FormatOptions::default());
        assert_eq!(
            out,
            "HTTP/1.1 200 OK\r\nAccept: x\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2"
        );
    }

    #[test]
    fn header_sort_case_sensitive_uppercase_first() {
        // Case-sensitive sort: uppercase names sort before lowercase.
        let head = "HTTP/1.1 200 OK\r\naccept: x\r\nAccept: y";
        let out = sort_header_lines(head, &FormatOptions::default());
        assert_eq!(out, "HTTP/1.1 200 OK\r\nAccept: y\r\naccept: x");
    }

    #[test]
    fn header_sort_single_line_unchanged() {
        let head = "HTTP/1.1 200 OK";
        let out = sort_header_lines(head, &FormatOptions::default());
        assert_eq!(out, head);
    }

    // --- XML reindent (§5.2) ---

    #[test]
    fn xml_reindent_simple_indent2() {
        let out = format_body(
            "<root><e>text</e></root>",
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(out, "<root>\n  <e>text</e>\n</root>");
    }

    #[test]
    fn xml_reindent_indent4() {
        let opts = FormatOptions {
            xml_indent: 4,
            ..FormatOptions::default()
        };
        let out = format_body(
            "<root><e>text</e></root>",
            Some("application/xml"),
            false,
            &opts,
            PrettyMode::All,
        );
        assert_eq!(out, "<root>\n    <e>text</e>\n</root>");
    }

    #[test]
    fn xml_declaration_preserved() {
        let out = format_body(
            r#"<?xml version="1.0" encoding="utf-8"?><root><e>text</e></root>"#,
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <e>text</e>\n</root>"
        );
    }

    #[test]
    fn xml_declaration_standalone_attr_preserved() {
        let out = format_body(
            r#"<?xml version="1.0" standalone="yes"?><root/>"#,
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert!(out.starts_with(r#"<?xml version="1.0" standalone="yes"?>"#));
    }

    #[test]
    fn xml_format_false_leaves_unchanged() {
        let opts = FormatOptions {
            xml_format: false,
            ..FormatOptions::default()
        };
        let raw = "<root><e>text</e></root>";
        let out = format_body(raw, Some("application/xml"), false, &opts, PrettyMode::All);
        assert_eq!(out, raw);
    }

    #[test]
    fn xml_invalid_passes_through_unchanged() {
        let raw = "<root><unclosed></root>";
        let out = format_body(
            raw,
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(out, raw);
    }

    #[test]
    fn xml_doctype_left_unchanged_for_safety() {
        // A DOCTYPE (entity-expansion danger surface) is never reformatted.
        let raw = r#"<!DOCTYPE root><root><e>x</e></root>"#;
        let out = format_body(
            raw,
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(out, raw);
    }

    #[test]
    fn xml_entity_bomb_left_unchanged() {
        // Billion-laughs style input must pass through verbatim, never
        // expanded — the DOCTYPE tripwire catches it before any expansion.
        let raw = concat!(
            r#"<?xml version="1.0"?>"#,
            r#"<!DOCTYPE lolz [<!ENTITY lol "lol">"#,
            r#"<!ENTITY lol2 "&lol;&lol;">]>"#,
            r#"<lolz>&lol2;</lolz>"#
        );
        let out = format_body(
            raw,
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(out, raw);
    }

    #[test]
    fn xml_not_xml_mime_no_op() {
        // JSON mime → XML step never touches the body.
        let raw = "<root><e>x</e></root>";
        let out = format_body(
            raw,
            Some("application/json"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        // Not JSON-parseable and not XML-eligible → unchanged.
        assert_eq!(out, raw);
    }

    #[test]
    fn xml_comment_preserved() {
        let out = format_body(
            "<root><!-- hi --><e>x</e></root>",
            Some("application/xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert!(out.contains("<!-- hi -->"));
        assert!(out.contains("<e>x</e>"));
    }

    #[test]
    fn xhtml_routed_through_xml() {
        let out = format_body(
            "<html><body><p>hi</p></body></html>",
            Some("application/xhtml+xml"),
            false,
            &FormatOptions::default(),
            PrettyMode::All,
        );
        assert_eq!(out, "<html>\n  <body>\n    <p>hi</p>\n  </body>\n</html>");
    }
}
