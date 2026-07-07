# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-07

A parity and hardening release: previously parsed-but-inert options are now
wired, the error/output surface matches the reference more closely, and a
differential audit surfaced and fixed several correctness and robustness bugs.

### Added

- **Proxies.** `--proxy` and the `*_proxy` / `no_proxy` environment variables
  route requests: plain-`http` targets use absolute-form with
  `Proxy-Authorization`, `https` targets tunnel through `CONNECT` (including
  TLS-in-TLS for `https` proxies). `no_proxy` supports exact, dot-suffix, CIDR,
  and `*` matching.
- **`--ciphers`** filters the TLS cipher suites (rustls/IANA names) and
  **`--cert-key-pass`** decrypts PKCS#8-encrypted client keys, prompting on the
  terminal when the passphrase is not supplied.
- **`furl-manager cli sessions upgrade` / `upgrade-all`** migrate legacy
  dict-layout cookies and headers to the current list layout.
- **`furl-manager --help` / `-h`** prints usage.
- Byte-exact rendering for every named 256-color style; `--style` now rejects
  unknown names.
- Response-charset handling (`--response-charset`, declared charset, detection),
  `--stream` line-by-line processing, and binary-body suppression across the
  whole text pipeline.
- Framed stderr log blocks with per-line coloring and quiet suppression, a
  slow-stdin read warning, `SIGINT` → 130, and broken-pipe handling.

### Fixed

- **Do not panic or hang on non-`http(s)` schemes.** A `ftp://`, `file://`,
  `mailto:`, or `data:` target — or a redirect to one — now errors with
  `InvalidSchema` instead of hanging or panicking.
- **Reject header injection.** Header names/values containing CR/LF, reserved
  characters, or leading whitespace are rejected before any connection.
- **IPv6 and IDN hosts.** Bracketed IPv6 literals resolve correctly and
  non-ASCII hosts are IDNA-encoded in the `Host` header.
- **URL requoting** matches the reference wire form (unsafe characters encoded,
  escapes normalized, lone `%` escaped); an empty authority is rejected as a
  missing host.
- **Redirects** rewrite all non-`HEAD` methods to `GET` on 302/303 and purge the
  body and framing headers for every redirect except 307/308.
- **Cookies** honor `Max-Age`/`Expires` (expired cookies are deleted and never
  sent) and default to the request path's directory.
- URL-userinfo credentials go into Basic auth verbatim; multipart parameter
  names/filenames escape `"`, CR, and LF; the `--max-headers`, `--continue`,
  and `--print` error messages, the terminal trailing blank line, and the
  connection-error chains match the reference.

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
- `-x`/`--compress` to Deflate-compress the request body (`-x` only when it
  shrinks the body; `-xx` unconditionally).
- `-d`/`--download` with filename derivation (Content-Disposition, URL
  basename, or a Content-Type extension) and `-c`/`--continue` resume.
- Sessions: `--session` and `--session-read-only` persist headers, cookies, and
  credentials to a JSON session file and replay them on later requests.
- Configuration: `config.json` `default_options` are prepended to every
  invocation; the config directory honors `FURL_CONFIG_DIR` and XDG.
- Output formatting: `--pretty`, `--format-options`, and `--sorted`/`--unsorted`
  reindent JSON and XML bodies and sort headers (color highlighting is planned).
- `furl-manager cli export-args` emits a machine-readable description of the
  request parser; `furl-manager` also reports plugin and update-check status.
- ANSI color highlighting of headers and JSON bodies (`--pretty=colors`/`all`,
  `--style`), byte-exact with the reference for the `auto` and pie styles.
- `.netrc` credential lookup as an authentication fallback.
- Online HTTP Digest authentication (`-A digest`), answering a 401 challenge
  with a computed `Authorization: Digest` retry.

[Unreleased]: https://github.com/o1x3/furl/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/o1x3/furl/releases/tag/v0.1.0
