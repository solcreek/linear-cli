//! cycle show, create, update and archive, beside the cycle list lookups
//! makes.

use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::client::Client;
use crate::fail::{R, failed, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::Run;
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

/// cycle show's fields: Linear's, under Linear's names.
const SHOW_FIELDS: &[&str] = &[
    "id",
    "number",
    "name",
    "description",
    "startsAt",
    "endsAt",
    "completedAt",
    "isActive",
    "isNext",
    "isPrevious",
    "isPast",
    "isFuture",
    "progress",
    "team",
    "createdAt",
    "updatedAt",
    "archivedAt",
    "autoArchivedAt",
    "currentProgress",
    "progressHistory",
    "scopeHistory",
    "completedScopeHistory",
    "inProgressScopeHistory",
    "issueCountHistory",
    "completedIssueCountHistory",
    "inheritedFrom",
    "issues",
    "uncompletedIssuesUponClose",
];

/// show (and the changes), added to the cycle command cycle list made.
pub fn commands() -> Vec<Cmd> {
    vec![show_cmd()]
}

/// The filter for a CYCLE argument within --team (or the default team), when
/// one is set: a number is unique only within a team, and current, next and
/// previous are per team.
fn cycle_filter(ctx: &Ctx, word: &str) -> R<Value> {
    let mut f = cycle_ref("CYCLE", word)?;
    let team = team_flag(ctx)?;
    if !team.is_empty() {
        f["team"] = json!({"key": {"eqIgnoreCase": team}});
    }
    Ok(f)
}

/// The one cycle the filter finds: none is not found, and two (the same
/// word in two teams) is an error naming them.
fn find_cycle(run: &mut Run, cl: &Client, word: &str, vars: Value) -> R<Value> {
    let data = cl.query(run, queries::CYCLE_SHOW, vars)?;
    let n = super::issue::nodes(&data["matches"]);
    match n.len() {
        0 => Err(failed(format!("not found: cycle {word} (or this API key cannot see it)"))),
        1 => super::issue::nodes(&data["cycles"])
            .into_iter()
            .next()
            .ok_or_else(|| failed(format!("not found: cycle {word} (or this API key cannot see it)"))),
        _ => Err(failed(format!(
            "cycle {} is ambiguous: teams {} and {} both have one; pass --team",
            quote_go(word),
            str_at(&n[0], "team.key"),
            str_at(&n[1], "team.key")
        ))),
    }
}

/// A connection as the list shape (items, total, shown, truncated,
/// nextCursor); next is where --cursor continues, when it applies.
fn nested_list(conn: &Value, paged: bool) -> Value {
    let items: Vec<Value> = conn["nodes"].as_array().cloned().unwrap_or_default().into_iter().map(flatten).collect();
    let more = conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
    let next = if more && paged { optional(conn["pageInfo"]["endCursor"].as_str().unwrap_or("")) } else { Value::Null };
    json!({"items": items, "total": null, "shown": items.len(), "truncated": more, "nextCursor": next})
}

fn show_cmd() -> Cmd {
    let c = Cmd::new("show CYCLE", "Show a cycle: its progress, histories and issues, as Linear computes them")
        .long(
            "Show a cycle with Linear's own fields: dates, the is* states, progress (completed\n\
points plus a quarter of in-progress ones, over all points), currentProgress, and the\n\
day-by-day histories behind Linear's burndown charts. Nothing is computed here.\n\
CYCLE is current, next, previous, a cycle number, or its UUID; all but a UUID are\n\
per team, so --team (or LINEARCTL_TEAM, or default-team) picks it, and a word that\n\
fits cycles in two teams is an error naming both.\n\
issues and uncompletedIssuesUponClose come as lists: --cursor continues issues.",
        )
        .example(
            "  linearctl cycle show current --team ENG --json\n  linearctl cycle show 23 --team ENG --issues 250 --fields number,progress,issues --json",
        )
        .args(Args::Exact(1))
        .run(show)
        .flag(Flag::str("team", "the cycle's team `KEY` (falls back to LINEARCTL_TEAM, then default-team)"))
        .flag(Flag::whole("issues", 50, "include the first `N` issues; 0 for none"))
        .flag(Flag::whole("uncompleted", 50, "include the first `N` issues left open at close; 0 for none"))
        .flag(cursor_flag());
    fields_flag_spec(c, SHOW_FIELDS)
}

fn show(run: &mut Run, ctx: &Ctx) -> R {
    let word = ctx.args[0].clone();
    let filter = cycle_filter(ctx, &word)?;
    let fields = fields_flag(ctx, SHOW_FIELDS)?;
    let cursor = cursor_value(ctx)?;
    let (issues, uncompleted) = (ctx.whole("issues"), ctx.whole("uncompleted"));
    if issues > 250 || uncompleted > 250 {
        return Err(usage("--issues and --uncompleted are at most 250; page issues with --cursor"));
    }
    let cl = run.client()?;
    let vars = json!({"filter": filter, "issues": issues, "withIssues": issues > 0, "after": optional(&cursor),
        "uncompleted": uncompleted, "withUncompleted": uncompleted > 0});
    let mut c = find_cycle(run, &cl, &word, vars)?;
    for (k, paged) in [("issues", true), ("uncompletedIssuesUponClose", false)] {
        if c.get(k).is_some_and(|v| !v.is_null()) {
            c[k] = nested_list(&c[k], paged);
        }
    }
    let c = flatten(c);
    if let Some(next) = c["issues"]["nextCursor"].as_str() {
        run.eprint(&format!("{} issues shown, more remain; --cursor {next} for the next page\n", c["issues"]["shown"]));
    }
    if run.json {
        return run.print_json(json!({"cycle": pick(c, &fields)}));
    }
    let name = str_at(&c, "name");
    let title = if name.is_empty() { format!("Cycle {}", str_at(&c, "number")) } else { name };
    let state = ["isActive", "isNext", "isPrevious", "isFuture", "isPast"]
        .iter()
        .find(|k| c[**k].as_bool() == Some(true))
        .map_or("", |k| match *k {
            "isActive" => "current",
            "isNext" => "next",
            "isPrevious" => "previous",
            "isFuture" => "future",
            _ => "past",
        });
    let mut s = format!(
        "{}  {}  ({})\n{} – {}   progress {}\n",
        printable(&title),
        str_at(&c, "team.key"),
        state,
        str_at(&c, "startsAt"),
        str_at(&c, "endsAt"),
        str_at(&c, "progress")
    );
    let d = str_at(&c, "description");
    if !d.is_empty() {
        s.push_str(&format!("\n{}\n", printable_text(&d)));
    }
    run.print(&s)?;
    if let Some(is) = c["issues"]["items"].as_array().filter(|i| !i.is_empty()) {
        run.print(&format!("\n{} issues\n", is.len()))?;
        rows(run, is, &["identifier", "state.name", "assignee.name", "title"])?;
    }
    Ok(())
}
