//! Text-encoding lookup shared by option validation and response decoding.

/// Is `label` a text encoding this program can decode?
///
/// Labels are matched per the WHATWG registry after normalizing common
/// alternative spellings (underscores for hyphens, surrounding space).
pub fn is_known_encoding(label: &str) -> bool {
    lookup(label).is_some()
}

pub fn lookup(label: &str) -> Option<&'static encoding_rs::Encoding> {
    let normalized = label.trim().replace('_', "-");
    if let Some(encoding) = encoding_rs::Encoding::for_label(normalized.as_bytes()) {
        return Some(encoding);
    }
    // Spellings like "utf-16-le" for the registry's "utf-16le".
    let squashed = normalized
        .strip_suffix("-le")
        .map(|stem| format!("{stem}le"))
        .or_else(|| {
            normalized
                .strip_suffix("-be")
                .map(|stem| format!("{stem}be"))
        });
    if let Some(encoding) =
        squashed.and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
    {
        return Some(encoding);
    }
    // Hyphen-free aliases: "latin-1" for the registry's "latin1".
    let dehyphenated = normalized.replace('-', "");
    encoding_rs::Encoding::for_label(dehyphenated.as_bytes())
}

/// Detection only fires for bodies past this size; shorter ones assume
/// UTF-8 (guessing tiny inputs produces confusing results).
const DETECTION_MINIMUM: usize = 32;

/// Decode body bytes for the text pipeline: the given label when one is
/// declared (or forced), valid UTF-8 as itself, then a detector guess for
/// longer non-UTF-8 bodies. Malformed sequences become U+FFFD.
pub fn decode_body(bytes: &[u8], label: Option<&str>) -> String {
    if let Some(encoding) = label.and_then(lookup) {
        return encoding.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) if bytes.len() > DETECTION_MINIMUM => {
            let mut detector = chardetng::EncodingDetector::new();
            detector.feed(bytes, true);
            let encoding = detector.guess(None, true);
            encoding.decode(bytes).0.into_owned()
        }
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Encode pipeline text for the output target: a terminal always gets
/// UTF-8; a pipe gets the message's own declared encoding (UTF-8 when
/// none resolves), so unformatted foreign-charset bodies round-trip.
pub fn encode_body(text: &str, declared: Option<&str>, terminal: bool) -> Vec<u8> {
    if terminal {
        return text.as_bytes().to_vec();
    }
    match declared.and_then(lookup) {
        Some(encoding) => encoding.encode(text).0.into_owned(),
        None => text.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_labels_resolve() {
        for label in [
            "utf-8",
            "UTF-8",
            "utf8",
            "utf_8",
            "latin1",
            "iso-8859-1",
            "big5",
            "gb2312",
            "utf-16",
            "utf-16le",
            "utf_16_be",
            "shift-jis",
            "euc-kr",
            "windows-1252",
        ] {
            assert!(is_known_encoding(label), "{label} should resolve");
        }
    }

    #[test]
    fn unknown_labels_fail() {
        for label in ["utf-64", "foobar", "", "klingon"] {
            assert!(!is_known_encoding(label), "{label} should not resolve");
        }
    }
}
