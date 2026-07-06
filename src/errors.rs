//! User-facing error rendering.

use crate::cli::options::OptionSpec;

pub const DOCS_URL: &str = "https://github.com/o1x3/furl";

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

/// The single-line runtime error: `furl: error: Kind: message`.
pub fn runtime_error_line(program: &str, kind: &str, message: &str) -> String {
    format!("{program}: error: {kind}: {message}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::find_exact;

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
