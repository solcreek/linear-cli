//! The "NOUN list" commands: what an agent or a picker needs to name things
//! (states, labels, users) in other commands.

use crate::cli::{Cmd, Ctx, Flag};
use crate::fail::{R, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::Run;
use crate::util::*;
use serde_json::{Map, Value, json};
use std::cmp::Ordering;

type Filter = fn(&Ctx, &str) -> R<Option<Value>>;
type Sort = fn(&Value, &Value) -> Ordering;

struct Lookup {
    noun: &'static str,
    short: &'static str,
    long: &'static str,
    example: &'static str,
    doc: &'static str,
    conn: &'static str,
    fields: &'static [&'static str],
    /// The columns of the text output.
    cols: &'static [&'static str],
    /// It takes --team.
    team: bool,
    filter: Option<Filter>,
    /// Orders the items, for lists Linear does not order usefully.
    sort: Option<Sort>,
}

pub const PROJECT_STATUSES: &[&str] = &["backlog", "planned", "started", "paused", "completed", "canceled"];

/// cycle list's filter: the team, Linear's is* states (any of those given),
/// and a range of start and end dates.
fn cycle_filter(ctx: &Ctx, team: &str) -> R<Option<Value>> {
    let mut and: Vec<Value> = vec![];
    if !team.is_empty() {
        and.push(json!({"team": {"key": {"eqIgnoreCase": team}}}));
    }
    let states: Vec<Value> = [
        ("active", "isActive"),
        ("next", "isNext"),
        ("previous", "isPrevious"),
        ("past", "isPast"),
        ("future", "isFuture"),
    ]
    .iter()
    .filter(|(flag, _)| ctx.bool(flag))
    .map(|(_, field)| json!({*field: {"eq": true}}))
    .collect();
    match states.len() {
        0 => {}
        1 => and.push(states[0].clone()),
        _ => and.push(json!({"or": states})),
    }
    for (flag, field, op) in [("starts-since", "startsAt", "gte"), ("ends-before", "endsAt", "lte")] {
        let w = ctx.str(flag);
        if ctx.changed(flag) && w.is_empty() {
            return Err(usage(format!("--{flag} is empty")));
        }
        if !w.is_empty() {
            let at = super::issue::when(&w).map_err(|_| {
                usage(format!(
                    "--{flag} must be a date (2026-10-01), an RFC 3339 time, or 7d / 24h; got {}",
                    quote_go(&w)
                ))
            })?;
            and.push(json!({field: {op: at}}));
        }
    }
    Ok(match and.len() {
        0 => None,
        1 => and.pop(),
        _ => Some(json!({"and": and})),
    })
}

fn team_key(_: &Ctx, team: &str) -> R<Option<Value>> {
    Ok((!team.is_empty()).then(|| json!({"team": {"key": {"eqIgnoreCase": team}}})))
}

const LOOKUPS: &[Lookup] = &[
    Lookup {
        noun: "team",
        short: "List teams",
        long: "",
        example: "  linearctl team list --json",
        doc: queries::TEAMS,
        conn: "teams",
        fields: &["id", "key", "name", "description", "private"],
        cols: &["key", "name"],
        team: false,
        filter: None,
        sort: None,
    },
    Lookup {
        noun: "state",
        short: "List workflow states, by team and position",
        long: "List workflow states, by team and position: the names issue create and update\ntake for --state.",
        example: "  linearctl state list --team ENG --json",
        doc: queries::STATES,
        conn: "workflowStates",
        fields: &["id", "name", "type", "position", "color", "team"],
        cols: &["team.key", "name", "type"],
        team: true,
        filter: Some(team_key),
        sort: Some(|a, b| {
            str_at(a, "team.key").cmp(&str_at(b, "team.key")).then_with(|| {
                let (pa, pb) = (a["position"].as_f64().unwrap_or(0.0), b["position"].as_f64().unwrap_or(0.0));
                pa.partial_cmp(&pb).unwrap_or(Ordering::Equal)
            })
        }),
    },
    Lookup {
        noun: "label",
        short: "List issue labels",
        long: "List issue labels. With --team, the team's labels and the workspace's (no team),\nwhich are the ones its issues can take.",
        example: "  linearctl label list --team ENG --json",
        doc: queries::LABELS,
        conn: "issueLabels",
        fields: &["id", "name", "color", "isGroup", "team", "parent"],
        cols: &["name", "team.key", "parent.name"],
        team: true,
        filter: Some(|_, team| {
            Ok((!team.is_empty())
                .then(|| json!({"or": [{"team": {"key": {"eqIgnoreCase": team}}}, {"team": {"null": true}}]})))
        }),
        sort: Some(|a, b| str_at(a, "name").cmp(&str_at(b, "name"))),
    },
    Lookup {
        noun: "user",
        short: "List users in the workspace",
        long: "",
        example: "  linearctl user list --active --json",
        doc: queries::USERS,
        conn: "users",
        fields: &["id", "name", "displayName", "email", "active", "admin", "guest", "app"],
        cols: &["email", "name"],
        team: false,
        filter: Some(|ctx, _| {
            let mut f = Map::new();
            if ctx.bool("active") {
                f.insert("active".into(), json!({"eq": true}));
            }
            if ctx.bool("app") {
                f.insert("app".into(), json!({"eq": true}));
            }
            Ok((!f.is_empty()).then_some(Value::Object(f)))
        }),
        sort: None,
    },
    Lookup {
        noun: "project",
        short: "List projects",
        long: "",
        example: "  linearctl project list --team ENG --status started --json",
        doc: queries::PROJECTS,
        conn: "projects",
        fields: &["id", "name", "url", "targetDate", "status", "lead"],
        cols: &["name", "status.name", "lead.name"],
        team: true,
        filter: Some(|ctx, team| {
            let mut f = Map::new();
            if !team.is_empty() {
                f.insert("accessibleTeams".into(), json!({"some": {"key": {"eqIgnoreCase": team}}}));
            }
            let s = values(ctx, "status")?;
            if !s.is_empty() {
                for t in &s {
                    if !PROJECT_STATUSES.contains(&t.as_str()) {
                        return Err(usage(format!(
                            "--status must be one of {}; got {}",
                            PROJECT_STATUSES.join(", "),
                            quote_go(t)
                        )));
                    }
                }
                f.insert("status".into(), json!({"type": {"in": s}}));
            }
            Ok((!f.is_empty()).then_some(Value::Object(f)))
        }),
        sort: None,
    },
    Lookup {
        noun: "milestone",
        short: "List project milestones",
        long: "List project milestones, the names issue create and update take for --milestone.\nprogress is Linear's, from 0 to 1.",
        example: "  linearctl milestone list --project \"Q4 launch\" --json",
        doc: queries::MILESTONES,
        conn: "projectMilestones",
        fields: &["id", "name", "targetDate", "progress", "project"],
        cols: &["project.name", "name", "targetDate"],
        team: false,
        filter: Some(|ctx, _| {
            let p = ctx.str("project");
            if p.is_empty() {
                return Ok(None);
            }
            plain("--project", &p)?;
            Ok(Some(json!({"project": {"name": {"eqIgnoreCase": p}}})))
        }),
        sort: None,
    },
    Lookup {
        noun: "notification",
        short: "List your inbox notifications, newest first",
        long: "List your inbox notifications, newest first. readAt is null for an unread one;\nLinear has no unread filter, so filter on readAt yourself.",
        example: "  linearctl notification list --limit 20 --json",
        doc: queries::NOTIFICATIONS,
        conn: "notifications",
        fields: &[
            "id",
            "type",
            "title",
            "subtitle",
            "url",
            "createdAt",
            "readAt",
            "snoozedUntilAt",
            "actor",
            "issue",
            "comment",
        ],
        cols: &["createdAt", "type", "issue.identifier", "title"],
        team: false,
        filter: Some(|ctx, _| {
            let kind = ctx.str("type");
            if kind.is_empty() {
                return Ok(None);
            }
            plain("--type", &kind)?;
            Ok(Some(json!({"type": {"eq": kind}})))
        }),
        sort: None,
    },
    Lookup {
        noun: "view",
        short: "List saved custom views",
        long: "List the custom views saved in Linear. modelName says what a view lists (Issue,\nProject); issue list --view VIEW lists an issue view's issues.",
        example: "  linearctl view list --json",
        doc: queries::VIEWS,
        conn: "customViews",
        fields: &["id", "name", "description", "slugId", "modelName", "shared", "team", "owner", "updatedAt"],
        cols: &["modelName", "name", "team.key"],
        team: true,
        filter: Some(team_key),
        sort: None,
    },
    Lookup {
        noun: "cycle",
        short: "List cycles",
        long: "List cycles, with the same fields as cycle show's top level. Several of --active,\n--next, --previous, --past and --future match any of them.",
        example: "  linearctl cycle list --team ENG --json",
        doc: queries::CYCLES,
        conn: "cycles",
        fields: &[
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
        ],
        cols: &["team.key", "number", "startsAt", "endsAt"],
        team: true,
        filter: Some(cycle_filter),
        sort: None,
    },
];

pub fn commands() -> Vec<Cmd> {
    LOOKUPS.iter().map(command).collect()
}

fn command(l: &Lookup) -> Cmd {
    let mut list = Cmd::new("list", l.short).long(l.long).example(l.example).run(list);
    if l.team {
        list = list.flag(Flag::str("team", "only team `KEY` (falls back to LINEARCTL_TEAM, then default-team)"));
    }
    list = match l.noun {
        "user" => list
            .flag(Flag::bool("active", "only active users"))
            .flag(Flag::bool("app", "only app users: the agents issues can be delegated to")),
        "notification" => {
            list.flag(Flag::str("type", "only notifications of `TYPE` as Linear names them (e.g. issueAssignedToYou)"))
        }
        "milestone" => list.flag(Flag::str("project", "only milestones of project `NAME`")),
        "cycle" => list
            .flag(Flag::bool("active", "only the active cycle (Linear's isActive; CYCLE current elsewhere)"))
            .flag(Flag::bool("next", "only the next cycle (isNext)"))
            .flag(Flag::bool("previous", "only the previous cycle (isPrevious)"))
            .flag(Flag::bool("past", "only cycles that have ended (isPast)"))
            .flag(Flag::bool("future", "only cycles yet to start (isFuture)"))
            .flag(Flag::str("starts-since", "only cycles starting at or after `WHEN` (a date, RFC 3339, 7d, 24h)"))
            .flag(Flag::str("ends-before", "only cycles ending at or before `WHEN`")),
        "project" => list.flag(Flag::strs(
            "status",
            &format!("only projects whose status is `TYPE` (repeatable):\n{}", PROJECT_STATUSES.join(", ")),
        )),
        _ => list,
    };
    list = list.flag(limit_flag(50)).flag(cursor_flag());
    list = fields_flag_spec(list, l.fields);
    Cmd::group(l.noun, l.short, vec![list])
}

fn list(run: &mut Run, ctx: &Ctx) -> R {
    let noun = ctx.found.chain[1].name();
    let l = LOOKUPS.iter().find(|l| l.noun == noun).expect("a lookup");
    let fields = fields_flag(ctx, l.fields)?;
    let cursor = cursor_value(ctx)?;
    let team = if l.team { team_flag(ctx)? } else { String::new() };
    let filter = match l.filter {
        Some(f) => f(ctx, &team)?,
        None => None,
    };
    let cl = run.client()?;
    let (mut items, next) = collect(ctx.whole("limit"), cursor, |first, after| {
        let mut vars = json!({"first": first, "after": optional(after)});
        if let Some(f) = &filter {
            vars["filter"] = f.clone();
        }
        let mut data = cl.query(run, l.doc, vars)?;
        Ok(Page::take(&mut data[l.conn]))
    })?;
    if let Some(sort) = l.sort {
        items.sort_by(sort);
    }
    if run.json {
        let doc = list_doc(run, items, &next, &fields);
        return run.print_json(doc);
    }
    more_note(run, items.len(), &next);
    rows(run, &items, l.cols)
}
