//! The argument parse engine.
//!
//! A two-phase parse: the main scan consumes recognized options and
//! collects positionals plus unrecognized option-like tokens; a second
//! sweep resolves `--no-OPTION` leftovers (which always win, whatever
//! their position — that is what lets command-line flags cancel
//! config-supplied defaults). Long options may be abbreviated to any
//! unambiguous prefix; negations require exact names.

use super::args::ParsedArgs;
use super::options::{self, Action, OptId, OptionSpec};

/// What an argv parse produced.
#[derive(Debug)]
pub enum Outcome {
    Args(Box<ParsedArgs>),
    Help,
    Manual,
    Version,
}

/// A usage error: message plus the option to blame in the usage line
/// (when the error came from a specific known option).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError {
    pub message: String,
    pub option: Option<String>,
}

impl UsageError {
    fn plain(message: impl Into<String>) -> UsageError {
        UsageError {
            message: message.into(),
            option: None,
        }
    }

    fn for_option(spec: &OptionSpec, detail: &str) -> UsageError {
        UsageError {
            message: format!("argument {}: {detail}", spec.display_name()),
            option: Some(spec.display_name()),
        }
    }
}

/// Does this token start an option? Lone `-`, `--`, and tokens that read
/// as negative numbers are positionals.
fn is_option_token(token: &str) -> bool {
    if token == "-" || token == "--" || !token.starts_with('-') {
        return false;
    }
    !is_negative_number(token)
}

/// `-123` and `-12.5` style tokens (no registered option looks like a
/// negative number, so they always mean data).
fn is_negative_number(token: &str) -> bool {
    let rest = &token[1..];
    if rest.is_empty() {
        return false;
    }
    if rest.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if let Some((int_part, frac_part)) = rest.split_once('.') {
        return !frac_part.is_empty()
            && int_part.chars().all(|c| c.is_ascii_digit())
            && frac_part.chars().all(|c| c.is_ascii_digit());
    }
    false
}

struct Parser<'a> {
    argv: &'a [String],
    at: usize,
    args: ParsedArgs,
    positionals: Vec<String>,
    leftovers: Vec<String>,
    /// Which of the mutually exclusive session options was seen first.
    session_seen: Option<&'static str>,
}

pub fn parse(argv: &[String]) -> Result<Outcome, UsageError> {
    let mut parser = Parser {
        argv,
        at: 0,
        args: ParsedArgs::default(),
        positionals: Vec::new(),
        leftovers: Vec::new(),
        session_seen: None,
    };
    if let Some(terminal) = parser.scan()? {
        return Ok(terminal);
    }
    parser.sweep_negations()?;
    parser.assign_positionals()?;
    Ok(Outcome::Args(Box::new(parser.args)))
}

impl Parser<'_> {
    /// Consume the whole command line; a `Some` return means a terminal
    /// action (help/version/manual) cut the parse short.
    fn scan(&mut self) -> Result<Option<Outcome>, UsageError> {
        let mut positionals_only = false;
        while self.at < self.argv.len() {
            let token = self.argv[self.at].clone();
            self.at += 1;
            if positionals_only {
                self.positionals.push(token);
            } else if token == "--" {
                positionals_only = true;
            } else if is_option_token(&token) {
                if let Some(outcome) = self.handle_option(&token)? {
                    return Ok(Some(outcome));
                }
            } else {
                self.positionals.push(token);
            }
        }
        Ok(None)
    }

    /// Handle one option token; a `Some` return is a terminal action
    /// (help/version/manual), smuggled out through the error path.
    fn handle_option(&mut self, token: &str) -> Result<Option<Outcome>, UsageError> {
        if token.starts_with("--") {
            self.handle_long(token)
        } else {
            self.handle_short_cluster(token)
        }
    }

    fn handle_long(&mut self, token: &str) -> Result<Option<Outcome>, UsageError> {
        let (name, explicit) = match token.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (token, None),
        };
        let spec = match options::find_long_prefix(name) {
            Ok(Some(spec)) => spec,
            Ok(None) => {
                self.leftovers.push(token.to_string());
                return Ok(None);
            }
            Err(candidates) => {
                return Err(UsageError::plain(format!(
                    "ambiguous option: {name} could match {}",
                    candidates.join(", ")
                )));
            }
        };
        self.apply(spec, explicit)
    }

    fn handle_short_cluster(&mut self, token: &str) -> Result<Option<Outcome>, UsageError> {
        // `-o=value` names the option before the first `=`.
        if let Some((name, value)) = token.split_once('=') {
            if let Some(spec) = options::find_exact(name) {
                return self.apply(spec, Some(value.to_string()));
            }
        }
        let chars: Vec<char> = token.chars().collect();
        let mut i = 1;
        while i < chars.len() {
            let alias: String = format!("-{}", chars[i]);
            let Some(spec) = options::find_exact(&alias) else {
                let rest: String = chars[i..].iter().collect();
                self.leftovers.push(format!("-{rest}"));
                return Ok(None);
            };
            if spec.takes_value() {
                let attached: String = chars[i + 1..].iter().collect();
                let value = if attached.is_empty() {
                    None
                } else {
                    Some(attached)
                };
                return self.apply(spec, value);
            }
            if let Some(outcome) = self.apply(spec, None)? {
                return Ok(Some(outcome));
            }
            i += 1;
        }
        Ok(None)
    }

    /// Apply one occurrence, fetching the value from argv when the
    /// option needs one and none was attached.
    fn apply(
        &mut self,
        spec: &'static OptionSpec,
        explicit: Option<String>,
    ) -> Result<Option<Outcome>, UsageError> {
        match spec.action {
            Action::Terminal => {
                let outcome = match spec.id {
                    OptId::Help => Outcome::Help,
                    OptId::Manual => Outcome::Manual,
                    OptId::Version => Outcome::Version,
                    other => unreachable!("{other:?} is not terminal"),
                };
                Ok(Some(outcome))
            }
            Action::Flag | Action::Count => {
                if let Some(value) = explicit {
                    return Err(UsageError::for_option(
                        spec,
                        &format!("ignored explicit argument '{value}'"),
                    ));
                }
                self.check_session_group(spec)?;
                self.args.apply_flag(spec, None);
                Ok(None)
            }
            Action::AppendConst(constant) => {
                if let Some(value) = explicit {
                    return Err(UsageError::for_option(
                        spec,
                        &format!("ignored explicit argument '{value}'"),
                    ));
                }
                self.args.apply_flag(spec, Some(constant));
                Ok(None)
            }
            Action::Store | Action::Append => {
                let value = match explicit {
                    Some(value) => value,
                    None => self.take_value(spec)?,
                };
                self.check_session_group(spec)?;
                self.args
                    .apply_value(spec, &value)
                    .map_err(|detail| UsageError::for_option(spec, &detail))?;
                Ok(None)
            }
        }
    }

    /// The next argv token serves as the value unless it looks like an
    /// option itself.
    fn take_value(&mut self, spec: &OptionSpec) -> Result<String, UsageError> {
        match self.argv.get(self.at) {
            Some(next) if !is_option_token(next) && next != "--" => {
                self.at += 1;
                Ok(next.clone())
            }
            _ => Err(UsageError::for_option(spec, "expected one argument")),
        }
    }

    /// `--session` and `--session-read-only` are mutually exclusive; the
    /// clash is reported the moment the second one appears.
    fn check_session_group(&mut self, spec: &OptionSpec) -> Result<(), UsageError> {
        let name = match spec.id {
            OptId::Session => "--session",
            OptId::SessionReadOnly => "--session-read-only",
            _ => return Ok(()),
        };
        if let Some(previous) = self.session_seen {
            if previous != name {
                return Err(UsageError::for_option(
                    spec,
                    &format!("not allowed with argument {previous}"),
                ));
            }
        }
        self.session_seen = Some(name);
        Ok(())
    }

    /// Resolve leftovers: every one must be a `--no-OPTION` naming a real
    /// option (exact long alias, never abbreviated); anything else is an
    /// unrecognized argument. Negation resets the destination to its
    /// default, whatever the relative order was.
    fn sweep_negations(&mut self) -> Result<(), UsageError> {
        let mut invalid = Vec::new();
        for token in &self.leftovers {
            let target = token
                .strip_prefix("--no-")
                .map(|base| format!("--{base}"))
                .and_then(|inverted| options::find_negation_target(&inverted));
            match target {
                Some(spec) => self.args.reset(spec.id),
                None => invalid.push(token.clone()),
            }
        }
        if invalid.is_empty() {
            Ok(())
        } else {
            Err(UsageError::plain(format!(
                "unrecognized arguments: {}",
                invalid.join(" ")
            )))
        }
    }

    /// One positional is the URL; two or more fill METHOD then URL then
    /// items. Positionals are pooled across the whole command line, so
    /// options standing between them cannot skew the assignment.
    fn assign_positionals(&mut self) -> Result<(), UsageError> {
        let mut positionals = std::mem::take(&mut self.positionals).into_iter();
        match (positionals.next(), positionals.next()) {
            (None, _) => Err(UsageError::plain(
                "the following arguments are required: URL",
            )),
            (Some(url), None) => {
                self.args.url = url;
                Ok(())
            }
            (Some(method), Some(url)) => {
                self.args.method = Some(method);
                self.args.url = url;
                for item in positionals {
                    ParsedArgs::validate_item_token(&item).map_err(|detail| UsageError {
                        message: format!("argument REQUEST_ITEM: {detail}"),
                        option: Some("REQUEST_ITEM".to_string()),
                    })?;
                    self.args.request_items.push(item);
                }
                Ok(())
            }
        }
    }
}
