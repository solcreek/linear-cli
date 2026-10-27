//! cycle show, create, update and archive, beside the cycle list lookups
//! makes.

use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::client::Client;
use crate::fail::{Fail, R, ambiguous, missing, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::{Run, dry_run};
use crate::time;
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
    vec![show_cmd(), create_cmd(), update_cmd(), archive_cmd()]
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

/// A cycle that is not there, and where to see the ones that are.
fn cycle_not_found(word: &str) -> Fail {
    missing(format!("not found: cycle {word} (or this API key cannot see it)"))
        .with(sought("cycle", word))
        .hint("'linearctl cycle list --team KEY' lists a team's cycles")
}

/// A cycle word two teams' cycles fit, naming them.
fn cycle_ambiguous(word: &str, n: &[Value]) -> Fail {
    ambiguous(format!(
        "cycle {} is ambiguous: teams {} and {} both have one; pass --team",
        quote_go(word),
        str_at(&n[0], "team.key"),
        str_at(&n[1], "team.key")
    ))
    .with(sought("cycle", word))
    .with(json!({"candidates": candidates(n)}))
}

/// The one cycle the filter finds: none is not found, and two (the same
/// word in two teams) is an error naming them.
fn find_cycle(run: &mut Run, cl: &Client, word: &str, vars: Value) -> R<Value> {
    let mut data = cl.query(run, queries::CYCLE_SHOW, vars)?;
    super::issue::complete_labels(run, cl, &mut data)?;
    let n = super::issue::nodes(&data["matches"]);
    match n.len() {
        0 => Err(cycle_not_found(word)),
        1 => super::issue::nodes(&data["cycles"]).into_iter().next().ok_or_else(|| cycle_not_found(word)),
        _ => Err(cycle_ambiguous(word, &n)),
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

/// --all-issues: reads the cycle's issues after the first page, 250 at a
/// time, into c's issues, which then has no more to read.
fn rest_of_issues(run: &mut Run, cl: &Client, c: &mut Value) -> R {
    let id = str_at(c, "id");
    while c["issues"]["pageInfo"]["hasNextPage"] == json!(true) {
        let after = c["issues"]["pageInfo"]["endCursor"].clone();
        let mut page = cl.query(run, queries::CYCLE_ISSUES, json!({"id": id, "after": after}))?;
        super::issue::complete_labels(run, cl, &mut page)?;
        let got = &mut page["cycle"]["issues"];
        let nodes = got["nodes"].as_array_mut().map(std::mem::take).unwrap_or_default();
        if let Some(have) = c["issues"]["nodes"].as_array_mut() {
            have.extend(nodes);
        }
        c["issues"]["pageInfo"] = got["pageInfo"].take();
    }
    Ok(())
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
issues and uncompletedIssuesUponClose come as lists: --cursor continues issues,\n\
and --all-issues reads every one, 250 a request.",
        )
        .example(
            "  linearctl cycle show current --team ENG --json\n  linearctl cycle show 23 --team ENG --issues 250 --fields number,progress,issues --json\n  linearctl cycle show current --team ENG --all-issues --fields number,issues --json",
        )
        .args(Args::Exact(1))
        .run(show)
        .flag(Flag::str("team", "the cycle's team `KEY` (falls back to LINEARCTL_TEAM, then default-team)"))
        .flag(Flag::whole("issues", 50, "include the first `N` issues; 0 for none"))
        .flag(Flag::bool("all-issues", "include every issue, however many (overrides --issues)"))
        .flag(Flag::whole("uncompleted", 50, "include the first `N` issues left open at close; 0 for none"))
        .flag(cursor_flag());
    fields_flag_spec(c, SHOW_FIELDS)
}

fn show(run: &mut Run, ctx: &Ctx) -> R {
    let word = ctx.args[0].clone();
    let filter = cycle_filter(ctx, &word)?;
    let fields = fields_flag(ctx, SHOW_FIELDS)?;
    let cursor = cursor_value(ctx)?;
    let (mut issues, uncompleted) = (ctx.whole("issues"), ctx.whole("uncompleted"));
    let all = ctx.bool("all-issues");
    if (issues > 250 && !all) || uncompleted > 250 {
        return Err(usage(
            "--issues and --uncompleted are at most 250; page issues with --cursor, or pass --all-issues",
        ));
    }
    if all {
        issues = 250;
    }
    let cl = run.client()?;
    let vars = json!({"filter": filter, "issues": issues, "withIssues": issues > 0, "after": optional(&cursor),
        "uncompleted": uncompleted, "withUncompleted": uncompleted > 0});
    let mut c = find_cycle(run, &cl, &word, vars)?;
    if all {
        rest_of_issues(run, &cl, &mut c)?;
    }
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

/// A time flag (a date or an RFC 3339 time) as Linear's DateTime, and its
/// instant for comparing; None when the flag was not given.
fn time_flag(ctx: &Ctx, flag: &str) -> R<Option<(String, i64)>> {
    let s = ctx.str(flag);
    if !ctx.changed(flag) {
        return Ok(None);
    }
    let t = time::parse_rfc3339(&s).or_else(|| time::parse_date(&s)).ok_or_else(|| {
        usage(format!("--{flag} must be a date (2026-11-02) or an RFC 3339 time; got {}", quote_go(&s)))
    })?;
    Ok(Some((time::rfc3339(t), t)))
}

/// A cycle input object, and the instants of the times in it.
type Input = (serde_json::Map<String, Value>, Vec<(&'static str, i64)>);

/// The fields create and update share: name, description and the times.
fn cycle_input(ctx: &Ctx) -> R<Input> {
    let mut input = serde_json::Map::new();
    let mut times = vec![];
    for f in ["name", "description"] {
        if ctx.changed(f) {
            input.insert(f.into(), json!(ctx.str(f)));
        }
    }
    for (flag, field) in [("starts-at", "startsAt"), ("ends-at", "endsAt"), ("completed-at", "completedAt")] {
        if let Some((text, t)) = time_flag(ctx, flag)? {
            input.insert(field.into(), json!(text));
            times.push((field, t));
        }
    }
    Ok((input, times))
}

fn input_flags(c: Cmd) -> Cmd {
    c.flag(Flag::str("name", "the cycle's custom `NAME` (Linear shows \"Cycle N\" without one)"))
        .flag(Flag::str("description", "the cycle's description, `TEXT`"))
        .flag(Flag::str("starts-at", "when it starts, `TIME`: a date or an RFC 3339 time"))
        .flag(Flag::str("ends-at", "when it ends, `TIME`"))
        .flag(Flag::str("completed-at", "when it was completed, `TIME` (completes it early)"))
}

fn create_cmd() -> Cmd {
    let c = Cmd::new("create", "Create a cycle in a team")
        .long(
            "Create a cycle in a team, from --starts-at to --ends-at. Linear numbers it.\n\n\
--yes requires --id, a UUID you choose, so a rerun can never create a second\n\
cycle: if one with that id exists, it is returned with changed: false.\n\
--dry-run without --id generates one and prints it as id; pass it to --yes.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl cycle create --team ENG --starts-at 2026-11-02 --ends-at 2026-11-16 --dry-run --json\n  linearctl cycle create --team ENG --starts-at 2026-11-02 --ends-at 2026-11-16 --id 0b6c… --yes",
        )
        .run(create)
        .flag(Flag::str("team", "create in team `KEY` (e.g. ENG)"))
        .flag(Flag::str("id", "the new cycle's `UUID`; required with --yes"));
    input_flags(c).mutating()
}

fn create(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let id = super::issue::chosen_id(ctx, dry, "create a second cycle")?;
    let (mut input, times) = cycle_input(ctx)?;
    let team = team_flag(ctx)?;
    if team.is_empty() {
        return Err(usage("pass --team KEY (or set LINEARCTL_TEAM, or default-team with 'linearctl config set')"));
    }
    if !input.contains_key("startsAt") || !input.contains_key("endsAt") {
        return Err(usage("pass --starts-at and --ends-at"));
    }
    let at = |f: &str| times.iter().find(|(k, _)| *k == f).map_or(0, |(_, t)| *t);
    if at("endsAt") <= at("startsAt") {
        return Err(usage("--ends-at must be after --starts-at"));
    }
    let cl = run.client()?;
    let r = cl.query(run, queries::CYCLE_CREATE_RESOLVE, json!({"team": team, "id": id}))?;
    if let Some(existing) = super::issue::nodes(&r["existing"]).first() {
        let c = flatten(existing.clone());
        let text = format!("exists cycle {} in {}", str_at(&c, "number"), str_at(&c, "team.key"));
        return run.output(json!({"changed": false, "dryRun": dry, "id": id, "cycle": c}), &text);
    }
    let teams = super::issue::nodes(&r["teams"]);
    let Some(t) = teams.first() else {
        return Err(missing(format!("no team with key {team}; see 'linearctl team list'")).with(sought("team", &team)));
    };
    input.insert("teamId".into(), json!(str_at(t, "id")));
    input.insert("id".into(), json!(id));
    if dry {
        let text = format!("would create a cycle in {}\nid: {id} (pass --id {id} --yes)", str_at(t, "key"));
        return run.output(json!({"changed": true, "dryRun": true, "id": id, "input": input}), &text);
    }
    let mut out = cl.query(run, queries::CYCLE_CREATE, json!({"input": input}))?;
    let c = flatten(out["cycleCreate"]["cycle"].take());
    let text = format!("created cycle {} in {}", str_at(&c, "number"), str_at(&c, "team.key"));
    run.output(json!({"changed": true, "dryRun": false, "id": id, "cycle": c}), &text)
}

/// The cycle a change applies to, as it is now.
fn find_one(run: &mut Run, cl: &Client, ctx: &Ctx, word: &str) -> R<Value> {
    let data = cl.query(run, queries::CYCLE_FIND, json!({"filter": cycle_filter(ctx, word)?}))?;
    let n = super::issue::nodes(&data["cycles"]);
    match n.len() {
        0 => Err(cycle_not_found(word)),
        1 => Ok(n[0].clone()),
        _ => Err(cycle_ambiguous(word, &n)),
    }
}

fn update_cmd() -> Cmd {
    let c = Cmd::new("update CYCLE", "Change a cycle's name, description or dates")
        .long(
            "Change a cycle's name, description or dates. Only what differs is sent; when\n\
nothing does, nothing is sent and the result is changed: false. CYCLE is as for cycle\n\
show, within --team.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl cycle update current --team ENG --name \"Checkout v2\" --dry-run --json\n  linearctl cycle update 24 --team ENG --ends-at 2026-11-05 --yes",
        )
        .args(Args::Exact(1))
        .run(update)
        .flag(Flag::str("team", "the cycle's team `KEY` (falls back to LINEARCTL_TEAM, then default-team)"));
    input_flags(c).mutating()
}

fn update(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let word = ctx.args[0].clone();
    cycle_ref("CYCLE", &word)?;
    let (mut input, times) = cycle_input(ctx)?;
    if input.is_empty() {
        return Err(usage("pass --name, --description, --starts-at, --ends-at or --completed-at"));
    }
    let cl = run.client()?;
    let c = find_one(run, &cl, ctx, &word)?;
    for f in ["name", "description"] {
        if input.get(f).is_some_and(|v| *v == c[f] || (v == "" && c[f].is_null())) {
            input.remove(f);
        }
    }
    for (field, t) in &times {
        if c[*field].as_str().and_then(time::parse_rfc3339) == Some(*t) {
            input.remove(*field);
        }
    }
    let changed = !input.is_empty();
    let label = format!("cycle {} in {}", str_at(&c, "number"), str_at(&c, "team.key"));
    let mut doc = json!({"changed": changed, "dryRun": dry, "id": c["id"], "input": input});
    if !changed {
        return run.output(doc, &format!("unchanged {label}"));
    }
    if dry {
        return run.output(doc, &format!("would update {label}"));
    }
    let mut out = cl.query(run, queries::CYCLE_UPDATE, json!({"id": c["id"], "input": doc["input"]}))?;
    doc["cycle"] = flatten(out["cycleUpdate"]["cycle"].take());
    run.output(doc, &format!("updated {label}"))
}

fn archive_cmd() -> Cmd {
    Cmd::new("archive CYCLE", "Archive a cycle")
        .long(
            "Archive a cycle. One already archived is changed: false. CYCLE is as for cycle\n\
show, within --team.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl cycle archive 21 --team ENG --dry-run\n  linearctl cycle archive 21 --team ENG --yes --json",
        )
        .args(Args::Exact(1))
        .run(archive)
        .flag(Flag::str("team", "the cycle's team `KEY` (falls back to LINEARCTL_TEAM, then default-team)"))
        .mutating()
}

fn archive(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let word = ctx.args[0].clone();
    cycle_ref("CYCLE", &word)?;
    let cl = run.client()?;
    let c = find_one(run, &cl, ctx, &word)?;
    let changed = c["archivedAt"].is_null();
    if changed && !dry {
        cl.query(run, queries::CYCLE_ARCHIVE, json!({"id": c["id"]}))?;
    }
    let label = format!("cycle {} in {}", str_at(&c, "number"), str_at(&c, "team.key"));
    let verb = match (changed, dry) {
        (false, _) => "already archived",
        (true, true) => "would archive",
        (true, false) => "archived",
    };
    run.output(json!({"changed": changed, "dryRun": dry, "id": c["id"]}), &format!("{verb} {label}"))
}
