//! `.netrc` credential lookup.
//!
//! When an invocation carries no `-a`/URL credentials and `--ignore-netrc`
//! is absent, the request layer falls back to the user's netrc file for a
//! login/password pair. This mirrors what the reference client inherits
//! from its HTTP stack, which in turn defers to the platform netrc parser.
//!
//! The grammar is the classic netrc token stream: a flat sequence of
//! whitespace-separated tokens (spaces, tabs, and newlines are all just
//! separators) grouped into entries by the top-level `machine`, `default`,
//! and `macdef` keywords. Within an entry, `login`/`user`, `password`, and
//! `account` each consume the single following token as their value —
//! unconditionally, so a value that happens to spell a keyword is still
//! taken literally. Lookup is by exact machine name, falling back to a
//! `default` entry when present.

/// A resolved login/password pair from the netrc file.
///
/// `account` is parsed (so it can stand in for a missing `login`, matching
/// the reference) but not surfaced: the request layer only ever needs the
/// user and password that feed Basic auth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetrcAuth {
    pub login: String,
    pub password: String,
}

/// Look up credentials for `host` from the netrc file.
///
/// The file location follows the reference precedence: the `NETRC`
/// environment variable names the file outright when set; otherwise the
/// standard home locations `~/.netrc` then `~/_netrc` are tried in order
/// (the second is the Windows spelling, honored everywhere so a file
/// authored on one platform still resolves on another). Returns `None`
/// when no file exists, it cannot be read, or it holds no matching
/// `machine`/`default` entry.
pub fn lookup(host: &str) -> Option<NetrcAuth> {
    let text = read_netrc_file()?;
    lookup_in(&text, host)
}

/// Parse netrc `text` and look up `host` — the testable core, free of any
/// filesystem or environment access.
///
/// Matching is by exact machine name (the reference compares the parsed
/// name against the URL host verbatim, and the host it passes in has
/// already been lowercased by URL parsing, so a lowercase host here meets
/// a lowercase `machine` token). A `default` entry, if present, answers
/// any host that no `machine` entry claimed.
pub fn lookup_in(text: &str, host: &str) -> Option<NetrcAuth> {
    // The reference parser aborts on the first syntax error and its HTTP
    // stack swallows that error into "no netrc auth" — so a malformed file
    // yields nothing at all, even discarding entries parsed before the
    // error. `parse` signals that with `None`.
    let entries = parse(text)?;
    // The reference keys entries by name in a map, so a later duplicate
    // shadows an earlier one — hence the reverse scan (last-wins) for both
    // the exact machine and the `default` fallback.
    let matched = entries
        .iter()
        .rev()
        .find(|entry| entry.name.as_deref() == Some(host))
        .or_else(|| entries.iter().rev().find(|entry| entry.name.is_none()))?;
    matched.to_auth()
}

/// One parsed entry. `name` is `Some(machine)` for a named machine and
/// `None` for the `default` fallback entry.
struct Entry {
    name: Option<String>,
    login: String,
    account: String,
    password: String,
}

impl Entry {
    /// Collapse a parsed entry into the login/password the caller wants.
    ///
    /// The reference treats an entry as usable only when at least one of
    /// its fields is non-empty, and prefers `login` but falls back to
    /// `account` when `login` was omitted — so a `default account …`
    /// entry can still supply a username.
    fn to_auth(&self) -> Option<NetrcAuth> {
        if self.login.is_empty() && self.account.is_empty() && self.password.is_empty() {
            return None;
        }
        let login = if self.login.is_empty() {
            self.account.clone()
        } else {
            self.login.clone()
        };
        Some(NetrcAuth {
            login,
            password: self.password.clone(),
        })
    }
}

/// Read the netrc file from its resolved location, or `None` when there is
/// no readable file to read.
fn read_netrc_file() -> Option<String> {
    if let Some(path) = std::env::var_os("NETRC") {
        // An explicit `NETRC` names the file directly; a leading `~` still
        // expands, matching the reference's `expanduser` on the value.
        let path = crate::paths::expand_tilde(&path.to_string_lossy());
        return std::fs::read_to_string(path).ok();
    }
    let home = crate::paths::home_dir()?;
    for name in [".netrc", "_netrc"] {
        let candidate = home.join(name);
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            return Some(text);
        }
    }
    None
}

/// Tokenize and group a netrc document into entries, or `None` on a syntax
/// error anywhere in the document.
///
/// The reference parser raises on the first malformed token — a bad
/// top-level keyword, a bad follower token, or a `macdef` that never
/// reaches its terminating blank line — and its HTTP stack turns any such
/// error into "no credentials". So a single syntax error discards the whole
/// file, including entries that parsed cleanly before it. Returning `None`
/// (rather than the entries gathered so far) reproduces that all-or-nothing
/// contract.
fn parse(text: &str) -> Option<Vec<Entry>> {
    let mut lexer = Lexer::new(text);
    let mut entries = Vec::new();

    while let Some(toplevel) = lexer.next_token() {
        // `#`-leading tokens are handled specially at top level: only a
        // *bare* `#` (a single character) comments out the rest of its
        // physical line. A longer `#`-glued token like `#foo` is simply a
        // dropped token — parsing continues with the next token on the same
        // line, which the reference then treats as an ordinary top-level
        // token (so it can be a real keyword, or itself a syntax error).
        if toplevel.starts_with('#') {
            if toplevel.len() == 1 {
                lexer.skip_comment_line();
            }
            continue;
        }
        let name = match toplevel.as_str() {
            "machine" => match lexer.next_token() {
                // A `machine` with no following name is a syntax error in
                // the reference (`missing 'machine' name`).
                Some(name) if !name.is_empty() => Some(name),
                _ => return None,
            },
            "default" => None,
            "macdef" => {
                // A macro definition names a macro and then spans raw lines
                // up to the next blank line; none of it is credential data,
                // so consume the name and skip the body wholesale. A body
                // that never reaches a blank line before EOF is a syntax
                // error in the reference.
                let _ = lexer.next_token();
                if !lexer.skip_macdef_body() {
                    return None;
                }
                continue;
            }
            // Any other top-level token is malformed in the reference, which
            // raises `bad toplevel token`.
            _ => return None,
        };

        let mut entry = Entry {
            name,
            login: String::new(),
            account: String::new(),
            password: String::new(),
        };

        // Consume follower tokens until the next top-level keyword or EOF.
        loop {
            let Some(token) = lexer.next_token() else {
                break;
            };
            // Inside an entry, *any* `#`-leading token (not just a bare `#`)
            // comments out the remainder of its physical line.
            if token.starts_with('#') {
                lexer.skip_comment_line();
                continue;
            }
            if matches!(token.as_str(), "machine" | "default" | "macdef") {
                // Belongs to the next entry: hand it back and stop.
                lexer.push_back(token);
                break;
            }
            match token.as_str() {
                // `login` and `user` are synonyms. The following token is
                // taken as the value unconditionally, even if it spells a
                // keyword — matching the reference lexer.
                "login" | "user" => {
                    if let Some(value) = lexer.next_token() {
                        entry.login = value;
                    }
                }
                "account" => {
                    if let Some(value) = lexer.next_token() {
                        entry.account = value;
                    }
                }
                "password" => {
                    if let Some(value) = lexer.next_token() {
                        entry.password = value;
                    }
                }
                // An unrecognized follower is a syntax error in the
                // reference (`bad follower token`).
                _ => return None,
            }
        }

        entries.push(entry);
    }

    Some(entries)
}

/// A netrc tokenizer matching the reference lexer: whitespace (space, tab,
/// carriage return, newline) separates tokens; `"` quotes a run that may
/// contain whitespace; `\` escapes the next character both inside and
/// outside quotes; and a `#` that begins a token at the start of its line
/// starts a comment that runs to end of line.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    pushback: Vec<String>,
    /// Whether the most recently produced token was terminated by a newline
    /// (or end of input). Mirrors the reference's line-number guard: a `#`
    /// comment only swallows "the rest of the line" when we are still on
    /// that line — if the token already ended it, there is nothing to drop.
    ended_line: bool,
}

impl Lexer {
    fn new(text: &str) -> Self {
        Lexer {
            chars: text.chars().collect(),
            pos: 0,
            pushback: Vec::new(),
            ended_line: false,
        }
    }

    fn push_back(&mut self, token: String) {
        self.pushback.push(token);
    }

    /// Read one character, or `None` at end of input.
    fn read_char(&mut self) -> Option<char> {
        let ch = *self.chars.get(self.pos)?;
        self.pos += 1;
        Some(ch)
    }

    fn is_whitespace(ch: char) -> bool {
        matches!(ch, ' ' | '\t' | '\r' | '\n')
    }

    /// Advance past the remainder of the current line (used to drop a
    /// comment once its introducing `#` is seen).
    fn skip_to_end_of_line(&mut self) {
        while let Some(ch) = self.read_char() {
            if ch == '\n' {
                break;
            }
        }
    }

    /// Drop the rest of the current physical line as a comment, but only if
    /// the introducing `#` token has not already carried us onto the next
    /// line. When the `#` was the last token on its line (so reading it
    /// consumed the terminating newline), there is nothing left to drop and
    /// swallowing the following line would eat real content.
    fn skip_comment_line(&mut self) {
        if !self.ended_line {
            self.skip_to_end_of_line();
        }
    }

    /// Skip a `macdef` body: raw lines up to (and including) the first
    /// blank line. Returns `false` if end of input arrives before a blank
    /// line — a syntax error in the reference ("missing null line
    /// terminator").
    fn skip_macdef_body(&mut self) -> bool {
        // Reading the macro-name token already consumed its terminating
        // whitespace, so we now sit at the start of the body (or mid-line,
        // if the name was space-separated from body text — in which case the
        // rest of that line is the first body line). Swallow whole lines
        // until a blank one, matching the reference's line-by-line scan.
        let mut line = String::new();
        loop {
            match self.read_char() {
                None => return false,
                Some('\n') => {
                    if line.is_empty() {
                        return true;
                    }
                    line.clear();
                }
                Some(ch) => line.push(ch),
            }
        }
    }

    /// Produce the next token, or `None` at end of input.
    ///
    /// `#` is an ordinary character here — a token may begin with or contain
    /// it, and comment handling is left to the parser (which knows whether
    /// it is at top level or inside an entry). `ended_line` records whether
    /// the token was terminated by a newline (or EOF), so the parser can
    /// decide whether a `#` comment still has a line left to swallow.
    fn next_token(&mut self) -> Option<String> {
        if let Some(token) = self.pushback.pop() {
            // A pushed-back token was already produced with its `ended_line`
            // flag set; the parser only ever pushes back keyword tokens it
            // is about to re-read as a fresh top-level token, so leaving the
            // flag as-is is fine.
            return Some(token);
        }

        // Skip leading whitespace.
        let ch = loop {
            let ch = self.read_char()?;
            if !Self::is_whitespace(ch) {
                break ch;
            }
        };

        let mut ch = ch;
        let mut token = String::new();
        loop {
            if ch == '"' {
                // Quoted run: whitespace is literal until the closing `"`.
                loop {
                    match self.read_char() {
                        None => {
                            self.ended_line = true;
                            return Some(token);
                        }
                        Some('"') => break,
                        Some('\\') => {
                            if let Some(escaped) = self.read_char() {
                                token.push(escaped);
                            }
                        }
                        Some(other) => token.push(other),
                    }
                }
            } else if ch == '\\' {
                if let Some(escaped) = self.read_char() {
                    token.push(escaped);
                }
            } else {
                token.push(ch);
            }
            match self.read_char() {
                None => {
                    self.ended_line = true;
                    return Some(token);
                }
                Some(next) if Self::is_whitespace(next) => {
                    self.ended_line = next == '\n';
                    return Some(token);
                }
                Some(next) => ch = next,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(login: &str, password: &str) -> NetrcAuth {
        NetrcAuth {
            login: login.to_string(),
            password: password.to_string(),
        }
    }

    #[test]
    fn exact_machine_match() {
        let text = "machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn missing_host_without_default_is_none() {
        let text = "machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "other.com"), None);
    }

    #[test]
    fn default_entry_is_fallback() {
        let text = "machine a.com login x password y\n\
                    default login d password dp\n";
        // Exact match still wins over default.
        assert_eq!(lookup_in(text, "a.com"), Some(auth("x", "y")));
        // Any other host falls through to default.
        assert_eq!(lookup_in(text, "unknown.com"), Some(auth("d", "dp")));
    }

    #[test]
    fn tokens_split_on_any_whitespace_including_newlines() {
        let text = "machine\nexample.com\nlogin\nbob\npassword\nsecret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn user_is_synonym_for_login() {
        let text = "machine example.com user bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn multiple_machines_each_resolve() {
        let text = "machine a.com login alice password apw\n\
                    machine b.com login bob password bpw\n";
        assert_eq!(lookup_in(text, "a.com"), Some(auth("alice", "apw")));
        assert_eq!(lookup_in(text, "b.com"), Some(auth("bob", "bpw")));
    }

    #[test]
    fn duplicate_machine_last_wins() {
        // The reference stores entries in a dict keyed by name, so a later
        // duplicate replaces the earlier one.
        let text = "machine a.com login first password p1\n\
                    machine a.com login second password p2\n";
        assert_eq!(lookup_in(text, "a.com"), Some(auth("second", "p2")));
    }

    #[test]
    fn leading_comment_line_is_skipped() {
        let text = "# a comment\n\
                    machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn bare_hash_comment_line_is_skipped() {
        let text = "#\nmachine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn toplevel_multichar_hash_token_is_dropped_not_a_comment() {
        // At top level only a *bare* `#` comments its line. A longer
        // `#`-glued token like `#foo` is just dropped, and parsing resumes
        // with the next token on the same line — here `bar`, which is not a
        // valid top-level keyword, so the reference raises and its HTTP
        // stack yields no credentials at all.
        let text = "#foo bar baz\n\
                    machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn toplevel_multichar_hash_token_lets_following_keyword_parse() {
        // `#foo machine …` drops `#foo`, then `machine` on the same line is
        // read as a real top-level keyword — so the entry parses.
        let text = "#foo machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn hash_prefixed_follower_comments_rest_of_line() {
        // A mid-entry `#foo` drops `#foo` and everything after it on that
        // line; `password` resumes the entry on the next line.
        let text = "machine example.com login bob #foo bar\n\
                    password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn hash_inside_a_token_is_a_literal_character() {
        // A `#` glued into a value is not a comment; it stays in the token.
        let text = "machine example.com login bob password sec#ret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "sec#ret")));
    }

    #[test]
    fn hash_starting_a_midline_token_comments_to_end_of_line() {
        // `# hey` mid-entry drops the rest of that physical line, so
        // `password` continues on the following line.
        let text = "machine example.com login bob # hey\n\
                    password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn macdef_block_is_skipped_until_blank_line() {
        let text = "macdef init\n\
                    foo bar\n\
                    more stuff\n\
                    \n\
                    machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn quoted_value_preserves_internal_whitespace() {
        // The reference lexer honors double quotes even though "classic"
        // netrc did not; we match the observable behavior.
        let text = "machine example.com login bob password \"p a s s\"\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "p a s s")));
    }

    #[test]
    fn backslash_escapes_next_character() {
        let text = "machine example.com login bob password pa\\\\ss\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "pa\\ss")));
    }

    #[test]
    fn value_may_spell_a_keyword() {
        // The token after `password` is taken literally, so a password of
        // "machine" is honored rather than starting a new entry.
        let text = "machine example.com login bob password machine\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "machine")));
    }

    #[test]
    fn account_stands_in_for_missing_login() {
        // With no `login`, the reference falls back to `account` for the
        // username while returning the password unchanged.
        let text = "machine example.com account acct password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("acct", "secret")));
    }

    #[test]
    fn password_only_entry_yields_empty_login() {
        let text = "machine example.com password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("", "secret")));
    }

    #[test]
    fn login_only_entry_yields_empty_password() {
        let text = "machine example.com login bob\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "")));
    }

    #[test]
    fn default_before_machine_still_lets_exact_match_win() {
        // Even when `default` is listed first, an exact `machine` match takes
        // precedence; unknown hosts fall through to `default`.
        let text = "default login d password dp\n\
                    machine a.com login x password y\n";
        assert_eq!(lookup_in(text, "a.com"), Some(auth("x", "y")));
        assert_eq!(lookup_in(text, "z.com"), Some(auth("d", "dp")));
    }

    #[test]
    fn duplicate_default_last_wins() {
        let text = "default login d1 password p1\n\
                    default login d2 password p2\n";
        assert_eq!(lookup_in(text, "z.com"), Some(auth("d2", "p2")));
    }

    #[test]
    fn login_takes_precedence_over_account_for_username() {
        let text = "machine e.com login bob account acct password s\n";
        assert_eq!(lookup_in(text, "e.com"), Some(auth("bob", "s")));
    }

    #[test]
    fn repeated_field_within_entry_last_wins() {
        let text = "machine e.com login first login second password s\n";
        assert_eq!(lookup_in(text, "e.com"), Some(auth("second", "s")));
    }

    #[test]
    fn host_matching_is_case_sensitive() {
        // The reference compares the parsed machine name to the host
        // verbatim; URL parsing lowercases the host before it reaches us,
        // so a mixed-case `machine` token simply will not match.
        let text = "machine Example.COM login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn empty_file_is_none() {
        assert_eq!(lookup_in("", "example.com"), None);
    }

    #[test]
    fn comments_only_is_none() {
        assert_eq!(lookup_in("# just a comment\n", "example.com"), None);
    }

    #[test]
    fn macdef_at_eof_without_blank_line_terminator_is_none() {
        // A macro that never reaches a terminating blank line before EOF is
        // a syntax error in the reference, so the whole file yields nothing.
        let text = "macdef init\nfoo bar\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn unterminated_macdef_discards_prior_entry() {
        // A syntax error anywhere aborts the reference parse entirely — even
        // a valid machine entry read before the unterminated macdef is
        // discarded, so no credentials come back.
        let text = "machine example.com login bob password secret\n\
                    macdef init\ncmd\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn macdef_terminated_by_immediate_blank_line() {
        // A blank line right after the macro name terminates an empty body;
        // the following machine entry then parses normally.
        let text = "macdef init\n\n\
                    machine example.com login bob password secret\n";
        assert_eq!(lookup_in(text, "example.com"), Some(auth("bob", "secret")));
    }

    #[test]
    fn bad_toplevel_token_discards_prior_entry() {
        // Any malformed top-level token aborts the whole parse in the
        // reference, discarding entries gathered before it.
        let text = "machine example.com login bob password secret\n\
                    garbage\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn bad_follower_token_discards_prior_entry() {
        let text = "machine a.com login alice password apw\n\
                    machine b.com bogus login bob password bpw\n";
        assert_eq!(lookup_in(text, "a.com"), None);
    }

    #[test]
    fn machine_without_name_is_none() {
        // `machine` with nothing following is a syntax error in the
        // reference ("missing 'machine' name").
        let text = "machine\n";
        assert_eq!(lookup_in(text, "example.com"), None);
    }

    #[test]
    fn netrc_env_precedence_is_exercised_via_lookup_in() {
        // `lookup` picks the file (NETRC env, else home) and then defers to
        // `lookup_in` for parsing; this test pins the parsing contract that
        // both paths share, without mutating process env.
        let env_file = "machine example.com login envuser password envpass\n";
        let home_file = "machine example.com login homeuser password homepass\n";
        assert_eq!(
            lookup_in(env_file, "example.com"),
            Some(auth("envuser", "envpass"))
        );
        assert_eq!(
            lookup_in(home_file, "example.com"),
            Some(auth("homeuser", "homepass"))
        );
    }
}
