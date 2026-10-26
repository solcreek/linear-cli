//! cycle show, create, update and archive, beside the cycle list lookups
//! makes.

use crate::fail::{R, usage};
use crate::json::quote_go;
use crate::util::*;
use serde_json::{Value, json};

/// The words that name a cycle, here and in issue list --cycle.
pub const CYCLE_WORDS: &str = "current, next, previous, a cycle number, or its UUID";

/// The cycle a CYCLE word names, as Linear's CycleFilter (and
/// NullableCycleFilter, which has the same fields) takes it. A number is unique
/// only within a team, so callers add the team.
pub fn cycle_ref(what: &str, s: &str) -> R<Value> {
    plain(what, s)?;
    Ok(match s {
        "current" => json!({"isActive": {"eq": true}}),
        "next" => json!({"isNext": {"eq": true}}),
        "previous" => json!({"isPrevious": {"eq": true}}),
        n if !n.is_empty() && n.len() <= 9 && n.bytes().all(|c| c.is_ascii_digit()) => {
            json!({"number": {"eq": n.parse::<i64>().unwrap_or(0)}})
        }
        id if is_uuid(id) => json!({"id": {"eq": id.to_lowercase()}}),
        _ => return Err(usage(format!("{what} must be {CYCLE_WORDS}; got {}", quote_go(s)))),
    })
}
