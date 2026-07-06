//! furl — a human-friendly command-line HTTP client.
//!
//! This library crate backs the `furl`, `furls`, and `furl-manager`
//! binaries. The binaries are thin wrappers; all behavior lives here.

pub mod cli;
pub mod config;
pub mod cookies;
pub mod download;
pub mod encoding;
pub mod errors;
pub mod help;
pub mod json;
pub mod output;
pub mod paths;
pub mod request;
mod run;
pub mod session;
pub mod status;
pub mod transport;

/// The furl version, taken from the crate metadata.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Which command-line program variant was invoked.
///
/// `furl` and `furls` share a grammar and differ only in the default URL
/// scheme; `furl-manager` is a separate maintenance interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// `furl`: default scheme is `http://`.
    Furl,
    /// `furls`: default scheme is `https://`.
    Furls,
}

/// Entry point for the `furl` and `furls` binaries.
pub fn run(program: Program) -> i32 {
    run::run(program)
}

/// Entry point for the `furl-manager` binary.
pub fn run_manager() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("{VERSION}");
        return 0;
    }
    eprintln!("furl-manager: not yet implemented");
    1
}
