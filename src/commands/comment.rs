use super::issue::{chosen_id, first_of, landed, nodes};
use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::fail::{self, Fail, R, usage};
use crate::json::quote_go;
use crate::queries;
use crate::run::{Run, dry_run};
use crate::util::*;
use serde_json::{Value, json};

const FIELDS: &[&str] = &["id", "body", "createdAt", "url", "user", "parent"];

pub fn command() -> Cmd {
    Cmd::group(
        "comment",
        "List and create comments on an issue",
        vec![list_cmd(), create_cmd(), update_cmd(), delete_cmd()],
    )
}

fn list_cmd() -> Cmd {
    let c = Cmd::new("list ISSUE", "List an issue's comments, oldest first")
        .example("  linearctl comment list ENG-123 --json\n  linearctl comment list ENG-123 --limit 0 --fields body,user --json")
        .args(Args::Exact(1))
        .run(list)
        .flag(limit_flag(50))
        .flag(cursor_flag());
    fields_flag_spec(c, FIELDS)
}

fn list(run: &mut Run, ctx: &Ctx) -> R {
    let r = issue_ref("ISSUE", &ctx.args[0])?;
    let fields = fields_flag(ctx, FIELDS)?;
    let cursor = cursor_value(ctx)?;
    let cl = run.client()?;
    let (items, next) = collect(ctx.whole("limit"), cursor, |first, after| {
        let mut data = cl
            .query(run, queries::COMMENT_LIST, json!({"id": r, "first": first, "after": optional(after)}))
            .map_err(|e| e.not_found(&format!("issue {r}"), fail::FIND_ISSUE))?;
        Ok(Page::take(&mut data["issue"]["comments"]))
    })?;
    if run.json {
        let doc = list_doc(run, items, &next, &fields);
        return run.print_json(doc);
    }
    more_note(run, items.len(), &next);
    for it in &items {
        run.print(&format!(
            "— {}, {}\n{}\n\n",
            printable(&str_at(it, "user.name")),
            str_at(it, "createdAt"),
            printable_text(&str_at(it, "body"))
        ))?;
    }
    Ok(())
}

fn create_cmd() -> Cmd {
    Cmd::new("create ISSUE", "Add a comment to an issue")
        .long(
            "Add a Markdown comment to an issue.\n\n\
--yes requires --id, a UUID you choose, so a rerun can never post twice: if a\n\
comment with that id exists, it is returned with changed: false.\n\
--dry-run without --id generates one and prints it as id; pass it to --yes.\n\
--parent replies in a comment's thread.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl comment create ENG-123 --body \"Fixed in #42\" --dry-run --json\n  git log -1 --format=%B | linearctl comment create ENG-123 --body-file - --id 5f1e… --yes",
        )
        .args(Args::Exact(1))
        .run(create)
        .flag(Flag::str("body", "the comment, Markdown `TEXT`"))
        .flag(Flag::str("body-file", "read the comment from `PATH` (- for stdin)"))
        .flag(Flag::str("id", "the new comment's `UUID`; required with --yes"))
        .flag(Flag::str("parent", "reply in the thread of comment `ID`"))
        .mutating()
}

/// --body or --body-file, not empty.
fn body(ctx: &Ctx) -> R<String> {
    match text(ctx, "body")? {
        Some(b) if !b.trim().is_empty() => Ok(b),
        _ => Err(usage("pass --body TEXT or --body-file PATH (- for stdin), not empty")),
    }
}

fn create(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let r = issue_ref("ISSUE", &ctx.args[0])?;
    let id = chosen_id(ctx, dry, "post twice")?;
    let body = body(ctx)?;
    let parent = ctx.str("parent");
    if !parent.is_empty() && !is_uuid(&parent) {
        return Err(usage(format!(
            "--parent must be a comment id (a UUID, as comment list shows); got {}",
            quote_go(&parent)
        )));
    }
    let cl = run.client()?;
    let vars = json!({"issue": r, "id": id});
    let res = cl
        .query(run, queries::COMMENT_RESOLVE, vars.clone())
        .map_err(|e| e.not_found(&format!("issue {r}"), fail::FIND_ISSUE))?;
    let exists = |run: &mut Run, c: Value| -> R {
        let it = flatten(c);
        let text = format!("exists {}", str_at(&it, "url"));
        run.output(json!({"changed": false, "dryRun": dry, "id": id, "comment": it}), &text)
    };
    if let Some(c) = nodes(&res["comments"]).into_iter().next() {
        return exists(run, c);
    }
    let mut input = json!({"id": id, "issueId": str_at(&res["issue"], "id"), "body": body});
    if !parent.is_empty() {
        input["parentId"] = json!(parent.to_lowercase());
    }
    if dry {
        let text = format!("would comment on {}\nid: {id} (pass --id {id} --yes)", str_at(&res["issue"], "identifier"));
        return run.output(json!({"changed": true, "dryRun": true, "id": id, "input": input}), &text);
    }
    let out = match cl.query(run, queries::COMMENT_CREATE, json!({"input": input})) {
        Ok(out) => out,
        Err(e) => {
            let c = landed(run, &cl, e, queries::COMMENT_RESOLVE, &vars, first_of("comments"))?;
            return exists(run, c);
        }
    };
    let it = flatten(out["commentCreate"]["comment"].clone());
    let text = format!("commented {}", str_at(&it, "url"));
    run.output(json!({"changed": true, "dryRun": false, "id": id, "comment": it}), &text)
}

/// A COMMENT argument: a comment UUID, as comment list shows it.
fn comment_id(s: &str) -> R<String> {
    plain("COMMENT", s)?;
    if !is_uuid(s) {
        return Err(usage(format!(
            "COMMENT must be a comment id (a UUID, as comment list shows); got {}",
            quote_go(s)
        )));
    }
    Ok(s.to_lowercase())
}

fn update_cmd() -> Cmd {
    Cmd::new("update COMMENT", "Replace a comment's text")
        .long(
            "Replace a comment's Markdown text. The text is sent also when the comment\n\
already seems to have it: Linear can answer a read from before its last change.\n\
changed says whether the comment as read differs.\n\
--if-updated-at TIME refuses (exit 7, conflict) if the comment changed since you\n\
read it. It also proves the read current, so the same text again sends nothing.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl comment update 5f1e… --body-file note.md --dry-run --json\n  linearctl comment update 5f1e… --body \"Fixed in #43\" --yes",
        )
        .args(Args::Exact(1))
        .run(update)
        .flag(Flag::str("body", "the new text, Markdown `TEXT`"))
        .flag(Flag::str("body-file", "read the new text from `PATH` (- for stdin)"))
        .flag(super::issue::if_updated_at_flag())
        .mutating()
}

fn update(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let id = comment_id(&ctx.args[0])?;
    let body = body(ctx)?;
    let since = super::issue::if_updated_at(ctx, "comment")?;
    let cl = run.client()?;
    let res = cl.query(run, queries::COMMENT, json!({"id": id})).map_err(|e| {
        e.not_found(&format!("comment {id}"), "'linearctl comment list ISSUE' lists an issue's comment ids")
    })?;
    let comment = &res["comment"];
    let url = str_at(comment, "url");
    // As issue update: the read may lag a write Linear confirmed, so text it
    // already shows is still sent, unless --if-updated-at proves it current.
    let current_read = super::issue::read_is_current(
        &since,
        &str_at(comment, "updatedAt"),
        &format!("comment {id}"),
        "'linearctl comment list ISSUE', for its issue,",
    )?;
    let changed = str_at(comment, "body") != body;
    let mut doc = json!({"changed": changed, "dryRun": dry, "id": id});
    if !changed && current_read {
        doc["comment"] = flatten(comment.clone());
        return run.output(doc, &format!("unchanged {url}"));
    }
    if dry {
        doc["input"] = json!({"body": body});
        let text = if changed {
            format!("would update {url}")
        } else {
            format!("unchanged {url}, as read; would send it all the same")
        };
        return run.output(doc, &text);
    }
    let out = cl.query(run, queries::COMMENT_UPDATE, json!({"id": id, "input": {"body": body}}))?;
    let updated = &out["commentUpdate"]["comment"];
    doc["comment"] = flatten(updated.clone());
    let verb = if changed { "updated" } else { "unchanged" };
    run.output(doc, &format!("{verb} {}", str_at(updated, "url")))
}

fn delete_cmd() -> Cmd {
    Cmd::new("delete COMMENT", "Delete a comment")
        .long(
            "Delete a comment. A comment that is not there (already deleted, or never was)\n\
is changed: false with found: false.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example("  linearctl comment delete 5f1e… --dry-run\n  linearctl comment delete 5f1e… --yes --json")
        .args(Args::Exact(1))
        .run(delete)
        .mutating()
}

fn delete(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let id = comment_id(&ctx.args[0])?;
    let cl = run.client()?;
    let res = match cl.query(run, queries::COMMENT, json!({"id": id})) {
        Err(Fail::Linear { not_found: true, .. }) => {
            return run.output(
                json!({"changed": false, "dryRun": dry, "id": id, "found": false}),
                &format!("no comment {id}"),
            );
        }
        r => r?,
    };
    if !dry {
        cl.query(run, queries::COMMENT_DELETE, json!({"id": id}))?;
    }
    let url = str_at(&res["comment"], "url");
    let verb = if dry { "would delete " } else { "deleted " };
    run.output(json!({"changed": true, "dryRun": dry, "id": id, "found": true, "url": url}), &format!("{verb}{url}"))
}
