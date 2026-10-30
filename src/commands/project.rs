use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::client::Client;
use crate::fail::{Fail, R, ambiguous, missing, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::{Run, dry_run};
use crate::util::*;
use serde_json::{Map, Value, json};

const FIELDS: &[&str] = &[
    "id",
    "name",
    "slugId",
    "url",
    "description",
    "content",
    "status",
    "lead",
    "startDate",
    "targetDate",
    "progress",
    "health",
    "createdAt",
    "updatedAt",
    "teams",
    "documents",
    "projectMilestones",
    "issues",
    "issuesTruncated",
];

/// --fields with issues brings issuesTruncated along, so that picking the
/// issues never hides that some were left out.
fn with_truncated(mut fields: Vec<String>) -> Vec<String> {
    let has = |f: &str| fields.iter().any(|x| x == f);
    if has("issues") && !has("issuesTruncated") {
        fields.push("issuesTruncated".into());
    }
    fields
}

/// show, create and update, added to the project command project list made.
pub fn commands() -> Vec<Cmd> {
    vec![show_cmd(), create_cmd(), update_cmd()]
}

/// A project given as a linear.app project URL, its slugId (the hex at the
/// end of that URL), a UUID, or its name, as Linear's ProjectFilter; `what`
/// names the argument in usage errors.
pub fn project_filter(what: &str, s: &str) -> R<Value> {
    plain(what, s)?;
    if s.starts_with("https://") {
        let parts: Option<Vec<String>> = parse_url(s)
            .filter(|u| u.host == "linear.app")
            .map(|u| u.path.trim_matches('/').split('/').map(String::from).collect());
        let Some(p) = parts.filter(|p| p.len() >= 3 && p[1] == "project") else {
            return Err(usage(format!("{what}: not a linear.app project URL: {s}")));
        };
        let slug = p[2].rsplit('-').next().unwrap_or("");
        if !is_slug(slug) {
            return Err(usage(format!("{what}: no project id at the end of {s}")));
        }
        return Ok(json!({"slugId": {"eq": slug}}));
    }
    if is_uuid(s) {
        return Ok(json!({"id": {"eq": s.to_lowercase()}}));
    }
    if is_slug(s) {
        return Ok(json!({"slugId": {"eq": s}}));
    }
    Ok(json!({"name": {"eqIgnoreCase": s}}))
}

/// The one project PROJECT names: none is not found, and a name two
/// projects share is an error listing them.
fn find_project(run: &mut Run, cl: &Client, r: &str, filter: Value, issues: u64) -> R<Value> {
    let mut data =
        cl.query(run, queries::PROJECT_FIND, json!({"filter": filter, "issues": issues, "withIssues": issues > 0}))?;
    super::issue::complete_labels(run, cl, &mut data)?;
    one_project(r, &super::issue::nodes(&data["matches"]))?;
    super::issue::nodes(&data["projects"]).into_iter().next().ok_or_else(|| project_not_found(r))
}

fn project_not_found(r: &str) -> Fail {
    missing(format!("not found: project {r} (or this API key cannot see it)"))
        .with(sought("project", r))
        .hint("'linearctl project list' lists the projects this key can see")
}

/// The one project of those a filter for r found.
pub fn one_project(r: &str, n: &[Value]) -> R<Value> {
    match n.len() {
        0 => Err(project_not_found(r)),
        1 => Ok(n[0].clone()),
        _ => Err(ambiguous(format!(
            "more than one project is named {} ({}, {}); pass its URL or slugId",
            quote_go(r),
            str_at(&n[0], "url"),
            str_at(&n[1], "url")
        ))
        .with(sought("project", r))
        .with(json!({"candidates": candidates(n)}))),
    }
}

/// Reads the rest of the project's connection key, 250 at a time with doc,
/// into p's key, which then has no more to read.
fn read_rest(run: &mut Run, cl: &Client, p: &mut Value, key: &str, doc: &str) -> R {
    let id = str_at(p, "id");
    while p[key]["pageInfo"]["hasNextPage"] == json!(true) {
        let after = p[key]["pageInfo"]["endCursor"].clone();
        let mut page = cl.query(run, doc, json!({"id": id, "after": after}))?;
        super::issue::complete_labels(run, cl, &mut page)?;
        let got = &mut page["project"][key];
        let nodes = got["nodes"].as_array_mut().map(std::mem::take).unwrap_or_default();
        if let Some(have) = p[key]["nodes"].as_array_mut() {
            have.extend(nodes);
        }
        p[key]["pageInfo"] = got["pageInfo"].take();
    }
    Ok(())
}

/// Takes the pageInfo off p's connection key, leaving {nodes} for flatten,
/// and says whether it had more to read; None when key is absent.
fn take_more(p: &mut Value, key: &str) -> Option<bool> {
    let Some(Value::Object(c)) = p.get_mut(key) else { return None };
    let info = c.shift_remove("pageInfo").unwrap_or(Value::Null);
    Some(info["hasNextPage"] == json!(true))
}

fn show_cmd() -> Cmd {
    let c = Cmd::new("show PROJECT", "Show a project: its overview, milestones and issues")
        .long(
            "Show a project: its description, overview (content), status, health, documents,\n\
milestones and issues. Documents and milestones are read in full; issues up to\n\
--issues, and --all-issues reads every one, 250 a request. issuesTruncated is\n\
true when more issues exist than came; --fields issues brings it along.\n\
PROJECT is a linear.app project URL, its slugId (the hex the URL ends with), a UUID,\n\
or its exact name.",
        )
        .example(
            "  linearctl project show https://linear.app/acme/project/q4-launch-0527ca4c9128\n  linearctl project show \"Q4 launch\" --issues 100 --fields name,content,issues --json\n  linearctl project show \"Q4 launch\" --all-issues --fields name,issues --json",
        )
        .args(Args::Exact(1))
        .run(show)
        .flag(Flag::whole("issues", 50, "include the first `N` issues; 0 for none"))
        .flag(Flag::bool("all-issues", "include every issue, however many (overrides --issues)"));
    fields_flag_spec(c, FIELDS)
}

fn show(run: &mut Run, ctx: &Ctx) -> R {
    let filter = project_filter("PROJECT", &ctx.args[0])?;
    let fields = with_truncated(fields_flag(ctx, FIELDS)?);
    let all = ctx.bool("all-issues");
    let issues = if all { 250 } else { ctx.whole("issues") };
    if issues > 250 {
        return Err(usage("--issues is at most 250; pass --all-issues, or page with 'linearctl issue list --project'"));
    }
    let cl = run.client()?;
    let mut p = find_project(run, &cl, &ctx.args[0], filter, issues)?;
    read_rest(run, &cl, &mut p, "documents", queries::PROJECT_DOCUMENTS)?;
    take_more(&mut p, "documents");
    read_rest(run, &cl, &mut p, "projectMilestones", queries::PROJECT_MILESTONES)?;
    take_more(&mut p, "projectMilestones");
    if all {
        read_rest(run, &cl, &mut p, "issues", queries::PROJECT_ISSUES)?;
    }
    let more = take_more(&mut p, "issues");
    let mut p = flatten(p);
    let shown = p["issues"].as_array().map_or(0, Vec::len);
    if let Some(more) = more {
        p["issuesTruncated"] = json!(more);
        if more {
            run.eprint(&format!("{shown} issues shown, more remain; --all-issues for all\n"));
        }
    }
    if run.json {
        return run.print_json(json!({"project": pick(p, &fields)}));
    }
    let mut s = format!(
        "{}  ({})\n{}\n",
        printable(&str_at(&p, "name")),
        printable(&str_at(&p, "status.name")),
        str_at(&p, "url")
    );
    let d = str_at(&p, "description");
    if !d.is_empty() {
        s.push_str(&format!("\n{}\n", printable(&d)));
    }
    let body = str_at(&p, "content");
    if !body.is_empty() {
        s.push_str(&format!("\n{}\n", printable_text(&body)));
    }
    run.print(&s)?;
    if let Some(is) = p["issues"].as_array().filter(|i| !i.is_empty()) {
        let more = if more == Some(true) { " (more remain)" } else { "" };
        run.print(&format!("\n{shown} issues{more}\n"))?;
        rows(run, is, &["identifier", "state.name", "assignee.name", "title"])?;
    }
    Ok(())
}

fn create_cmd() -> Cmd {
    Cmd::new("create", "Create a project in one or more teams")
        .long(
            "Create a project in one or more teams (--team, repeatable; the first leads).\n\
Names (team keys, the lead's email, the status) are resolved to ids first;\n\
--dry-run prints the exact input it would send. --json-input takes Linear's\n\
ProjectCreateInput as is (icon, color, labelIds, templateId, …); flags win.\n\n\
--yes requires --id, a UUID you choose, so a rerun can never create a second\n\
project: if one with that id exists, it is returned with changed: false.\n\
--dry-run without --id generates one and prints it as id; pass it to --yes.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl project create --team ENG --name \"Q4 launch\" --target-date 2026-12-15 --dry-run --json\n  linearctl project create --team ENG --team DES --name \"Q4 launch\" --lead me --id 0b6c… --yes\n  linearctl project create --json-input project.json --id 0b6c… --yes",
        )
        .run(create)
        .flag(Flag::strs("team", "in team `KEY` (repeatable; else LINEARCTL_TEAM, then default-team)"))
        .flag(Flag::str("name", "the project's name, `TEXT`"))
        .flag(Flag::str("description", "its one-line summary, `TEXT`"))
        .flag(Flag::str("description-file", "read the summary from `PATH` (- for stdin)"))
        .flag(Flag::str("content", "its overview, Markdown `TEXT`"))
        .flag(Flag::str("content-file", "read the overview from `PATH` (- for stdin)"))
        .flag(Flag::str("lead", "the project lead, `WHO`: me or an email"))
        .flag(Flag::str("status", "project status `NAME` (Linear's default when not given)"))
        .flag(Flag::str("priority", "`PRIORITY`: 0-4 or none, urgent, high, medium, low"))
        .flag(Flag::str("start-date", "the planned start, `DATE` (2026-11-02)"))
        .flag(Flag::str("target-date", "the planned completion, `DATE`"))
        .flag(Flag::str("json-input", "read Linear's input object from `PATH` (- for stdin); flags win"))
        .flag(Flag::str("id", "the new project's `UUID`; required with --yes"))
        .mutating()
}

/// The teams a new project goes in: --team keys, else teamIds from
/// --json-input, else the default team. Each is (by, value), by being key
/// or id, in the order given: the first team leads.
fn create_teams(ctx: &Ctx, input: &Map<String, Value>) -> R<Vec<(&'static str, String)>> {
    let from_input = input.get("teamIds").filter(|_| !ctx.changed("team"));
    if let Some(ids) = from_input {
        let ids: Vec<String> = match ids.as_array() {
            Some(a) => a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
            None => vec![],
        };
        if ids.is_empty() || !ids.iter().all(|i| is_uuid(i)) {
            return Err(usage(format!(
                "teamIds in --json-input must be a list of team UUIDs; got {ids}",
                ids = input["teamIds"]
            )));
        }
        return Ok(ids.into_iter().map(|i| ("id", i.to_lowercase())).collect());
    }
    let keys = teams_flag(ctx)?;
    if keys.is_empty() {
        return Err(usage(
            "pass --team KEY, or teamIds in --json-input (or set LINEARCTL_TEAM, or default-team with 'linearctl config set')",
        ));
    }
    Ok(keys.into_iter().map(|k| ("key", k)).collect())
}

fn create(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let id = super::issue::chosen_id(ctx, dry, "create a second project")?;
    let mut input = super::issue::json_input(ctx)?;
    if ctx.changed("name") {
        let name = ctx.str("name");
        plain("--name", &name)?;
        input.insert("name".into(), json!(name));
    }
    for f in ["description", "content"] {
        if let Some(v) = text(ctx, f)? {
            input.insert(f.into(), json!(v));
        }
    }
    let p = ctx.str("priority");
    if !p.is_empty() {
        input.insert("priority".into(), json!(super::issue::priority("--priority", &p)?));
    }
    for (flag, field) in [("start-date", "startDate"), ("target-date", "targetDate")] {
        let d = ctx.str(flag);
        if !ctx.changed(flag) {
            continue;
        }
        if d.len() != 10 || crate::time::parse_date(&d).is_none() {
            return Err(usage(format!("--{flag} must be a date (2026-11-30); got {}", quote_go(&d))));
        }
        input.insert(field.into(), json!(d));
    }
    let lead = ctx.str("lead");
    if !lead.is_empty() && lead != "me" && !is_email(&lead) {
        return Err(usage(format!("--lead must be me or an email; got {}", quote_go(&lead))));
    }
    let status = ctx.str("status");
    if !status.is_empty() {
        plain("--status", &status)?;
    }
    if input.get("name").and_then(Value::as_str).unwrap_or("").is_empty() {
        return Err(usage("pass --name (or name in --json-input)"));
    }
    let teams = create_teams(ctx, &input)?;
    let filter: Vec<Value> = teams
        .iter()
        .map(|(by, v)| if *by == "key" { json!({"key": {"eqIgnoreCase": v}}) } else { json!({"id": {"eq": v}}) })
        .collect();
    let email = if is_email(&lead) { lead.clone() } else { String::new() };
    let vars = json!({"teams": {"or": filter}, "id": id, "email": email, "withUser": !email.is_empty(),
        "me": lead == "me", "withStatus": !status.is_empty()});
    let cl = run.client()?;
    let r = cl.query(run, queries::PROJECT_CREATE_RESOLVE, vars)?;
    if let Some(existing) = super::issue::nodes(&r["existing"]).first() {
        let p = flatten(existing.clone());
        let text = format!("exists {} {}", printable(&str_at(&p, "name")), str_at(&p, "url"));
        return run.output(json!({"changed": false, "dryRun": dry, "id": id, "project": p}), &text);
    }
    let found = super::issue::nodes(&r["teams"]);
    let mut ids = vec![];
    let mut keys = vec![];
    for (by, v) in &teams {
        let Some(t) = found.iter().find(|t| eq_fold(&str_at(t, by), v)) else {
            return Err(missing(format!("no team with {by} {v}; see 'linearctl team list'")).with(sought("team", v)));
        };
        if !ids.contains(&str_at(t, "id")) {
            ids.push(str_at(t, "id"));
            keys.push(str_at(t, "key"));
        }
    }
    input.insert("teamIds".into(), json!(ids));
    match lead.as_str() {
        "" => {}
        "me" => _ = input.insert("leadId".into(), json!(str_at(&r["viewer"], "id"))),
        e => {
            let users = super::issue::nodes(&r["users"]);
            let lid = super::issue::one("user", "user with email", e, &users, "'linearctl user list'")?;
            input.insert("leadId".into(), json!(lid));
        }
    }
    if !status.is_empty() {
        let all = super::issue::nodes(&r["projectStatuses"]);
        let named: Vec<Value> = all.iter().filter(|s| eq_fold(&str_at(s, "name"), &status)).cloned().collect();
        let sid = match named.as_slice() {
            [one] => str_at(one, "id"),
            [] => {
                let names: Vec<String> = all.iter().map(|s| str_at(s, "name")).collect();
                return Err(missing(format!(
                    "no project status named {}; the workspace has {}",
                    quote_go(&status),
                    names.join(", ")
                ))
                .with(sought("status", &status))
                .with(json!({"available": names})));
            }
            _ => {
                return Err(ambiguous(format!(
                    "{} project statuses are named {}; pass statusId in --json-input",
                    named.len(),
                    quote_go(&status)
                ))
                .with(sought("status", &status))
                .with(json!({"candidates": candidates(&named)})));
            }
        };
        input.insert("statusId".into(), json!(sid));
    }
    input.insert("id".into(), json!(id));
    let name = printable(input["name"].as_str().unwrap_or(""));
    if dry {
        let text = format!("would create project {name} in {}\nid: {id} (pass --id {id} --yes)", keys.join(", "));
        return run.output(json!({"changed": true, "dryRun": true, "id": id, "input": input}), &text);
    }
    let mut out = cl.query(run, queries::PROJECT_CREATE, json!({"input": input}))?;
    let p = flatten(out["projectCreate"]["project"].take());
    let text = format!("created project {} {}", printable(&str_at(&p, "name")), str_at(&p, "url"));
    run.output(json!({"changed": true, "dryRun": false, "id": id, "project": p}), &text)
}

fn update_cmd() -> Cmd {
    Cmd::new("update PROJECT", "Change a project's name, description or overview")
        .long(
            "Change a project's name, description (its one-line summary) or content (the\n\
overview document; the new text replaces all of it). Only fields that differ are\n\
sent; when none do, nothing is sent and the result is changed: false.\n\
PROJECT is a URL, slugId, UUID or exact name, as for project show.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl project update \"Q4 launch\" --content-file overview.md --dry-run --json\n  linearctl project update 0527ca4c9128 --description \"Ship the new login\" --yes",
        )
        .args(Args::Exact(1))
        .run(update)
        .flag(Flag::str("name", "the new name, `TEXT`"))
        .flag(Flag::str("description", "the new one-line summary, `TEXT`"))
        .flag(Flag::str("description-file", "read the summary from `PATH` (- for stdin)"))
        .flag(Flag::str("content", "the new overview, Markdown `TEXT`; replaces all of it"))
        .flag(Flag::str("content-file", "read the overview from `PATH` (- for stdin)"))
        .mutating()
}

fn update(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let filter = project_filter("PROJECT", &ctx.args[0])?;
    let mut input = Map::new();
    if ctx.changed("name") {
        let name = ctx.str("name");
        plain("--name", &name)?;
        input.insert("name".into(), json!(name));
    }
    for f in ["description", "content"] {
        if let Some(v) = text(ctx, f)? {
            input.insert(f.into(), json!(v));
        }
    }
    if input.is_empty() {
        return Err(usage("pass --name, --description, or --content / --content-file"));
    }
    let cl = run.client()?;
    let p = find_project(run, &cl, &ctx.args[0], filter, 0)?;
    input.retain(|k, v| str_at(&p, k) != v.as_str().unwrap_or(""));
    let changed = !input.is_empty();
    let doc = json!({"changed": changed, "dryRun": dry, "id": p["id"], "url": p["url"], "input": input});
    let name = printable(&str_at(&p, "name"));
    if !changed {
        return run.output(doc, &format!("unchanged {name}"));
    }
    if dry {
        return run.output(doc, &format!("would update {name}"));
    }
    cl.query(run, queries::PROJECT_UPDATE, json!({"id": p["id"], "input": input}))?;
    run.output(doc, &format!("updated {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_more_reads_and_drops_page_info() {
        let mut p = json!({
            "issues": {"nodes": [1], "pageInfo": {"hasNextPage": true, "endCursor": "c"}},
            "documents": {"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}},
            "teams": {"nodes": []},
        });
        assert_eq!(take_more(&mut p, "issues"), Some(true));
        assert_eq!(take_more(&mut p, "documents"), Some(false));
        assert_eq!(take_more(&mut p, "teams"), Some(false));
        assert_eq!(take_more(&mut p, "absent"), None);
        assert_eq!(flatten(p), json!({"issues": [1], "documents": [], "teams": []}));
    }

    #[test]
    fn fields_with_issues_bring_issues_truncated() {
        let f = |v: &[&str]| with_truncated(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(f(&["name", "issues"]), ["name", "issues", "issuesTruncated"]);
        assert_eq!(f(&["issuesTruncated", "issues"]), ["issuesTruncated", "issues"]);
        assert_eq!(f(&["name"]), ["name"]);
        assert!(f(&[]).is_empty());
    }
}
