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
        })?;
    encoding_rs::Encoding::for_label(squashed.as_bytes())
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
