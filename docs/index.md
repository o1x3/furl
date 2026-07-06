# furl documentation

furl is a human-friendly command-line HTTP client. It ships three binaries:
`furl` (defaults to `http://`), `furls` (defaults to `https://`), and
`furl-manager` (maintenance CLI).

For installation and a quickstart, see the top-level [README](../README.md). Run
`furl --help` for the full flag list or `furl --manual` for the long-form manual.

## Guides

- [MIGRATION.md](MIGRATION.md) — coming from an HTTPie-compatible workflow:
  command-name mapping, environment variables, config directory, and intentional
  deviations.
- [DEVIATIONS.md](DEVIATIONS.md) — where furl intentionally differs, with
  rationale, organized into bug-fixes, structural differences, and
  under-construction features.

## Request items

Arguments after the URL are *request items*; the separator inside each token
decides its role. The ten separators:

| Item             | Separator | Meaning                                              |
|------------------|-----------|------------------------------------------------------|
| `name=value`     | `=`       | Data field (JSON string, or form field with `-f`)    |
| `name:=value`    | `:=`      | Raw-JSON data field                                  |
| `name=@file`     | `=@`      | Data field, value from file                          |
| `name:=@file`    | `:=@`     | Raw-JSON data field, value from file                 |
| `Name:value`     | `:`       | HTTP header                                          |
| `Name;`          | `;`       | HTTP header with an empty value                      |
| `Name:@file`     | `:@`      | HTTP header, value from file                         |
| `name==value`    | `==`      | URL query parameter                                  |
| `name==@file`    | `==@`     | Query parameter, value from file                     |
| `field@file`     | `@`       | File-upload form field (implies multipart)           |

## Flag groups

`furl --help` organizes flags into these groups:

- **Predefined content types** — choose the request body shape: `-j`/`--json`
  (the default), `-f`/`--form`, `--multipart`, `--boundary`, `--raw`.
- **Content processing** — `-x`/`--compress` (under construction).
- **Output processing** — control how output is rendered: `--pretty`,
  `-s`/`--style`, `--sorted`/`--unsorted`, `--response-charset`,
  `--response-mime`, `--format-options`. (Terminal formatting is under
  construction; piped output is unformatted by default.)
- **Output options** — choose what to print: `-p`/`--print`, `-h`/`--headers`,
  `-m`/`--meta`, `-b`/`--body`, `-v`/`--verbose`, `--all`, `-S`/`--stream`,
  `-o`/`--output`, `-d`/`--download`, `-c`/`--continue`, `-q`/`--quiet`.
- **Sessions** — `--session`, `--session-read-only` (under construction).
- **Authentication** — `-a`/`--auth`, `-A`/`--auth-type` (`basic`, `bearer`,
  `digest`), `--ignore-netrc`.
- **Network** — `--offline`, `--proxy`, `-F`/`--follow`, `--max-redirects`,
  `--max-headers`, `--timeout`, `--check-status`, `--path-as-is`, `--chunked`.
- **SSL** — `--verify` (on by default), `--ssl` (`tls1.2`/`tls1.3`),
  `--ciphers`, `--cert`, `--cert-key`, `--cert-key-pass`.
- **Troubleshooting** — `-I`/`--ignore-stdin`, `--help`, `--manual`,
  `--version`, `--traceback`, `--default-scheme`, `--debug`.

See [DEVIATIONS.md](DEVIATIONS.md) for which of these are not yet fully
implemented.
