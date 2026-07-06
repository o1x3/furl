//! The `furl-manager cli export-args` machine-readable parser dump.

use crate::cli::options::{Group, OPTIONS, OptionSpec};
use crate::json::{DumpOptions, Value};

/// The schema version string, matching the compatibility target so
/// existing tooling can consume furl's export.
const SPEC_VERSION: &str = "0.0.1a0";

/// The help groups in display order.
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

/// Render the full export as a single-line JSON document.
pub fn export_json() -> String {
    let value = build();
    format!("{}\n", crate::json::dumps(&value, &compact()))
}

fn compact() -> DumpOptions {
    DumpOptions {
        indent: None,
        sort_keys: false,
        ensure_ascii: true,
    }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn build() -> Value {
    let mut groups: Vec<Value> = Vec::new();
    groups.push(positional_group());
    for group in GROUPS {
        groups.push(option_group(*group));
    }

    obj(vec![
        ("version", Value::from(SPEC_VERSION)),
        (
            "spec",
            obj(vec![
                ("name", Value::from("furl")),
                (
                    "description",
                    Value::from("furl: a human-friendly command-line HTTP client for the API era."),
                ),
                ("groups", Value::Array(groups)),
            ]),
        ),
    ])
}

fn positional_group() -> Value {
    let args = vec![
        positional(
            "METHOD",
            true,
            false,
            "The HTTP method for the request; guessed when omitted.",
        ),
        positional(
            "URL",
            false,
            false,
            "The request URL; a missing scheme defaults per program.",
        ),
        positional(
            "REQUEST_ITEM",
            true,
            true,
            "Key/value items specifying headers, data, query parameters, and files.",
        ),
    ];
    obj(vec![
        ("name", Value::from("Positional arguments")),
        ("description", Value::Null),
        ("is_mutually_exclusive", Value::Bool(false)),
        ("args", Value::Array(args)),
    ])
}

fn positional(metavar: &str, optional: bool, variadic: bool, help: &str) -> Value {
    let mut pairs = vec![
        ("options", Value::Array(vec![Value::from(metavar)])),
        ("is_positional", Value::Bool(true)),
    ];
    if optional {
        pairs.push(("is_optional", Value::Bool(true)));
    }
    if variadic {
        pairs.push(("is_variadic", Value::Bool(true)));
    }
    pairs.push(("short_description", Value::from(help)));
    pairs.push(("description", Value::from(help)));
    pairs.push(("metavar", Value::from(metavar)));
    obj(pairs)
}

fn option_group(group: Group) -> Value {
    let is_exclusive = matches!(group, Group::Sessions);
    let args: Vec<Value> = OPTIONS
        .iter()
        .filter(|spec| spec.group == group && !spec.hidden)
        .map(option_arg)
        .collect();
    obj(vec![
        ("name", Value::from(group.title())),
        ("description", Value::Null),
        ("is_mutually_exclusive", Value::Bool(is_exclusive)),
        ("args", Value::Array(args)),
    ])
}

fn option_arg(spec: &OptionSpec) -> Value {
    let mut pairs = vec![(
        "options",
        Value::Array(spec.aliases.iter().map(|a| Value::from(*a)).collect()),
    )];
    if !spec.help.is_empty() {
        pairs.push(("short_description", Value::from(spec.help)));
        pairs.push(("description", Value::from(spec.help)));
    }
    if let Some(choices) = spec.choices {
        pairs.push((
            "choices",
            Value::Array(choices.iter().map(|c| Value::from(*c)).collect()),
        ));
    }
    if let Some(metavar) = spec.metavar {
        pairs.push(("metavar", Value::from(metavar)));
    }
    obj(pairs)
}
