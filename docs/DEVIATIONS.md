# Intentional deviations

furl aims for behavioral parity with a widely used, HTTPie-compatible command
grammar. Where it intentionally differs, the reasoning is recorded here so the
difference is a documented choice rather than a surprise. Deviations fall into
three groups: bug-fixes (furl is correct where the reference grammar behaved
incorrectly), structural/environment differences, and features still under
construction.

## Bug-fixes

furl corrects a handful of long-standing edge-case behaviors:

- **Trailing backslash in nested-JSON keys.** A lone trailing backslash in a
  data key is kept as a literal backslash instead of aborting. Double literal
  backslashes are also handled correctly on the error path.
- **Descending through an explicit `null` in nested JSON.** furl replaces the
  `null` in place rather than discarding everything built so far. The reference
  behavior reset the whole context on this path.
- **JSON type-error messages.** Type mismatches are reported with JSON type
  names (`boolean`, `null`, `number`) instead of leaking a host language's
  internal type names.
- **Platform certificate trust.** System/OS certificates are used by default via
  the platform trust store, so system-installed CAs are honored.
- **No update check on invocation.** furl never contacts the network to check
  for updates; update checks are simply not present.

## Structural and environment differences

- **Branding and names.** The client binaries are `furl` (HTTP) and `furls`
  (HTTPS); the maintenance CLI is `furl-manager`. Environment variables use the
  `FURL_` prefix, configuration lives in `~/.config/furl`, and requests carry a
  `furl/<version>` User-Agent.
- **TLS protocol versions.** `--ssl` accepts `tls1.2` and `tls1.3` only. Modern
  TLS backends refuse older protocol versions regardless of what is requested.
- **Cipher suite names.** `--ciphers` maps to the TLS backend's cipher-suite
  names (rustls). The legacy OpenSSL cipher-string syntax is not supported.
- **No dynamic plugins.** furl is a single compiled binary. Only the built-in
  auth schemes (basic, bearer, digest) are available; there is no runtime
  plugin-loading mechanism.
- **ASCII punctuation in messages.** A few messages that used non-ASCII
  apostrophes now use plain ASCII.
- **Authorization header wire order.** When credentials are supplied *and* a
  raw `Authorization:` header is also given (an unusual combination), the
  computed `Authorization` header may appear in a slightly different position
  relative to `Content-Length`. Header order does not affect how any server
  interprets the request; the header values are identical.
- **Compression output.** `--compress` produces a valid RFC 1950 zlib stream
  and honors the `-x` skip-if-not-smaller / `-xx` force semantics, but the exact
  compressed bytes (and therefore the `Content-Length`) differ from other
  implementations because furl uses a pure-Rust DEFLATE encoder. Any server
  decodes furl's body to the identical payload.

## Under construction

These are recognized where applicable but not yet at parity. They are tracked and
intended for a future release:

- **Output colors and formatting** — `--pretty`, `--style`, JSON/XML
  reformatting, header sorting, binary-response conversion. Note: when output is
  piped the default is no formatting, so piped output is already byte-correct;
  the gap is terminal formatting only.
- **Downloads** — `--download`, `--continue`.
- **Sessions** — `--session`, `--session-read-only`.
- **Config-file `default_options`.**
- **`.netrc` credential loading.**
- **Full online Digest authentication flow.**
- **`furl-manager` subcommands** and man pages.
