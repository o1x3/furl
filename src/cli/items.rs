//! Request-item processing: from raw item tokens to the collections a
//! request is built from.

use crate::cli::nested_json::{NestedJson, NestedJsonError};
use crate::json;
use crate::paths::expand_tilde;

use super::args::RequestType;
use super::request_items::{ALL_SEPARATORS, Separator, split_item};

/// A header occurrence: a value, or an unset marker (`Name:`) that wipes
/// accumulated values and suppresses defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct HeaderItem {
    pub name: String,
    pub value: Option<String>,
}

/// A data field: a plain string or a typed raw-JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum DataValue {
    Text(String),
    Json(json::Value),
}

/// A `field@path` form file upload.
#[derive(Debug, Clone, PartialEq)]
pub struct FileField {
    pub name: String,
    pub path: std::path::PathBuf,
    /// Explicit `;type=MIME` override.
    pub mime: Option<String>,
}

/// One part of a multipart body, in CLI order across text and file items.
#[derive(Debug, Clone, PartialEq)]
pub enum MultipartEntry {
    Text { name: String, value: String },
    File(FileField),
}

/// The bare `@path` whole-body shortcut.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyFile {
    pub path: std::path::PathBuf,
}

#[derive(Debug, Default)]
pub struct RequestItems {
    pub headers: Vec<HeaderItem>,
    pub params: Vec<(String, String)>,
    pub data: Vec<(String, DataValue)>,
    pub files: Vec<FileField>,
    /// JSON mode: the data items folded through the nested-JSON syntax.
    pub json_data: Option<json::Value>,
    /// `=`, `=@`, and `@` items in original order (multipart framing).
    pub multipart_sequence: Vec<MultipartEntry>,
    pub body_file: Option<BodyFile>,
}

impl RequestItems {
    /// Any data separator present? (Method guessing counts these.)
    pub fn has_data(&self) -> bool {
        !self.data.is_empty() || !self.files.is_empty() || self.body_file.is_some()
    }
}

/// An item-processing failure.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemError {
    /// Rendered as a usage error.
    Message(String),
    /// Nested-JSON errors carry their own annotated rendering.
    NestedJson(NestedJsonError),
}

impl ItemError {
    fn new(message: impl Into<String>) -> ItemError {
        ItemError::Message(message.into())
    }
}

/// Fold raw item tokens into collections, reading embedded files.
///
/// `form_mode` covers both `--form` and `--multipart`: it changes how
/// typed (`:=`) values are treated and makes named file fields legal.
pub fn process_items(
    tokens: &[String],
    request_type: Option<RequestType>,
) -> Result<RequestItems, ItemError> {
    let form_mode = matches!(
        request_type,
        Some(RequestType::Form) | Some(RequestType::Multipart)
    );
    let mut items = RequestItems::default();
    let mut invalid_file_fields: Vec<String> = Vec::new();
    let mut body_files_seen = 0usize;

    // JSON mode evaluates every data item first — all values (including
    // embedded files and raw JSON) resolve as a batch, then fold through
    // the nested-JSON syntax — before any other item is examined.
    let mut remaining: Vec<&String> = Vec::new();
    if form_mode {
        remaining.extend(tokens);
    } else {
        let mut json_items: Vec<&String> = Vec::new();
        for token in tokens {
            match split_item(token, ALL_SEPARATORS) {
                Ok(split)
                    if split.separator.is_data() && split.separator != Separator::FileUpload =>
                {
                    json_items.push(token);
                }
                _ => remaining.push(token),
            }
        }
        if !json_items.is_empty() {
            for token in &json_items {
                let split = split_item(token, ALL_SEPARATORS).expect("split checked above");
                let value = match split.separator {
                    Separator::Data => DataValue::Text(split.value),
                    Separator::DataFromFile => {
                        DataValue::Text(read_text_file(token, &split.value)?)
                    }
                    Separator::RawJson => parse_json_value(token, &split.value, false)?,
                    Separator::RawJsonFromFile => {
                        let text = read_text_file(token, &split.value)?;
                        parse_json_value(token, &text, false)?
                    }
                    _ => unreachable!("only data separators reach the JSON batch"),
                };
                items.data.push((split.key, value));
            }
            let mut nested = NestedJson::new();
            for (key, value) in &items.data {
                let value = match value {
                    DataValue::Text(text) => json::Value::String(text.clone()),
                    DataValue::Json(value) => value.clone(),
                };
                nested.assign(key, value).map_err(ItemError::NestedJson)?;
            }
            items.json_data = Some(nested.finish());
        }
    }

    for token in remaining {
        let split = split_item(token, ALL_SEPARATORS)
            .map_err(|_| ItemError::new(format!("'{token}' is not a valid value")))?;
        let key = split.key;
        let value = split.value;
        match split.separator {
            Separator::Header => {
                let value = if value.is_empty() { None } else { Some(value) };
                items.headers.push(HeaderItem { name: key, value });
            }
            Separator::HeaderEmpty => {
                if !value.is_empty() {
                    return Err(ItemError::new(format!(
                        "Invalid item '{token}' (to specify an empty header use `Header;`)"
                    )));
                }
                items.headers.push(HeaderItem {
                    name: key,
                    value: Some(String::new()),
                });
            }
            Separator::HeaderFromFile => {
                let text = read_text_file(token, &value)?;
                items.headers.push(HeaderItem {
                    name: key,
                    value: Some(text.trim_end_matches('\n').to_string()),
                });
            }
            Separator::Query => items.params.push((key, value)),
            Separator::QueryFromFile => {
                let text = read_text_file(token, &value)?;
                items
                    .params
                    .push((key, text.trim_end_matches('\n').to_string()));
            }
            Separator::Data => {
                items.multipart_sequence.push(MultipartEntry::Text {
                    name: key.clone(),
                    value: value.clone(),
                });
                items.data.push((key, DataValue::Text(value)));
            }
            Separator::DataFromFile => {
                let text = read_text_file(token, &value)?;
                items.multipart_sequence.push(MultipartEntry::Text {
                    name: key.clone(),
                    value: text.clone(),
                });
                items.data.push((key, DataValue::Text(text)));
            }
            Separator::RawJson => {
                let parsed = parse_json_value(token, &value, form_mode)?;
                items.data.push((key, parsed));
            }
            Separator::RawJsonFromFile => {
                let text = read_text_file(token, &value)?;
                let parsed = parse_json_value(token, &text, form_mode)?;
                items.data.push((key, parsed));
            }
            Separator::FileUpload => {
                if form_mode {
                    let field = file_field(&key, &value);
                    items
                        .multipart_sequence
                        .push(MultipartEntry::File(field.clone()));
                    items.files.push(field);
                } else if key.is_empty() {
                    body_files_seen += 1;
                    if body_files_seen > 1 {
                        return Err(ItemError::new("Can't read request from multiple files"));
                    }
                    let field = file_field(&key, &value);
                    // Openability is checked now; contents stream later.
                    std::fs::File::open(&field.path)
                        .map_err(|error| ItemError::new(format!("'{token}': {error}")))?;
                    items.body_file = Some(BodyFile { path: field.path });
                } else {
                    invalid_file_fields.push(key);
                }
            }
        }
    }

    if !invalid_file_fields.is_empty() {
        invalid_file_fields.sort();
        invalid_file_fields.dedup();
        return Err(ItemError::new(format!(
            "Invalid file fields (perhaps you meant --form?): {}",
            invalid_file_fields.join(", ")
        )));
    }
    Ok(items)
}

/// Split a `path[;type=MIME]` file-upload value.
fn file_field(key: &str, value: &str) -> FileField {
    let (path, mime) = match value.split_once(";type=") {
        Some((path, mime)) => (path, Some(mime.to_string())),
        None => (value, None),
    };
    FileField {
        name: key.to_string(),
        path: expand_tilde(path),
        mime,
    }
}

/// Read a `…@path` embedded value as UTF-8 text.
fn read_text_file(token: &str, path: &str) -> Result<String, ItemError> {
    let expanded = expand_tilde(path);
    let bytes =
        std::fs::read(&expanded).map_err(|error| ItemError::new(format!("'{token}': {error}")))?;
    String::from_utf8(bytes).map_err(|_| {
        ItemError::new(format!(
            "'{token}': cannot embed the content of '{path}', \
             not a UTF-8 or ASCII-encoded text file"
        ))
    })
}

/// Evaluate a `:=` value: full JSON in JSON mode; in form mode only
/// primitives, stringified.
fn parse_json_value(token: &str, text: &str, form_mode: bool) -> Result<DataValue, ItemError> {
    if !form_mode {
        let value =
            json::parse(text).map_err(|error| ItemError::new(format!("'{token}': {error}")))?;
        return Ok(DataValue::Json(value));
    }
    let complex = || {
        ItemError::new("Cannot use complex JSON value types with --form/--multipart.".to_string())
    };
    let value = json::parse(text).map_err(|_| complex())?;
    let text = match &value {
        // The reference tool renders booleans with Python spellings in
        // form fields; kept for compatibility.
        json::Value::Bool(true) => "True".to_string(),
        json::Value::Bool(false) => "False".to_string(),
        json::Value::String(s) => s.clone(),
        json::Value::Number(_) => json::dumps(&value, &json::DumpOptions::default()),
        _ => return Err(complex()),
    };
    Ok(DataValue::Text(text))
}
