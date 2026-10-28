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

/// show and update, added to the project command project list made.
pub fn commands() -> Vec<Cmd> {
    vec![show_cmd(), update_cmd()]
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
milestones and issues. Documents are read in full; issues up to --issues, and\n\
--all-issues reads every one, 250 a request. issuesTruncated is true when more\n\
issues exist than came; --fields issues brings it along.\n\
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
