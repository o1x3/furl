//! Command-line grammar: flag definitions, request items, and the
//! nested-JSON data syntax.

pub mod args;
pub mod items;
pub mod nested_json;
pub mod options;
pub mod parser;
pub mod request_items;
pub mod urls;

#[cfg(test)]
mod items_tests;
#[cfg(test)]
mod parser_tests;
