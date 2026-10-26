use crate::cli::{Args, Cmd, Ctx};
use crate::fail::{self, Fail, R, missing, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::{Run, dry_run};
use crate::util::*;
use serde_json::{Value, json};

/// TYPE as the commands take it. Linear stores blocks, duplicate, related
/// and similar; blocked-by is blocks with the issues swapped. related and
/// similar hold in both directions.
const TYPES: &[&str] = &["blocks", "blocked-by", "duplicate", "related", "similar"];

const HELP: &str = "TYPE is one of:\n  blocks      ISSUE blocks OTHER\n  blocked-by  OTHER blocks ISSUE\n  duplicate   ISSUE is a duplicate of OTHER\n  related     ISSUE and OTHER are related (either direction)\n  similar     ISSUE and OTHER are similar (either direction)";

pub fn command() -> Cmd {
    Cmd::group(
        "relation",
        "Create and delete relations between issues (blocks, duplicate, related)",
        vec![relation_cmd(true), relation_cmd(false)],
    )
}

fn relation_cmd(create: bool) -> Cmd {
    let verb = if create { "create" } else { "delete" };
    let long = if create {
        "Relate two issues. A relation that already exists is changed: false.\n"
    } else {
        "Remove a relation between two issues. One that is not there is changed: false.\n\
Without TYPE, removes every relation between the two, in either direction,\n\
and lists each in relations.\n"
    };
    let usage_line = if create { "create ISSUE TYPE OTHER" } else { "delete ISSUE [TYPE] OTHER" };
    let mut c = Cmd::new(usage_line, if create { "Relate two issues" } else { "Remove a relation between two issues" })
        .long(&format!("{long}{HELP}\nChanges things: pass --dry-run to preview, --yes to apply."))
        .example(&format!(
            "  linearctl relation {verb} ENG-12 blocks ENG-15 --dry-run\n  linearctl relation {verb} ENG-12 duplicate ENG-9 --yes --json{}",
            if create { "" } else { "\n  linearctl relation delete ENG-12 ENG-15 --dry-run --json" }
        ))
        .args(if create {
            Args::Exact(3)
        } else {
            Args::Custom(|ctx| match ctx.args.len() {
                2 | 3 => Ok(()),
                n => Err(format!("accepts 2 or 3 arg(s), received {n}")),
            })
        })
        .mutating();
    c.run = Some(if create { |run, ctx| relation(run, ctx, true) } else { |run, ctx| relation(run, ctx, false) });
    c
}

/// ISSUE [TYPE] OTHER, checked before anything is sent. TYPE is "" when
/// it is left out.
fn relation_args(args: &[String]) -> R<(String, String, String)> {
    let from = issue_ref("ISSUE", &args[0])?;
    let kind = if args.len() == 3 { args[1].clone() } else { String::new() };
    if args.len() == 3 && !TYPES.contains(&kind.as_str()) {
        return Err(usage(format!("TYPE must be one of {}; got {}", TYPES.join(", "), quote_go(&kind))));
    }
    let to = issue_ref("OTHER", &args[args.len() - 1])?;
    if from == to {
        return Err(usage(format!("an issue cannot relate to itself: {from}")));
    }
    Ok((from, kind, to))
}

/// The two issues as Linear's relation has them ({id, identifier}), and
/// its type.
struct Ends {
    issue: Value,
    related: Value,
    kind: String,
}

/// The id of the relation ends describes, "" if there is none. a is one of
/// its two issues, with the relations Linear stores from it and to it.
/// related and similar also match reversed.
fn existing(a: &Value, ends: &Ends) -> String {
    let symmetric = ends.kind == "related" || ends.kind == "similar";
    let (ei, er) = (str_at(&ends.issue, "id"), str_at(&ends.related, "id"));
    let matches = |kind: &str, issue: &str, related: &str| {
        kind == ends.kind && ((issue == ei && related == er) || (symmetric && issue == er && related == ei))
    };
    let aid = str_at(a, "id");
    for r in super::issue::nodes(&a["relations"]) {
        if matches(&str_at(&r, "type"), &aid, &str_at(&r, "relatedIssue.id")) {
            return str_at(&r, "id");
        }
    }
    for r in super::issue::nodes(&a["inverseRelations"]) {
        if matches(&str_at(&r, "type"), &str_at(&r, "issue.id"), &aid) {
            return str_at(&r, "id");
        }
    }
    String::new()
}

fn relation(run: &mut Run, ctx: &Ctx, create: bool) -> R {
    let verb = if create { "create" } else { "delete" };
    let dry = dry_run(ctx)?;
    let (from, kind, to) = relation_args(&ctx.args)?;
    let cl = run.client()?;
    let r = cl.query(run, queries::RELATION_RESOLVE, json!({"a": issue_filter(&from), "b": issue_filter(&to)}))?;
    let a_nodes = super::issue::nodes(&r["a"]);
    let b_nodes = super::issue::nodes(&r["b"]);
    let Some(a) = a_nodes.first() else {
        return Err(missing(format!("not found: issue {from} (or this API key cannot see it)")).hint(fail::FIND_ISSUE));
    };
    let Some(b) = b_nodes.first() else {
        return Err(missing(format!("not found: issue {to} (or this API key cannot see it)")).hint(fail::FIND_ISSUE));
    };
    if kind.is_empty() {
        return delete_all(run, &cl, dry, a, b);
    }
    let me = json!({"id": str_at(a, "id"), "identifier": str_at(a, "identifier")});
    let other = json!({"id": str_at(b, "id"), "identifier": str_at(b, "identifier")});
    let ends = if kind == "blocked-by" {
        Ends { issue: other, related: me, kind: "blocks".into() }
    } else {
        Ends { issue: me, related: other, kind }
    };
    let id = existing(a, &ends);
    let changed = id.is_empty() == create;
    let (ii, ri) = (str_at(&ends.issue, "identifier"), str_at(&ends.related, "identifier"));
    let mut doc = json!({"changed": changed, "dryRun": dry,
        "relation": {"id": optional(&id), "type": ends.kind, "issue": ii, "relatedIssue": ri}});
    let text = format!("{ii} {} {ri}", ends.kind);
    if !changed {
        let prefix = if create { "exists: " } else { "no relation: " };
        return run.output(doc, &format!("{prefix}{text}"));
    }
    if dry {
        return run.output(doc, &format!("would {verb}: {text}"));
    }
    if create {
        let input = json!({"issueId": str_at(&ends.issue, "id"), "relatedIssueId": str_at(&ends.related, "id"), "type": ends.kind});
        let out = cl.query(run, queries::RELATION_CREATE, json!({"input": input}))?;
        doc["relation"]["id"] = json!(str_at(&out["issueRelationCreate"]["issueRelation"], "id"));
    } else {
        cl.query(run, queries::RELATION_DELETE, json!({"id": id}))?;
    }
    run.output(doc, &format!("{verb}d: {text}"))
}

/// Every relation between a and the issue with id b, either direction, as
/// relation reports one.
fn between(a: &Value, b: &str) -> Vec<Value> {
    let ai = str_at(a, "identifier");
    let mut out = vec![];
    for r in super::issue::nodes(&a["relations"]) {
        if str_at(&r, "relatedIssue.id") == b {
            out.push(json!({"id": str_at(&r, "id"), "type": str_at(&r, "type"), "issue": ai, "relatedIssue": str_at(&r, "relatedIssue.identifier")}));
        }
    }
    for r in super::issue::nodes(&a["inverseRelations"]) {
        if str_at(&r, "issue.id") == b {
            out.push(json!({"id": str_at(&r, "id"), "type": str_at(&r, "type"), "issue": str_at(&r, "issue.identifier"), "relatedIssue": ai}));
        }
    }
    out
}

/// relation delete ISSUE OTHER: removes every relation between the two.
/// One that fails leaves the rest reported, as a partial result.
fn delete_all(run: &mut Run, cl: &crate::client::Client, dry: bool, a: &Value, b: &Value) -> R {
    let found = between(a, &str_at(b, "id"));
    let text = |r: &Value| format!("{} {} {}", str_at(r, "issue"), str_at(r, "type"), str_at(r, "relatedIssue"));
    let mut doc = json!({"changed": !found.is_empty(), "dryRun": dry, "relations": found});
    if found.is_empty() {
        return run.output(doc, &format!("no relation: {} and {}", str_at(a, "identifier"), str_at(b, "identifier")));
    }
    if dry {
        let lines: Vec<String> = found.iter().map(|r| format!("would delete: {}", text(r))).collect();
        return run.output(doc, &lines.join("\n"));
    }
    let (mut done, mut problems) = (vec![], vec![]);
    for r in &found {
        match cl.query(run, queries::RELATION_DELETE, json!({"id": str_at(r, "id")})) {
            Ok(_) => done.push(r.clone()),
            Err(e) => problems.push(json!({"input": str_at(r, "id"), "error": e.to_string()})),
        }
    }
    doc["changed"] = json!(!done.is_empty());
    doc["relations"] = json!(done);
    let lines: Vec<String> = done.iter().map(|r| format!("deleted: {}", text(r))).collect();
    if problems.is_empty() {
        return run.output(doc, &lines.join("\n"));
    }
    doc["problems"] = json!(problems);
    run.output(doc, &lines.join("\n"))?;
    for p in &problems {
        run.eprint(&format!("linearctl: relation {}: {}\n", str_at(p, "input"), str_at(p, "error")));
    }
    Err(Fail::Partial(format!("{} relations not deleted", problems.len())))
}
