//! `--help` text generation from the option table.
//!
//! Interim layout: usage, grouped options, negation rule. Full parity
//! with the compatibility target's rich help layout is tracked
//! separately.

use crate::cli::options::{Group, OPTIONS};

const GROUPS: &[Group] = &[
    Group::ContentTypes,
    Group::ContentProcessing,
    Group::OutputProcessing,
    Group::OutputOptions,
    Group::Sessions,
    Group::Authentication,
    Group::Network,
    Group::Ssl,
    Group::Troubleshooting,
];

pub fn full_help(program: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "usage:\n    {program} [METHOD] URL [REQUEST_ITEM ...]\n\n"
    ));
    out.push_str(
        "furl: a human-friendly command-line HTTP client for the API era.\n\n\
         Positional arguments:\n\n\
         \x20 METHOD        The HTTP method (GET, POST, PUT, DELETE, ...);\n\
         \x20               guessed when omitted: POST with data, GET without.\n\
         \x20 URL           The request URL. A missing scheme defaults to\n\
         \x20               http:// (or https:// for furls); :3000/path\n\
         \x20               abbreviates localhost.\n\
         \x20 REQUEST_ITEM  Key/value pairs shaping the request:\n\
         \x20               Header:value, name=data, name:=json, name==param,\n\
         \x20               field@file, name=@file, name:=@file\n\n",
    );
    for group in GROUPS {
        let visible: Vec<_> = OPTIONS
            .iter()
            .filter(|spec| spec.group == *group && !spec.hidden)
            .collect();
        if visible.is_empty() {
            continue;
        }
        out.push_str(group.title());
        out.push_str(":\n\n");
        for spec in visible {
            let mut invocation = spec.aliases.join(", ");
            if let Some(metavar) = spec.metavar {
                invocation.push(' ');
                invocation.push_str(metavar);
            }
            out.push_str(&format!("  {invocation}\n"));
            for line in wrap(spec.help, 68) {
                out.push_str(&format!("      {line}\n"));
            }
            out.push('\n');
        }
    }
    out.push_str(
        "For every --OPTION there is a --no-OPTION that reverts it to its\n\
         default value.\n",
    );
    out
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && current.len() + 1 + word.len() > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}
