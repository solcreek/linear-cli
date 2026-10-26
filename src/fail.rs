//! Every way a command can fail, each with its exit code (docs/design.md,
//! "Exit codes"). Add to them; never reuse a code.

use std::fmt;

pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const USAGE: i32 = 2;
pub const PARTIAL: i32 = 3;
pub const BUSY: i32 = 4;
pub const AUTH: i32 = 5;
pub const LIMITED: i32 = 6;
pub const CONFLICT: i32 = 7;

#[derive(Debug)]
pub enum Fail {
    /// Exit 1, the message says what.
    Failed(String),
    /// Exit 1: what was looked for (an issue, a team, a name) is not there.
    NotFound(String),
    /// Exit 1: a name fits more than one thing; the message lists them.
    Ambiguous(String),
    /// Exit 1: Linear could not be reached, timed out, or answered 5xx;
    /// rerunning is safe.
    Network(String),
    /// A mistake in the invocation: exit 2, a short message.
    Usage(String),
    /// A result that is already printed: exit 3.
    Partial(String),
    /// A failure whose document is already printed (api query prints
    /// Linear's answer): exit 1, only the stderr line.
    Reported(String),
    /// Another run holds a lock this one needs; nothing changed (exit 4).
    Busy(String),
    /// No API key, or Linear rejected it (exit 5).
    Auth(String),
    /// Linear's request or complexity budget is spent (exit 6); when it
    /// resets, in Unix milliseconds, if Linear said.
    Limited(String, Option<i64>),
    /// A precondition such as --if-updated-at no longer holds (exit 7).
    Conflict(String),
    /// Linear's own error, in the words it offers people (exit 1).
    Linear { msg: String, not_found: bool },
    /// Another failure, with the next step to take: a command to run, or
    /// whether a rerun is safe. The message is the inner one's, unchanged.
    Hinted(Box<Fail>, String),
}

/// Where to find an issue's identifier when one is not found.
pub const FIND_ISSUE: &str = "'linearctl issue list --query TEXT' finds an issue by the words in it";

/// After a request that may or may not have reached Linear.
pub const RERUN: &str = "rerunning the same command is safe: a change that already applied reports \"changed\": false";

pub type R<T = ()> = Result<T, Fail>;

/// Every error code a failure's JSON carries in "code", with its exit code
/// and what it asks of the caller. describe lists them. Add; never rename.
pub const ERROR_CODES: &[(&str, i32, &str)] = &[
    ("USAGE", USAGE, "the invocation is wrong: fix it, do not retry it as is"),
    ("NOT_FOUND", FAILED, "what was named is not there, or this key cannot see it: look it up with a list command"),
    ("AMBIGUOUS", FAILED, "a name fits more than one thing, listed in error: name it more precisely"),
    ("LINEAR_ERROR", FAILED, "Linear refused the request; error has its words"),
    ("NETWORK", FAILED, "Linear could not be reached, timed out or answered 5xx: rerunning is safe"),
    ("FAILED", FAILED, "anything else that failed; error says what"),
    ("BUSY", BUSY, "another run holds a lock; nothing changed: retry"),
    ("AUTH", AUTH, "no API key, or Linear rejected it"),
    ("RATE_LIMITED", LIMITED, "Linear's budget is spent: retry after retryAt"),
    ("CONFLICT", CONFLICT, "a precondition such as --if-updated-at no longer holds: read again"),
];

impl Fail {
    /// The failure's kind, for the "code" its JSON carries (ERROR_CODES).
    pub fn kind(&self) -> &'static str {
        match self {
            Fail::Usage(_) => "USAGE",
            Fail::NotFound(_) | Fail::Linear { not_found: true, .. } => "NOT_FOUND",
            Fail::Ambiguous(_) => "AMBIGUOUS",
            Fail::Linear { .. } | Fail::Reported(_) => "LINEAR_ERROR",
            Fail::Network(_) => "NETWORK",
            Fail::Failed(_) | Fail::Partial(_) => "FAILED",
            Fail::Busy(_) => "BUSY",
            Fail::Auth(_) => "AUTH",
            Fail::Limited(..) => "RATE_LIMITED",
            Fail::Conflict(_) => "CONFLICT",
            Fail::Hinted(f, _) => f.kind(),
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            Fail::Failed(_)
            | Fail::NotFound(_)
            | Fail::Ambiguous(_)
            | Fail::Network(_)
            | Fail::Linear { .. }
            | Fail::Reported(_) => FAILED,
            Fail::Usage(_) => USAGE,
            Fail::Partial(_) => PARTIAL,
            Fail::Busy(_) => BUSY,
            Fail::Auth(_) => AUTH,
            Fail::Limited(..) => LIMITED,
            Fail::Conflict(_) => CONFLICT,
            Fail::Hinted(f, _) => f.code(),
        }
    }

    /// The same failure, with the next step to take.
    pub fn hint(self, hint: impl Into<String>) -> Fail {
        Fail::Hinted(Box::new(self.base_owned()), hint.into())
    }

    /// The failure without its hint.
    pub fn base(&self) -> &Fail {
        match self {
            Fail::Hinted(f, _) => f.base(),
            f => f,
        }
    }

    fn base_owned(self) -> Fail {
        match self {
            Fail::Hinted(f, _) => f.base_owned(),
            f => f,
        }
    }

    pub fn hint_text(&self) -> Option<&str> {
        match self {
            Fail::Hinted(_, h) => Some(h),
            _ => None,
        }
    }

    /// notFound turns Linear's "Entity not found" into an error naming what
    /// was looked for.
    pub fn not_found(self, what: &str, hint: &str) -> Fail {
        match self {
            Fail::Linear { not_found: true, .. } => {
                Fail::NotFound(format!("not found: {what} (or this API key cannot see it)")).hint(hint)
            }
            other => other,
        }
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Failed(m)
            | Fail::NotFound(m)
            | Fail::Ambiguous(m)
            | Fail::Network(m)
            | Fail::Usage(m)
            | Fail::Partial(m)
            | Fail::Reported(m)
            | Fail::Busy(m)
            | Fail::Auth(m)
            | Fail::Conflict(m) => f.write_str(m),
            Fail::Limited(m, _) => f.write_str(m),
            Fail::Linear { msg, .. } => write!(f, "Linear: {msg}"),
            Fail::Hinted(inner, _) => inner.fmt(f),
        }
    }
}

/// Shorthands for the two most common failures.
pub fn usage(msg: impl Into<String>) -> Fail {
    Fail::Usage(msg.into())
}

pub fn failed(msg: impl Into<String>) -> Fail {
    Fail::Failed(msg.into())
}

pub fn missing(msg: impl Into<String>) -> Fail {
    Fail::NotFound(msg.into())
}

pub fn ambiguous(msg: impl Into<String>) -> Fail {
    Fail::Ambiguous(msg.into())
}

pub fn network(msg: impl Into<String>) -> Fail {
    Fail::Network(msg.into())
}
