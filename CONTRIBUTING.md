# Contributing to furl

Thanks for your interest in improving furl. This guide covers building, testing,
and the conventions we follow.

## Prerequisites

- A recent stable Rust toolchain (edition 2024; minimum supported Rust version
  1.85). Install it with [rustup](https://rustup.rs/).

## Build

```sh
cargo build              # debug build of all three binaries
cargo build --release    # optimized build
```

The crate is `furl-http`; it produces the `furl`, `furls`, and `furl-manager`
binaries and a `furl` library.

## Test

```sh
cargo test               # unit, integration, and property tests
```

Property tests (via `proptest`) exercise the parsers with generated input. Some
integration tests spin up a local test server and compare furl's output against
recorded golden results, so no network access is required to run the suite.

### Differential testing (conceptual)

furl targets a command grammar compatible with a widely used HTTP client. To
guard against drift, contributors can run furl and a reference client against the
same inputs and diff their output — request lines, headers, bodies, and exit
codes. When furl deliberately differs, the difference is recorded in
[docs/DEVIATIONS.md](docs/DEVIATIONS.md); please add an entry there rather than
"fixing" an intentional deviation.

## Lint and format

Both must pass before a change is merged:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

Run `cargo fmt --all` to apply formatting. The crate denies `unsafe_code` and
treats clippy warnings as errors in CI.

## Commit style

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>
```

Common types: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`,
`chore`. Keep the subject line in the imperative mood and under ~72 characters.

## Sign your work (DCO)

Contributions are accepted under the
[Developer Certificate of Origin](https://developercertificate.org/). Certify it
by signing off each commit:

```sh
git commit -s -m "feat(cli): add --foo"
```

This appends a `Signed-off-by: Your Name <you@example.com>` trailer.

## Pull requests

- Keep changes focused; one logical change per PR.
- Update tests and documentation alongside code.
- Note any user-visible behavior change in [CHANGELOG.md](CHANGELOG.md) under
  `## [Unreleased]`.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By
participating you agree to uphold it. Report unacceptable behavior to
security@o1x3.com.
