# Migrating from an HTTPie-compatible workflow

furl implements a command grammar compatible with HTTPie's. If you already know
that grammar, almost everything transfers directly — the request-item separators
(`=`, `:=`, `==`, `:`, `@`, and friends), the flag names, and the URL shorthand
all work the same way. This page covers the few things that are named or spelled
differently, plus the intentional behavioral deviations.

## Command names

| If you used | Use in furl    | Default scheme |
|-------------|----------------|----------------|
| `http`      | `furl`         | `http://`      |
| `https`     | `furls`        | `https://`     |
| the manager | `furl-manager` | —              |

The request grammar is identical, so a command like:

```sh
http POST example.org/api name=widget count:=3 X-Api-Version:2 q==search
```

becomes:

```sh
furl POST example.org/api name=widget count:=3 X-Api-Version:2 q==search
```

## Environment variables

Environment-variable names use the `FURL_` prefix instead of `HTTPIE_`:

| Old prefix   | furl prefix |
|--------------|-------------|
| `HTTPIE_*`   | `FURL_*`    |

## Configuration directory

furl reads its configuration from `~/.config/furl` (following the XDG base
directory convention where applicable).

## User-Agent

Requests are sent with a `furl/<version>` User-Agent by default.

## Intentional deviations

furl diverges from the reference grammar in a small number of deliberate ways.
The full list with rationale is in [DEVIATIONS.md](DEVIATIONS.md). The headline
items:

- **TLS versions.** `--ssl` accepts `tls1.2` and `tls1.3` only. Modern TLS
  backends refuse older versions regardless.
- **Cipher names.** `--ciphers` uses the TLS backend's suite names; the legacy
  OpenSSL cipher-string syntax is not supported.
- **Certificates.** The platform trust store is used by default, so
  system-installed CAs are honored.
- **No update check.** furl never phones home to check for updates on
  invocation.
- **No dynamic plugins.** furl is a compiled binary with built-in auth schemes
  (basic, bearer, digest); there is no plugin-loading mechanism.
- **Cleaner error messages.** A few nested-JSON error and type-name messages are
  corrected to use JSON type names and to avoid crashing on edge-case input.

See [DEVIATIONS.md](DEVIATIONS.md) for the precise behavior in each case.

## Not yet implemented

furl 0.1.0 focuses on request building and sending. The following are recognized
on the command line where applicable but are **not yet functional**, and are
tracked for a future release:

- Colored and pretty-printed output on a terminal (`--pretty`, `--style`) — when
  output is piped, the default is already no formatting, so piped output is
  correct today.
- Request-body compression application (`--compress`).
- File downloads (`--download`, `--continue`).
- Sessions (`--session`, `--session-read-only`).
- Config-file `default_options`.
- `.netrc` credential loading.
- The full online Digest authentication flow.
- `furl-manager` subcommands.

If you rely on one of these, pin to the reference tool for that specific
workflow until the corresponding furl feature lands.
