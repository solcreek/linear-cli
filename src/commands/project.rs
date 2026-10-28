use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::client::Client;
use crate::fail::{R, ambiguous, missing, usage};
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
];

/// show and update, added to the project command project list made.
pub fn commands() -> Vec<Cmd> {
    vec![show_cmd(), update_cmd()]
}

/// A PROJECT given as a linear.app project URL, its slugId (the hex at the
/// end of that URL), a UUID, or its exact name.
fn project_filter(s: &str) -> R<Value> {
    plain("PROJECT", s)?;
    if s.starts_with("https://") {
        let parts: Option<Vec<String>> = parse_url(s)
            .filter(|u| u.host == "linear.app")
            .map(|u| u.path.trim_matches('/').split('/').map(String::from).collect());
        let Some(p) = parts.filter(|p| p.len() >= 3 && p[1] == "project") else {
            return Err(usage(format!("PROJECT: not a linear.app project URL: {s}")));
        };
        let slug = p[2].rsplit('-').next().unwrap_or("");
        if !is_slug(slug) {
            return Err(usage(format!("PROJECT: no project id at the end of {s}")));
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
    let n = super::issue::nodes(&data["projects"]);
    match n.len() {
        0 => Err(missing(format!("not found: project {r} (or this API key cannot see it)"))
            .with(sought("project", r))
            .hint("'linearctl project list' lists the projects this key can see")),
        1 => Ok(n[0].clone()),
        _ => Err(ambiguous(format!(
            "more than one project is named {} ({}, {}); pass its URL or slugId",
            quote_go(r),
            str_at(&n[0], "url"),
            str_at(&n[1], "url")
        ))
        .with(sought("project", r))
        .with(json!({"candidates": candidates(&n)}))),
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
milestones and issues. Documents are read in full; issues up to --issues, and\n\
--all-issues reads every one, 250 a request.\n\
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
    let filter = project_filter(&ctx.args[0])?;
    let fields = fields_flag(ctx, FIELDS)?;
    let all = ctx.bool("all-issues");
    let issues = if all { 250 } else { ctx.whole("issues") };
    if issues > 250 {
        return Err(usage("--issues is at most 250; pass --all-issues, or page with 'linearctl issue list --project'"));
    }
    let cl = run.client()?;
    let mut p = find_project(run, &cl, &ctx.args[0], filter, issues)?;
    read_rest(run, &cl, &mut p, "documents", queries::PROJECT_DOCUMENTS)?;
    take_more(&mut p, "documents");
    if all {
        read_rest(run, &cl, &mut p, "issues", queries::PROJECT_ISSUES)?;
    }
    take_more(&mut p, "issues");
    let p = flatten(p);
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
        run.print(&format!("\n{} issues\n", is.len()))?;
        rows(run, is, &["identifier", "state.name", "assignee.name", "title"])?;
    }
    Ok(())
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
    let filter = project_filter(&ctx.args[0])?;
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
