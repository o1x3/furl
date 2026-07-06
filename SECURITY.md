# Security Policy

## Supported versions

furl is pre-1.0. Security fixes are released against the latest published
version. Until 1.0, only the most recent `0.x` release line receives fixes.

| Version | Supported |
|---------|-----------|
| 0.1.x   | Yes       |
| < 0.1   | No        |

## Reporting a vulnerability

Please report suspected vulnerabilities **privately**. Do not open a public
issue for security problems.

Email **security@o1x3.com** with:

- a description of the issue and its impact,
- steps to reproduce (a minimal command line is ideal),
- the furl version (`furl --version`) and your platform.

We aim to acknowledge reports promptly and will coordinate a fix and disclosure
timeline with you. Please give us a reasonable window to release a fix before any
public disclosure.

## Security posture

furl is designed to be safe by default:

- **TLS verification is on by default.** Server certificates are validated
  against the platform trust store. Verification is only relaxed when you
  explicitly pass `--verify no`, and doing so is your decision to make per
  invocation.
- **No telemetry, no phone-home.** furl does not report usage anywhere and
  performs no background update check on invocation. Nothing contacts the
  network except the request you asked for.
- **Cookies are origin-scoped across redirects.** When following redirects,
  cookies set for one origin are not forwarded to a different origin, closing a
  well-known class of cross-origin cookie-leak issues.
- **Credential handling.** With `--auth`, supplying a username without a
  password prompts for it interactively rather than requiring it on the command
  line (where it could land in shell history or the process table). Reading
  credentials from `.netrc` can be disabled with `--ignore-netrc`.
- **Memory safety.** The crate is written in Rust and denies `unsafe` code.

furl also inherits protections against common HTTP-client attack classes,
including header/response-splitting via untrusted input, uncontrolled redirect
chains (bounded by `--max-redirects`), and cross-origin credential or cookie
leakage on redirect. If you believe any of these protections can be bypassed,
please report it as described above.
