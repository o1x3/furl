# furl

[![CI](https://github.com/o1x3/furl/actions/workflows/ci.yml/badge.svg)](https://github.com/o1x3/furl/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/furl-http.svg)](https://crates.io/crates/furl-http)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

**furl** is a human-friendly command-line HTTP client for the API era. It turns
terse, memorable command-line syntax into well-formed HTTP requests, so building
a JSON POST or attaching a header is a matter of a few `name=value` tokens rather
than a wall of flags.

furl ships three binaries:

| Binary         | Purpose                                             | Default scheme |
|----------------|-----------------------------------------------------|----------------|
| `furl`         | The everyday HTTP client                            | `http://`      |
| `furls`        | Same client, defaults to TLS                        | `https://`     |
| `furl-manager` | Maintenance and introspection CLI                   | —              |

## Install

### From crates.io

```sh
cargo install furl-http
```

This builds and installs all three binaries (`furl`, `furls`, `furl-manager`).

### One-line installer

```sh
curl -fsSL https://raw.githubusercontent.com/o1x3/furl/main/install.sh | sh
```

### Prebuilt binaries

Download an archive for your platform from the
[releases page](https://github.com/o1x3/furl/releases), extract it, and place the
binaries on your `PATH`.

## Quickstart

```sh
# GET request (method is inferred: no data -> GET)
furl example.org/status

# GET with query parameters (== adds to the URL query string)
furl example.org/search q==furl per_page==20

# POST JSON (presence of data items infers POST)
#   name=value  -> JSON string field
#   count:=3    -> raw JSON (number, here); also for booleans, arrays, objects
furl example.org/api/items name=widget count:=3 in_stock:=true

# Custom request headers (Name:value)
furl example.org/api X-Api-Version:2 Accept:application/json

# Form submission (-f switches the body to urlencoded form fields)
furl -f example.org/login username=alice password=secret

# File upload (@ attaches a file; the body becomes multipart/form-data)
furl -f example.org/upload avatar@./portrait.png caption=hello

# Basic auth (USER:PASS; omit the password to be prompted)
furl -a alice:secret example.org/private

# Bearer token
furl -A bearer -a "$TOKEN" example.org/private

# Build the request but don't send it (great for debugging)
furl --offline example.org/api name=widget count:=3

# HTTPS via furls (default scheme is https://)
furls api.example.org/health

# Follow redirects, cap the hops, and reflect status in the exit code
furl --follow --max-redirects 5 --check-status example.org/old-path
```

By default furl prints the response headers and body. Use `-v`/`--verbose` to
print the whole exchange (request and response), `-h` for response headers only,
`-b` for the body only, and `-q`/`--quiet` to silence normal output.

## Request-item syntax

Every argument after the URL that isn't a flag is a *request item*. The
separator inside the token decides what it becomes. furl scans each token left to
right and splits on the earliest separator; when several separators start at the
same position, the longest one wins. Separator characters (`:` `;` `=` `@`) can be
escaped with a backslash to include them literally.

| Item                    | Separator | Meaning                                                    |
|-------------------------|-----------|------------------------------------------------------------|
| `name=value`            | `=`       | Data field, sent as a JSON string (or form field with `-f`)|
| `name:=value`           | `:=`      | Raw-JSON data field (numbers, booleans, arrays, objects)   |
| `name=@file`            | `=@`      | Data field whose value is read from a file                 |
| `name:=@file`           | `:=@`     | Raw-JSON data field whose value is read from a file        |
| `Name:value`            | `:`       | HTTP request header                                        |
| `Name;`                 | `;`       | HTTP header with an empty value                            |
| `Name:@file`            | `:@`      | HTTP header whose value is read from a file                |
| `name==value`           | `==`      | URL query-string parameter                                 |
| `name==@file`           | `==@`     | Query parameter whose value is read from a file            |
| `field@file`            | `@`       | File upload form field (implies multipart/form-data)       |

Data items also support a nested-object path syntax on the key
(`user[name]=Ada`, `tags[]:=1`) that folds into structured JSON. See
[docs/](docs/) for the full grammar.

The presence of any data item (or piped stdin) makes the default method `POST`;
otherwise it is `GET`. You can always set the method explicitly by naming it
before the URL: `furl PUT example.org/api name=widget`.

## Documentation

- [docs/](docs/index.md) — documentation index
- [docs/MIGRATION.md](docs/MIGRATION.md) — mapping from an HTTPie-compatible workflow
- [docs/DEVIATIONS.md](docs/DEVIATIONS.md) — where furl intentionally differs

Run `furl --help` for the full flag list, or `furl --manual` for the long-form
manual.

## How furl relates to HTTPie

furl implements a command grammar compatible with HTTPie's: the request-item
separators, the flag names, and the URL shorthand will be familiar if you have
used HTTPie. The implementation itself is entirely original and written in Rust —
furl shares no code with HTTPie. If you are coming from an HTTPie-based workflow,
[docs/MIGRATION.md](docs/MIGRATION.md) covers the binary-name mapping, environment
variables, and the handful of intentional behavioral deviations.

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you shall be dual-licensed
as above, without any additional terms or conditions.

Author: Karthik Vinayan.
