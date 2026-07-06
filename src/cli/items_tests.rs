use std::io::Write;

use super::args::RequestType;
use super::items::{DataValue, HeaderItem, MultipartEntry, process_items};

fn tokens(list: &[&str]) -> Vec<String> {
    list.iter().map(|t| t.to_string()).collect()
}

#[track_caller]
fn ok(list: &[&str], request_type: Option<RequestType>) -> super::items::RequestItems {
    process_items(&tokens(list), request_type).expect("expected items to process")
}

#[track_caller]
fn err(list: &[&str], request_type: Option<RequestType>) -> String {
    process_items(&tokens(list), request_type)
        .expect_err("expected an item error")
        .message
}

fn temp_file(content: &[u8], suffix: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .expect("temp file");
    file.write_all(content).expect("write temp file");
    file
}

#[test]
fn header_variants() {
    let items = ok(&["Name:value", "Gone:", "Empty;"], None);
    assert_eq!(
        items.headers,
        vec![
            HeaderItem {
                name: "Name".into(),
                value: Some("value".into())
            },
            HeaderItem {
                name: "Gone".into(),
                value: None
            },
            HeaderItem {
                name: "Empty".into(),
                value: Some("".into())
            },
        ]
    );
}

#[test]
fn empty_header_with_junk_is_an_error() {
    assert_eq!(
        err(&["Name;junk"], None),
        "Invalid item 'Name;junk' (to specify an empty header use 'Header;')"
    );
}

#[test]
fn header_from_file_strips_trailing_newlines() {
    let file = temp_file(b"token-value\n\n", ".txt");
    let token = format!("X-Auth:@{}", file.path().display());
    let items = ok(&[token.as_str()], None);
    assert_eq!(items.headers[0].value.as_deref(), Some("token-value"));
}

#[test]
fn params_accumulate_in_order() {
    let items = ok(&["q==1", "q==2", "r=="], None);
    assert_eq!(
        items.params,
        vec![
            ("q".to_string(), "1".to_string()),
            ("q".to_string(), "2".to_string()),
            ("r".to_string(), "".to_string()),
        ]
    );
}

#[test]
fn data_values() {
    let items = ok(&["a=text", "b:=42", r#"c:=[1, "x"]"#, "d:=true"], None);
    assert_eq!(
        items.data[0],
        ("a".to_string(), DataValue::Text("text".into()))
    );
    assert!(matches!(&items.data[1].1, DataValue::Json(v) if v.to_string() == "42"));
    assert!(matches!(&items.data[2].1, DataValue::Json(v) if v.to_string() == r#"[1, "x"]"#));
    assert!(matches!(&items.data[3].1, DataValue::Json(v) if v.to_string() == "true"));
}

#[test]
fn invalid_raw_json_reports_the_original_item() {
    let message = err(&["a:=nope"], None);
    assert_eq!(
        message,
        "'a:=nope': Expecting value: line 1 column 1 (char 0)"
    );
}

#[test]
fn form_mode_stringifies_primitives() {
    let form = Some(RequestType::Form);
    let items = ok(
        &["i:=1", "f:=1.1", "t:=true", "x:=false", r#"s:="quoted""#],
        form,
    );
    let texts: Vec<&DataValue> = items.data.iter().map(|(_, v)| v).collect();
    assert_eq!(texts[0], &DataValue::Text("1".into()));
    assert_eq!(texts[1], &DataValue::Text("1.1".into()));
    assert_eq!(texts[2], &DataValue::Text("True".into()));
    assert_eq!(texts[3], &DataValue::Text("False".into()));
    assert_eq!(texts[4], &DataValue::Text("quoted".into()));
}

#[test]
fn form_mode_rejects_complex_json() {
    for token in ["a:=[1,2]", r#"a:={"x":1}"#, "a:=null", "a:=broken"] {
        assert_eq!(
            err(&[token], Some(RequestType::Form)),
            "Cannot use complex JSON value types with --form/--multipart.",
            "token: {token}"
        );
        assert_eq!(
            err(&[token], Some(RequestType::Multipart)),
            "Cannot use complex JSON value types with --form/--multipart.",
            "token: {token}"
        );
    }
}

#[test]
fn data_from_file_keeps_newlines() {
    let file = temp_file(b"line\n", ".txt");
    let token = format!("field=@{}", file.path().display());
    let items = ok(&[token.as_str()], None);
    assert_eq!(items.data[0].1, DataValue::Text("line\n".into()));
}

#[test]
fn json_from_file() {
    let file = temp_file(br#"{"k": 1}"#, ".json");
    let token = format!("field:=@{}", file.path().display());
    let items = ok(&[token.as_str()], None);
    assert!(matches!(&items.data[0].1, DataValue::Json(v) if v.to_string() == r#"{"k": 1}"#));
}

#[test]
fn non_utf8_embed_is_an_error() {
    let file = temp_file(&[0xff, 0xfe, 0x00], ".bin");
    let token = format!("field=@{}", file.path().display());
    let message = err(&[token.as_str()], None);
    assert!(
        message.contains("cannot embed the content of"),
        "got: {message}"
    );
    assert!(message.contains("not a UTF-8 or ASCII-encoded text file"));
}

#[test]
fn missing_embed_file_is_an_error() {
    let message = err(&["f=@/definitely/not/here.txt"], None);
    assert!(message.starts_with("'f=@/definitely/not/here.txt': "));
}

#[test]
fn file_fields_require_form_mode() {
    let file = temp_file(b"x", ".txt");
    let token = format!("field@{}", file.path().display());
    assert_eq!(
        err(&[token.as_str()], None),
        "Invalid file fields (perhaps you meant --form?): field"
    );
    let other = format!("second@{}", file.path().display());
    assert_eq!(
        err(&[token.as_str(), other.as_str()], None),
        "Invalid file fields (perhaps you meant --form?): field, second"
    );
}

#[test]
fn form_file_fields_capture_mime_override() {
    let file = temp_file(b"x", ".ico");
    let token = format!(
        "logo@{};type=image/vnd.microsoft.icon",
        file.path().display()
    );
    let items = ok(&[token.as_str()], Some(RequestType::Form));
    assert_eq!(items.files.len(), 1);
    assert_eq!(items.files[0].name, "logo");
    assert_eq!(
        items.files[0].mime.as_deref(),
        Some("image/vnd.microsoft.icon")
    );
    assert_eq!(items.files[0].path, file.path());
}

#[test]
fn whole_body_file() {
    let file = temp_file(b"body", ".txt");
    let token = format!("@{}", file.path().display());
    let items = ok(&[token.as_str()], None);
    assert_eq!(items.body_file.as_ref().unwrap().path, file.path());
    assert!(items.has_data());
}

#[test]
fn multiple_body_files_error() {
    let a = temp_file(b"a", ".txt");
    let b = temp_file(b"b", ".txt");
    let token_a = format!("@{}", a.path().display());
    let token_b = format!("@{}", b.path().display());
    assert_eq!(
        err(&[token_a.as_str(), token_b.as_str()], None),
        "Can't read request from multiple files"
    );
}

#[test]
fn missing_body_file_is_an_error() {
    let message = err(&["@/definitely/not/here.txt"], None);
    assert!(message.starts_with("'@/definitely/not/here.txt': "));
}

#[test]
fn multipart_sequence_interleaves_text_and_files_but_drops_typed() {
    let file = temp_file(b"x", ".txt");
    let upload = format!("pic@{}", file.path().display());
    let items = ok(
        &["first=1", "n:=2", upload.as_str(), "last=3"],
        Some(RequestType::Multipart),
    );
    assert_eq!(items.multipart_sequence.len(), 3);
    assert!(matches!(
        &items.multipart_sequence[0],
        MultipartEntry::Text { name, value } if name == "first" && value == "1"
    ));
    assert!(matches!(
        &items.multipart_sequence[1],
        MultipartEntry::File(f) if f.name == "pic"
    ));
    assert!(matches!(
        &items.multipart_sequence[2],
        MultipartEntry::Text { name, .. } if name == "last"
    ));
    // The typed item is still present as form data (three text fields);
    // the file upload lives in `files`.
    assert_eq!(items.data.len(), 3);
    assert_eq!(items.files.len(), 1);
}

#[test]
fn has_data_reflects_sources() {
    assert!(!ok(&["X:1", "q==2"], None).has_data());
    assert!(ok(&["a=b"], None).has_data());
}
