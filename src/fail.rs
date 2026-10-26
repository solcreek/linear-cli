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
    /// A mistake in the invocation: exit 2, a short message.
    Usage(String),
    /// A result that is already printed: exit 3.
    Partial(String),
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
}

pub type R<T = ()> = Result<T, Fail>;

impl Fail {
    pub fn code(&self) -> i32 {
        match self {
            Fail::Failed(_) | Fail::Linear { .. } => FAILED,
            Fail::Usage(_) => USAGE,
            Fail::Partial(_) => PARTIAL,
            Fail::Busy(_) => BUSY,
            Fail::Auth(_) => AUTH,
            Fail::Limited(..) => LIMITED,
            Fail::Conflict(_) => CONFLICT,
        }
    }

    /// notFound turns Linear's "Entity not found" into an error naming what
    /// was looked for.
    pub fn not_found(self, what: &str) -> Fail {
        match self {
            Fail::Linear { not_found: true, .. } => {
                Fail::Failed(format!("not found: {what} (or this API key cannot see it)"))
            }
            other => other,
        }
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Fail::Failed(m) | Fail::Usage(m) | Fail::Partial(m) | Fail::Busy(m) | Fail::Auth(m) | Fail::Conflict(m) => {
                f.write_str(m)
            }
            Fail::Limited(m, _) => f.write_str(m),
            Fail::Linear { msg, .. } => write!(f, "Linear: {msg}"),
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
