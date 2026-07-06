# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-07

Initial release.

### Added

- Three binaries: `furl` (default scheme `http://`), `furls` (default scheme
  `https://`), and `furl-manager` (maintenance CLI).
- Request-item grammar with all ten separators: data fields (`name=value`),
  raw-JSON fields (`name:=value`), file-backed data (`name=@file`,
  `name:=@file`), headers (`Name:value`), empty headers (`Name;`), file-backed
  headers (`Name:@file`), query parameters (`name==value`, `name==@file`), and
  file-upload fields (`field@file`). Separator characters are backslash-escapable.
- JSON request bodies (the default), form bodies (`-f`/`--form`), and
  multipart bodies (`--multipart`, or automatically when files are attached).
- Nested-object path syntax on data keys (`user[name]=Ada`, `tags[]:=1`) that
  folds request items into structured JSON.
- URL query parameters via the `==` separator.
- Custom request headers, including empty and file-backed values.
- Authentication: `-a`/`--auth` with `-A`/`--auth-type` for `basic`, `bearer`,
  and `digest` (Digest partially supported; see the migration notes).
- URL shorthand: bare hosts and paths are expanded using each binary's default
  scheme; `--default-scheme` overrides it.
- `--offline` to build and print a request without sending it.
- Online requests over HTTP and HTTPS for GET, POST, and other methods, with
  the method inferred from the presence of request data.
- Redirect handling: `-F`/`--follow`, `--all`, and `--max-redirects`.
- `--check-status` to reflect the HTTP status class in the process exit code.
- Cookie propagation across redirects, scoped so cookies are not forwarded to a
  different origin.
- Automatic gzip and deflate response-body decoding.
- Output control: `--print`, `-v`/`--verbose`, `-h`/`--headers`, `-b`/`--body`,
  `-m`/`--meta`, and `-q`/`--quiet`.
- `--timeout` for idle-connection deadlines.
- TLS controls: `--verify` (on by default), `--ssl` for the protocol version,
  and `--cert`/`--cert-key` for client certificates.
- Declarative option table driving parsing, `--no-OPTION` negation, and help
  rendering, with argparse-style unambiguous long-option prefix matching.

[Unreleased]: https://github.com/o1x3/furl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/o1x3/furl/releases/tag/v0.1.0
