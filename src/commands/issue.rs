use crate::cli::{Args, Cmd, Ctx, Flag};
use crate::fail::{self, Fail, R, ambiguous, failed, missing, usage};
use crate::json::{marshal, quote_go};
use crate::queries::{self, ISSUE_ROW_EXTRAS, ISSUE_ROW_FIELDS};
use crate::run::{Run, dry_run};
use crate::time;
use crate::util::*;
use serde_json::{Map, Value, json};

pub fn show_fields() -> Vec<&'static str> {
    let mut f = ISSUE_ROW_FIELDS.to_vec();
    f.extend([
        "description",
        "branchName",
        "startedAt",
        "completedAt",
        "canceledAt",
        "archivedAt",
        "creator",
        "parent",
        "children",
        "relations",
        "inverseRelations",
        "attachments",
        "comments",
    ]);
    f
}

/// issue list's --fields: the row's, and what a row carries only when asked.
fn list_fields() -> Vec<&'static str> {
    let mut f = ISSUE_ROW_FIELDS.to_vec();
    f.extend(ISSUE_ROW_EXTRAS);
    f
}

pub fn command() -> Cmd {
    Cmd::group(
        "issue",
        "List, show, create, update and archive issues",
        vec![list_cmd(), show_cmd(), create_cmd(), update_cmd(), archive_cmd(true), archive_cmd(false)],
    )
}

fn list_cmd() -> Cmd {
    let c = Cmd::new("list", "List issues, most recently updated first")
        .long(
            "List issues, most recently updated first. Filters combine with AND; a repeated\n\
--state or --state-type matches any of its values, a repeated --label all of them;\n\
--any-label matches any of its values, --without-label none of them.\n\
--query searches titles, descriptions and comments, ranked by relevance unless\n\
--sort is given; Linear allows 30 searches a minute. With one --team, it searches\n\
that team, as Linear's team search does.\n\
--team falls back to LINEARCTL_TEAM, then to default-team in config.\n\
parent, relations and inverseRelations are in a row only when --fields names\n\
them, as issue show has them (each side of a relation with its state); they\n\
cost one more request per 250 rows, and one per issue with more than 250\n\
relations.",
        )
        .example(
            "  linearctl issue list --assignee me --state-type started --json\n  linearctl issue list --team ENG --label bug --updated-since 7d --fields identifier,title --json\n  linearctl issue list --query \"login timeout\" --limit 10\n  linearctl issue list --project \"Q4 launch\" --limit 0 --fields identifier,state,parent,inverseRelations --json",
        )
        .run(list);
    let mut c = filter_flags(c)
        .flag(Flag::strs(
            "id",
            "only the issue `ISSUE` (ENG-123 or a UUID; repeatable); one not\nfound is a problem (exit 3)",
        ))
        .flag(Flag::str("query", "full-text search for `TEXT`"))
        .flag(Flag::bool("include-comments", "with --query, also match the text of comments"))
        .flag(Flag::str("view", "only issues in Linear's saved custom view `VIEW` (a name or its id)"))
        .flag(Flag::str("sort", "`FIELD`: updated (default), created, priority, due-date, title, state"))
        .flag(Flag::bool("reverse", "reverse the order of --sort"))
        .flag(limit_flag(25))
        .flag(cursor_flag());
    c = fields_flag_spec(c, &list_fields());
    c
}

/// The flags list_filter reads: issue list's and watch's.
pub fn filter_flags(c: Cmd) -> Cmd {
    c.flag(Flag::strs("team", "only issues of team `KEY` (e.g. ENG; repeatable, any of)"))
        .flag(Flag::strs("priority", "only issues of `PRIORITY`: 0-4 or none, urgent, high, medium, low\n(repeatable, any of)"))
        .flag(Flag::str("assignee", "only issues assigned to `WHO`: me, none, or an email"))
        .flag(Flag::strs("state", "only issues in workflow state `NAME` (repeatable; any of)"))
        .flag(Flag::strs(
            "state-type",
            "only issues whose state is of `TYPE` (repeatable): triage, backlog,\nunstarted, started, completed, canceled, duplicate",
        ))
        .flag(Flag::strs("label", "only issues with label `NAME` (repeatable; all of)"))
        .flag(Flag::strs("any-label", "only issues with label `NAME` (repeatable; any of)"))
        .flag(Flag::strs("without-label", "only issues without label `NAME` (repeatable; none of)"))
        .flag(Flag::str("project", "only issues in project `NAME` (none: in no project)"))
        .flag(Flag::str("milestone", "only issues in project milestone `NAME` (none: in no milestone)"))
        .flag(Flag::str("delegate", "only issues delegated to the agent `WHO` (name, email, me, any, none)"))
        .flag(Flag::str("attachment-url", "only issues with an attachment at `URL` (e.g. a linked pull request)"))
        .flag(Flag::str("cycle", "only issues in cycle `CYCLE`: current, next, previous, a number, a UUID"))
        .flag(Flag::str("updated-since", "only issues updated since `WHEN`: a date, an RFC 3339 time, 7d or 24h"))
        .flag(Flag::str("creator", "only issues created by `WHO`: me, none, or an email"))
        .flag(Flag::str("completed-since", "only issues completed since `WHEN`: a date, an RFC 3339 time, 7d or 24h"))
        .flag(Flag::bool("blocked", "only issues blocked by an open issue"))
        .flag(Flag::bool("blocking", "only issues that block another"))
        .flag(Flag::str("due-before", "only issues due on or before `DATE` (2026-11-30)"))
        .flag(Flag::str("due-after", "only issues due on or after `DATE`"))
}

fn list(run: &mut Run, ctx: &Ctx) -> R {
    let fields = fields_flag(ctx, &list_fields())?;
    let mut filter = list_filter(ctx)?;
    let cursor = cursor_value(ctx)?;
    let query = ctx.str("query");
    let sort = ctx.str("sort");
    let reverse = ctx.bool("reverse");
    // Each --sort as Linear's IssueSortInput field and its natural order.
    // Descending priority is urgent first with no priority last; Ascending
    // puts no priority (0) first (verified 2026-10-26).
    let (field, natural) = match sort.as_str() {
        "" | "updated" => ("updatedAt", "Descending"),
        "created" => ("createdAt", "Descending"),
        "priority" => ("priority", "Descending"),
        "due-date" => ("dueDate", "Ascending"),
        "title" => ("title", "Ascending"),
        "state" => ("workflowState", "Ascending"),
        _ => {
            return Err(usage(format!(
                "--sort must be updated, created, priority, due-date, title or state; got {}",
                quote_go(&sort)
            )));
        }
    };
    let plain_order = matches!(sort.as_str(), "" | "updated" | "created") && !reverse;
    if !query.is_empty() {
        plain("--query", &query)?;
        if !plain_order {
            return Err(usage("--query sorts only by updated or created, newest first; drop --sort or --reverse"));
        }
    }
    let mut vars = Map::new();
    if plain_order {
        // As before: orderBy, newest first; a search without --sort ranks by
        // relevance.
        if query.is_empty() || !sort.is_empty() {
            vars.insert("orderBy".into(), json!(field));
        }
    } else {
        let order = match (natural, reverse) {
            ("Descending", false) | ("Ascending", true) => "Descending",
            _ => "Ascending",
        };
        // Issues without a due date come last either way.
        let spec = if field == "dueDate" { json!({"order": order, "nulls": "last"}) } else { json!({"order": order}) };
        vars.insert("sort".into(), json!([{ field: spec }]));
    }
    let (mut doc, mut conn) = (queries::ISSUE_LIST, "issues");
    let view = ctx.str("view");
    if ctx.changed("view") {
        plain("--view", &view)?;
        if !query.is_empty() {
            return Err(usage("--view lists a saved view's issues; it does not search: drop --query"));
        }
        let by = if is_uuid(&view) {
            json!({"id": {"eq": view.to_lowercase()}})
        } else {
            json!({"name": {"eqIgnoreCase": view}})
        };
        vars.insert("view".into(), by);
        doc = queries::ISSUE_LIST_VIEW;
    }
    let with_comments = ctx.bool("include-comments");
    if with_comments && query.is_empty() {
        return Err(usage("--include-comments searches comments too, so it needs --query"));
    }
    if !query.is_empty() {
        (doc, conn) = (queries::ISSUE_SEARCH, "searchIssues");
        vars.insert("term".into(), json!(query));
        if with_comments {
            vars.insert("includeComments".into(), json!(true));
        }
    }
    let cl = run.client()?;
    // A search of one team passes it as searchIssues' teamId too: Linear
    // then searches that team, which finds hits a workspace search filtered
    // to the team misses (measured 2026-10-27). teamId takes one team.
    if let (false, [team]) = (query.is_empty(), teams_flag(ctx)?.as_slice()) {
        let found = cl.query(run, queries::TEAM_FIND, json!({"key": team}))?;
        if let Some(t) = nodes(&found["teams"]).first() {
            vars.insert("teamId".into(), json!(str_at(t, "id")));
        }
    }
    let mut limit = ctx.whole("limit");
    let ids = ids_flag(ctx)?;
    if !ids.is_empty() {
        filter.get_or_insert_with(Map::new).insert("id".into(), json!({"in": ids}));
        if !ctx.changed("limit") {
            limit = 0; // the ids bound the list
        }
    }
    vars.insert("filter".into(), filter.map_or(Value::Null, Value::Object));
    let (mut items, next) = collect(limit, cursor, |first, after| {
        vars.insert("first".into(), json!(first));
        vars.insert("after".into(), optional(after));
        let mut data = cl.query(run, doc, Value::Object(vars.clone()))?;
        complete_labels(run, &cl, &mut data)?;
        if doc == queries::ISSUE_LIST_VIEW {
            let found = nodes(&data["matches"]);
            match found.as_slice() {
                [] => {
                    return Err(missing(format!("no saved view {}; see 'linearctl view list'", quote_go(&view)))
                        .with(sought("view", &view)));
                }
                [v] if str_at(v, "modelName") != "Issue" => {
                    return Err(usage(format!(
                        "view {} lists {}s, not issues",
                        quote_go(&str_at(v, "name")),
                        str_at(v, "modelName").to_lowercase()
                    )));
                }
                [_] => {}
                _ => {
                    return Err(ambiguous(format!(
                        "more than one saved view is named {}; pass its id",
                        quote_go(&view)
                    ))
                    .with(sought("view", &view))
                    .with(json!({"candidates": candidates(&found)})));
                }
            }
            return Ok(Page::take(&mut data["customViews"]["nodes"][0]["issues"]));
        }
        Ok(Page::take(&mut data[conn]))
    })?;
    add_relations(run, &cl, &mut items, &fields)?;
    // Asked-for issues that did not come back: partial, never silent.
    let problems: Vec<Value> = ids
        .iter()
        .filter(|r| !items.iter().any(|it| str_at(it, "identifier") == **r || str_at(it, "id") == **r))
        .map(|r| json!({"input": r, "error": "not found (or this API key cannot see it)"}))
        .collect();
    if run.json {
        let mut out = list_doc(run, items, &next, &fields);
        if !problems.is_empty() {
            out["problems"] = json!(problems);
        }
        run.print_json(out)?;
    } else {
        more_note(run, items.len(), &next);
        rows(run, &items, &["identifier", "state.name", "assignee.name", "title"])?;
    }
    for p in &problems {
        run.eprint(&format!("linearctl: {}: {}\n", str_at(p, "input"), str_at(p, "error")));
    }
    if !problems.is_empty() {
        return Err(Fail::Partial(format!("{} issues not found", problems.len())));
    }
    Ok(())
}

/// issue list's --id values as Linear's id filter takes them.
fn ids_flag(ctx: &Ctx) -> R<Vec<String>> {
    let vs = values(ctx, "id")?;
    if vs.len() as u64 > PAGE_SIZE {
        return Err(usage(format!("--id takes at most {PAGE_SIZE} issues; got {}", vs.len())));
    }
    let mut ids: Vec<String> = vec![];
    for v in vs {
        let r = issue_ref("--id", &v)?;
        if !ids.contains(&r) {
            ids.push(r);
        }
    }
    Ok(ids)
}

const STATE_TYPES: &[&str] = &["triage", "backlog", "unstarted", "started", "completed", "canceled", "duplicate"];

/// Linear's IssueFilter for issue list's flags; None when there is none.
/// Names go to Linear as names, so listing needs no lookups.
pub fn list_filter(ctx: &Ctx) -> R<Option<Map<String, Value>>> {
    let mut filter = Map::new();
    let mut and: Vec<Value> = vec![];
    let teams = teams_flag(ctx)?;
    match teams.len() {
        0 => {}
        1 => _ = filter.insert("team".into(), json!({"key": {"eq": teams[0]}})),
        _ => _ = filter.insert("team".into(), json!({"key": {"in": teams}})),
    }
    let prios = values(ctx, "priority")?;
    if !prios.is_empty() {
        let ns = prios.iter().map(|p| priority("--priority", p)).collect::<R<Vec<i64>>>()?;
        filter.insert("priority".into(), json!({"in": ns}));
    }
    let who = ctx.str("assignee");
    if !who.is_empty() {
        filter.insert("assignee".into(), user_filter("--assignee", &who)?);
    }
    // State names match ignoring case, as issue create and update resolve
    // them; Linear has no case-insensitive "in", so more than one is an or.
    let mut state = Map::new();
    let names = values(ctx, "state")?;
    match names.as_slice() {
        [] => {}
        [n] => _ = state.insert("name".into(), json!({"eqIgnoreCase": n})),
        _ => {
            let or: Vec<Value> = names.iter().map(|n| json!({"state": {"name": {"eqIgnoreCase": n}}})).collect();
            and.push(json!({"or": or}));
        }
    }
    let types = values(ctx, "state-type")?;
    if !types.is_empty() {
        for t in &types {
            if !STATE_TYPES.contains(&t.as_str()) {
                return Err(usage(format!(
                    "--state-type must be one of {}; got {}",
                    STATE_TYPES.join(", "),
                    quote_go(t)
                )));
            }
        }
        state.insert("type".into(), json!({"in": types}));
    }
    if !state.is_empty() {
        filter.insert("state".into(), Value::Object(state));
    }
    for l in values(ctx, "label")? {
        and.push(json!({"labels": {"some": {"name": {"eqIgnoreCase": l}}}}));
    }
    let any_of = values(ctx, "any-label")?;
    if !any_of.is_empty() {
        let or: Vec<Value> =
            any_of.iter().map(|l| json!({"labels": {"some": {"name": {"eqIgnoreCase": l}}}})).collect();
        and.push(json!({"or": or}));
    }
    // "every label differs" also holds for an issue with no labels.
    for l in values(ctx, "without-label")? {
        and.push(json!({"labels": {"every": {"name": {"neqIgnoreCase": l}}}}));
    }
    if !and.is_empty() {
        filter.insert("and".into(), Value::Array(and));
    }
    let p = ctx.str("project");
    if !p.is_empty() {
        plain("--project", &p)?;
        let f = if p == "none" { json!({"null": true}) } else { json!({"name": {"eqIgnoreCase": p}}) };
        filter.insert("project".into(), f);
    }
    let u = ctx.str("attachment-url");
    if !u.is_empty() {
        plain("--attachment-url", &u)?;
        if !u.starts_with("https://") && !u.starts_with("http://") {
            return Err(usage(format!("--attachment-url must be an http(s) URL; got {}", quote_go(&u))));
        }
        filter.insert("attachments".into(), json!({"some": {"url": {"eq": u}}}));
    }
    let d = ctx.str("delegate");
    if !d.is_empty() {
        plain("--delegate", &d)?;
        let f = match d.as_str() {
            "none" => json!({"null": true}),
            "any" => json!({"null": false}),
            "me" => json!({"isMe": {"eq": true}}),
            _ => {
                json!({"or": [{"name": {"eqIgnoreCase": d}}, {"displayName": {"eqIgnoreCase": d}}, {"email": {"eqIgnoreCase": d}}]})
            }
        };
        filter.insert("delegate".into(), f);
    }
    let m = ctx.str("milestone");
    if !m.is_empty() {
        plain("--milestone", &m)?;
        let f = if m == "none" { json!({"null": true}) } else { json!({"name": {"eqIgnoreCase": m}}) };
        filter.insert("projectMilestone".into(), f);
    }
    let cy = ctx.str("cycle");
    if !cy.is_empty() {
        filter.insert("cycle".into(), super::cycle::cycle_ref("--cycle", &cy)?);
    }
    let done = ctx.str("completed-since");
    if ctx.changed("completed-since") {
        let at = when(&done).map_err(|_| {
            usage(format!(
                "--completed-since must be a date (2026-10-01), an RFC 3339 time, or 7d / 24h; got {}",
                quote_go(&done)
            ))
        })?;
        filter.insert("completedAt".into(), json!({"gte": at}));
    }
    if ctx.bool("blocked") {
        filter.insert("hasBlockedByRelations".into(), json!({"eq": true}));
    }
    if ctx.bool("blocking") {
        filter.insert("hasBlockingRelations".into(), json!({"eq": true}));
    }
    let creator = ctx.str("creator");
    if !creator.is_empty() {
        filter.insert("creator".into(), user_filter("--creator", &creator)?);
    }
    let mut due = Map::new();
    for (flag, op) in [("due-before", "lte"), ("due-after", "gte")] {
        let d = ctx.str(flag);
        if ctx.changed(flag) && (d.len() != 10 || crate::time::parse_date(&d).is_none()) {
            return Err(usage(format!("--{flag} must be a date (2026-11-30); got {}", quote_go(&d))));
        }
        if !d.is_empty() {
            due.insert(op.into(), json!(d));
        }
    }
    if !due.is_empty() {
        filter.insert("dueDate".into(), Value::Object(due));
    }
    let since = ctx.str("updated-since");
    if !since.is_empty() {
        filter.insert("updatedAt".into(), json!({"gte": when(&since)?}));
    }
    Ok((!filter.is_empty()).then_some(filter))
}

/// A NullableUserFilter for me, none or an email.
fn user_filter(flag: &str, who: &str) -> R<Value> {
    match who {
        "me" => Ok(json!({"isMe": {"eq": true}})),
        "none" => Ok(json!({"null": true})),
        w if is_email(w) => Ok(json!({"email": {"eqIgnoreCase": w}})),
        _ => Err(usage(format!("{flag} must be me, none or an email; got {}", quote_go(who)))),
    }
}

/// --updated-since as an RFC 3339 time: a date, a time, or 7d / 24h.
pub fn when(s: &str) -> R<String> {
    let b = s.as_bytes();
    if (2..=5).contains(&b.len())
        && b[..b.len() - 1].iter().all(u8::is_ascii_digit)
        && matches!(b[b.len() - 1], b'd' | b'h')
    {
        let n: i64 = s[..s.len() - 1].parse().unwrap_or(0);
        let unit = if b[b.len() - 1] == b'd' { 24 * time::HOUR } else { time::HOUR };
        return Ok(time::rfc3339(time::now() - n * unit));
    }
    if let Some(t) = time::parse_date(s) {
        return Ok(time::rfc3339(t));
    }
    if let Some(t) = time::parse_rfc3339(s) {
        return Ok(time::rfc3339(t));
    }
    Err(usage(format!(
        "--updated-since must be a date (2026-10-01), an RFC 3339 time, or 7d / 24h; got {}",
        quote_go(s)
    )))
}

fn show_cmd() -> Cmd {
    let c = Cmd::new("show ISSUE", "Show one issue with its description, comments, sub-issues and relations")
        .alias("view") // gh's word, which agents reach for first
        .long(
            "Show one issue with its description, comments, sub-issues and relations.\n\
ISSUE is an identifier (ENG-123), a UUID, or a linear.app issue URL.\n\
--signed-urls makes file URLs in the result open without the API key for that\n\
many seconds, so a viewer can fetch images directly; signedUrlsExpireAt says until when.\n\
Comments are included (20 by default); give another count as --comments=N.",
        )
        .example(
            "  linearctl issue show ENG-123\n  linearctl issue view ENG-123 --signed-urls 300 --json\n  linearctl issue show ENG-123 --comments=0 --fields identifier,title,description --json",
        )
        .args(Args::Custom(|ctx| {
            // --comments takes its count with =, so "--comments 5" leaves 5
            // as an argument; say how to write it rather than "accepts 1 arg".
            let a = &ctx.args;
            if a.len() == 2 && ctx.changed("comments") && !a[1].is_empty() && a[1].bytes().all(|c| c.is_ascii_digit()) {
                return Err(format!("give the count as --comments={}", a[1]));
            }
            if a.len() != 1 {
                return Err(format!("accepts 1 arg(s), received {}", a.len()));
            }
            Ok(())
        }))
        .run(show)
        .flag(Flag::whole("comments", 20, "include the first `N` comments (--comments=N); 0 for none").no_opt("20"))
        .flag(Flag::whole("signed-urls", 0, "sign file URLs for `SECONDS` so they open without the key"));
    fields_flag_spec(c, &show_fields())
}

fn show(run: &mut Run, ctx: &Ctx) -> R {
    let r = issue_ref("ISSUE", &ctx.args[0])?;
    let fields = fields_flag(ctx, &show_fields())?;
    let comments = ctx.whole("comments");
    if comments > 250 {
        return Err(usage("--comments is at most 250; page with 'linearctl comment list'"));
    }
    let cl = run.client()?;
    cl.signed_urls.set(ctx.whole("signed-urls"));
    let expires = cl.signed_until();
    let data = cl
        .query(run, queries::ISSUE_SHOW, json!({"id": r, "comments": comments, "withComments": comments > 0}))
        .map_err(|e| e.not_found(&format!("issue {r}"), fail::FIND_ISSUE))?;
    let mut data = data;
    let mut issue = data["issue"].take();
    if let Some(m) = issue.as_object_mut() {
        m.shift_remove("labelIds"); // read_all reads every label
    }
    read_all(run, &cl, &mut issue)?;
    let issue = flatten(issue);
    if run.json {
        let mut doc = object([("issue", pick(issue, &fields))]);
        if !expires.is_null() {
            doc["signedUrlsExpireAt"] = expires;
        }
        return run.print_json(doc);
    }
    let mut s = format!("{}  {}\n", str_at(&issue, "identifier"), printable(&str_at(&issue, "title")));
    s.push_str(&format!(
        "state: {}   assignee: {}   priority: {}\n",
        printable(&str_at(&issue, "state.name")),
        or_dash(printable(&str_at(&issue, "assignee.name"))),
        str_at(&issue, "priorityLabel")
    ));
    s.push_str(&format!("{}\n", str_at(&issue, "url")));
    let d = str_at(&issue, "description");
    if !d.is_empty() {
        s.push_str(&format!("\n{}\n", printable_text(&d)));
    }
    if let Some(cs) = issue["comments"].as_array().filter(|c| !c.is_empty()) {
        s.push_str(&format!("\n{} comments\n", cs.len()));
        for c in cs {
            s.push_str(&format!(
                "\n— {}, {}\n{}\n",
                printable(&str_at(c, "user.name")),
                str_at(c, "createdAt"),
                printable_text(&str_at(c, "body"))
            ));
        }
    }
    run.print(&s)
}

/// issue show's collections: the field in the answer, and the variables
/// IssueShowMore takes for its cursor and for whether to read it.
const SHOW_CONNECTIONS: &[(&str, &str, &str)] = &[
    ("allLabels", "labels", "withLabels"),
    ("children", "children", "withChildren"),
    ("relations", "relations", "withRelations"),
    ("inverseRelations", "inverse", "withInverse"),
    ("attachments", "attachments", "withAttachments"),
];

/// Reads the rest of each collection issue show has the first page of,
/// so none is cut short, then leaves each as {nodes} for flatten. The
/// labels come back under their own name.
fn read_all(run: &mut Run, cl: &crate::client::Client, issue: &mut Value) -> R {
    let more = |issue: &Value, field: &str| issue[field]["pageInfo"]["hasNextPage"] == json!(true);
    while SHOW_CONNECTIONS.iter().any(|(field, ..)| more(issue, field)) {
        let mut vars = json!({"id": str_at(issue, "id")});
        for (field, cursor, with) in SHOW_CONNECTIONS {
            let next = more(issue, field);
            vars[*cursor] = if next { issue[*field]["pageInfo"]["endCursor"].clone() } else { Value::Null };
            vars[*with] = json!(next);
        }
        let mut page = cl.query(run, queries::ISSUE_SHOW_MORE, vars)?;
        for (field, ..) in SHOW_CONNECTIONS {
            if !more(issue, field) {
                continue;
            }
            let got = &mut page["issue"][*field];
            let nodes = got["nodes"].as_array_mut().map(std::mem::take).unwrap_or_default();
            if let Some(have) = issue[*field]["nodes"].as_array_mut() {
                have.extend(nodes);
            }
            issue[*field]["pageInfo"] = got["pageInfo"].take();
        }
    }
    let Some(m) = issue.as_object_mut() else { return Ok(()) };
    for (field, ..) in SHOW_CONNECTIONS {
        if let Some(Value::Object(c)) = m.get_mut(*field) {
            c.shift_remove("pageInfo");
        }
    }
    if let Some(labels) = m.shift_remove("allLabels") {
        m.insert("labels".into(), labels);
    }
    Ok(())
}

/// Gives issue rows the parent, relations and inverse relations that
/// fields names, 250 rows to a request, each relation read in full. A row
/// whose issue the read did not find has them null.
fn add_relations(run: &mut Run, cl: &crate::client::Client, items: &mut [Value], fields: &[String]) -> R {
    let want: Vec<&str> = ISSUE_ROW_EXTRAS.iter().copied().filter(|f| fields.iter().any(|x| x == f)).collect();
    if want.is_empty() {
        return Ok(());
    }
    for rows in items.chunks_mut(PAGE_SIZE as usize) {
        let ids: Vec<String> = rows.iter().map(|it| str_at(it, "id")).collect();
        let vars = json!({"ids": ids, "withParent": want.contains(&"parent"),
            "withRelations": want.contains(&"relations"), "withInverse": want.contains(&"inverseRelations")});
        let mut data = cl.query(run, queries::ISSUE_RELATIONS, vars)?;
        let mut found = data["issues"]["nodes"].as_array_mut().map(std::mem::take).unwrap_or_default();
        for got in &mut found {
            read_all(run, cl, got)?;
        }
        for row in rows.iter_mut() {
            let got = found.iter_mut().find(|g| str_at(g, "id") == str_at(row, "id"));
            let mut got = got.map(Value::take).unwrap_or(Value::Null);
            for f in &want {
                row[*f] = got[*f].take();
            }
        }
    }
    Ok(())
}

/// The flags issue create and update share.
fn issue_fields(c: Cmd) -> Cmd {
    c.flag(Flag::str("title", "the title, `TEXT`"))
        .flag(Flag::str("description", "the description, Markdown `TEXT`"))
        .flag(Flag::str("description-file", "read the description from `PATH` (- for stdin)"))
        .flag(Flag::str("assignee", "assign to `WHO`: me, none, or an email"))
        .flag(Flag::str("state", "workflow state `NAME` (see 'linearctl state list')"))
        .flag(Flag::str("state-type", "the team's first workflow state of `TYPE` (e.g. started)"))
        .flag(Flag::str("estimate", "the estimate, `POINTS` (a whole number), or none"))
        .flag(Flag::str("due-date", "the due date, `DATE` (2026-11-30), or none"))
        .flag(Flag::str("cycle", "cycle `CYCLE` (current, next, previous, N, a UUID), or none"))
        .flag(Flag::str("priority", "`PRIORITY`: 0-4 or none, urgent, high, medium, low"))
        .flag(Flag::str("project", "project `NAME`, or none to clear it"))
        .flag(Flag::str(
            "milestone",
            "project milestone `NAME`, or none to clear; it sets the project\nwhen the issue has none",
        ))
        .flag(Flag::str("delegate", "delegate to the agent (app user) `WHO`: its name or email, me, or none"))
        .flag(Flag::str("parent", "make it a sub-issue of `ISSUE` (none: of no issue)"))
        .flag(Flag::str("json-input", "read Linear's input object from `PATH` (- for stdin); flags win"))
}

/// A priority as Linear numbers it: 0-4 or its name.
fn priority(flag: &str, p: &str) -> R<i64> {
    let named = match p.to_lowercase().as_str() {
        "none" => Some(0),
        "urgent" => Some(1),
        "high" => Some(2),
        "medium" => Some(3),
        "low" => Some(4),
        _ => None,
    };
    if let Some(n) = named {
        return Ok(n);
    }
    match go_atoi(p) {
        Some(n) if (0..=4).contains(&n) => Ok(n),
        _ => Err(usage(format!("{flag} must be 0-4 or none, urgent, high, medium, low; got {}", quote_go(p)))),
    }
}

/// strconv.Atoi: an optional sign, then decimal digits.
fn go_atoi(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// What the shared flags ask for, before names become ids.
#[derive(Default)]
pub struct Wanted {
    /// The input object so far (from --json-input and plain flags).
    pub input: Map<String, Value>,
    state: String,
    /// me, none or an email.
    assignee: String,
    labels: Vec<String>,
    project: String,
    /// An issue ref.
    parent: String,
    /// A name, or none.
    milestone: String,
    /// An agent user's name or email, me, or none.
    delegate: String,
    /// A state type (started, …): the team's first state of it.
    state_type: String,
    /// --cycle as a cycle filter, found among the issue's team's cycles.
    cycle: Option<Value>,
    cycle_word: String,
}

fn read_wanted(ctx: &Ctx) -> R<Wanted> {
    let mut w = Wanted::default();
    let path = ctx.str("json-input");
    if !path.is_empty() {
        let data = read_input("--json-input", &path)?;
        let v = crate::json::parse(&data).map_err(|e| usage(format!("--json-input is not a JSON object: {e}")))?;
        match v {
            Value::Object(m) => w.input = m,
            Value::Null => {}
            other => {
                let e = crate::json::type_error(&other, "", "map[string]interface {}");
                return Err(usage(format!("--json-input is not a JSON object: {e}")));
            }
        }
    }
    if ctx.changed("title") {
        let t = ctx.str("title");
        plain("--title", &t)?;
        w.input.insert("title".into(), json!(t));
    }
    if let Some(d) = text(ctx, "description")? {
        w.input.insert("description".into(), json!(d));
    }
    let p = ctx.str("priority");
    if !p.is_empty() {
        w.input.insert("priority".into(), json!(priority("--priority", &p)?));
    }
    for flag in ["state", "project", "milestone", "delegate"] {
        let v = ctx.str(flag);
        if v.is_empty() {
            continue;
        }
        plain(&format!("--{flag}"), &v)?;
        match flag {
            "state" => w.state = v,
            "project" => w.project = v,
            "milestone" => w.milestone = v,
            _ => w.delegate = v,
        }
    }
    let a = ctx.str("assignee");
    if !a.is_empty() {
        user_filter("--assignee", &a)?;
        w.assignee = a;
    }
    let st = ctx.str("state-type");
    if !st.is_empty() {
        if !STATE_TYPES.contains(&st.as_str()) {
            return Err(usage(format!(
                "--state-type must be one of {}; got {}",
                STATE_TYPES.join(", "),
                quote_go(&st)
            )));
        }
        if !w.state.is_empty() {
            return Err(usage("pass --state or --state-type, not both"));
        }
        w.state_type = st;
    }
    let est = ctx.str("estimate");
    if ctx.changed("estimate") {
        let v = match est.as_str() {
            "none" => Value::Null,
            n if !n.is_empty() && n.len() <= 6 && n.bytes().all(|c| c.is_ascii_digit()) => {
                json!(n.parse::<i64>().unwrap_or(0))
            }
            _ => {
                return Err(usage(format!(
                    "--estimate must be a whole number of points, or none; got {}",
                    quote_go(&est)
                )));
            }
        };
        w.input.insert("estimate".into(), v);
    }
    let due = ctx.str("due-date");
    if ctx.changed("due-date") {
        let v = match due.as_str() {
            "none" => Value::Null,
            d if d.len() == 10 && crate::time::parse_date(d).is_some() => json!(d),
            _ => return Err(usage(format!("--due-date must be a date (2026-11-30), or none; got {}", quote_go(&due)))),
        };
        w.input.insert("dueDate".into(), v);
    }
    let cy = ctx.str("cycle");
    if ctx.changed("cycle") {
        if cy == "none" {
            w.input.insert("cycleId".into(), Value::Null);
        } else {
            w.cycle = Some(super::cycle::cycle_ref("--cycle", &cy)?);
            w.cycle_word = cy;
        }
    }
    let p = ctx.str("parent");
    if p == "none" {
        w.input.insert("parentId".into(), Value::Null);
    } else if !p.is_empty() {
        w.parent = issue_ref("--parent", &p)?;
    }
    if ctx.has_flag("label") {
        w.labels = values(ctx, "label")?;
    }
    Ok(w)
}

/// Finds labels by name, ignoring case as issue list's --label does:
/// Linear's in comparator is case-sensitive.
fn label_filter(names: &[String]) -> Value {
    if names.is_empty() {
        return json!({});
    }
    json!({"or": names.iter().map(|n| json!({"name": {"eqIgnoreCase": n}})).collect::<Vec<_>>()})
}

impl Wanted {
    /// The variables both resolve queries share.
    pub fn resolve_vars(&self, extra_labels: &[String]) -> Map<String, Value> {
        let mut labels = self.labels.clone();
        labels.extend(extra_labels.iter().cloned());
        let parent = if self.parent.is_empty() { json!({}) } else { issue_filter(&self.parent) };
        let email = if is_email(&self.assignee) { self.assignee.clone() } else { String::new() };
        let v = json!({
            "state": self.state, "withState": !self.state.is_empty(),
            "email": email, "withUser": !email.is_empty(), "me": self.assignee == "me" || self.delegate == "me",
            "labels": label_filter(&labels), "withLabels": !labels.is_empty(),
            "project": self.project, "withProject": !self.project.is_empty() && self.project != "none",
            "parent": parent, "withParent": !self.parent.is_empty(),
            "milestone": self.milestone, "withMilestone": !self.milestone.is_empty() && self.milestone != "none",
            "delegate": self.delegate, "withDelegate": !matches!(self.delegate.as_str(), "" | "none" | "me"),
            "stateType": self.state_type, "withStateType": !self.state_type.is_empty(),
            "cycle": self.cycle.clone().unwrap_or(json!({})), "withCycle": self.cycle.is_some(),
        });
        match v {
            Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    /// Turns the resolved names into ids on input. team_id picks team
    /// labels over another team's label of the same name; project_id is the
    /// issue's project now ("" for a new issue): it picks the milestone
    /// when several projects have one of that name.
    /// typed are the team's states of --state-type: the one with the lowest
    /// position is the first, as Linear's agent guide picks "started".
    /// cycles are the team's cycles --cycle names.
    fn apply(
        &mut self,
        r: &Value,
        states: &[Value],
        typed: &[Value],
        cycles: &[Value],
        team_id: &str,
        project_id: &str,
    ) -> R {
        if self.cycle.is_some() {
            match cycles {
                [one] => _ = self.input.insert("cycleId".into(), json!(str_at(one, "id"))),
                [] => {
                    return Err(missing(format!(
                        "no cycle {} in the issue's team; see 'linearctl cycle list'",
                        quote_go(&self.cycle_word)
                    ))
                    .with(sought("cycle", &self.cycle_word)));
                }
                _ => {
                    return Err(ambiguous(format!("more than one cycle is {}", quote_go(&self.cycle_word)))
                        .with(sought("cycle", &self.cycle_word))
                        .with(json!({"candidates": candidates(cycles)})));
                }
            }
        }
        if !self.state.is_empty() {
            let id = one("state", "workflow state", &self.state, states, "'linearctl state list'")?;
            self.input.insert("stateId".into(), json!(id));
        }
        if !self.state_type.is_empty() {
            let pos = |v: &Value| v["position"].as_f64().unwrap_or(f64::MAX);
            let Some(first) = typed.iter().min_by(|a, b| pos(a).total_cmp(&pos(b))) else {
                return Err(missing(format!(
                    "the team has no workflow state of type {}; see 'linearctl state list'",
                    self.state_type
                )));
            };
            self.input.insert("stateId".into(), json!(str_at(first, "id")));
        }
        match self.assignee.as_str() {
            "" => {}
            "none" => _ = self.input.insert("assigneeId".into(), Value::Null),
            "me" => _ = self.input.insert("assigneeId".into(), json!(str_at(&r["viewer"], "id"))),
            a => {
                let id = one("user", "user with email", a, &nodes(&r["users"]), "'linearctl user list'")?;
                self.input.insert("assigneeId".into(), json!(id));
            }
        }
        if self.project == "none" {
            self.input.insert("projectId".into(), Value::Null);
        } else if !self.project.is_empty() {
            let id = one("project", "project", &self.project, &nodes(&r["projects"]), "'linearctl project list'")?;
            self.input.insert("projectId".into(), json!(id));
        }
        if !self.parent.is_empty() {
            let found = nodes(&r["parent"]);
            let Some(p) = found.first() else {
                return Err(missing(format!("not found: parent issue {}", self.parent))
                    .with(sought("issue", &self.parent))
                    .hint(fail::FIND_ISSUE));
            };
            self.input.insert("parentId".into(), json!(str_at(p, "id")));
        }
        match self.delegate.as_str() {
            "" => {}
            "none" => _ = self.input.insert("delegateId".into(), Value::Null),
            "me" => {
                if r["viewer"]["app"] != json!(true) {
                    return Err(failed(format!(
                        "--delegate me needs an app (agent) token; this key is {}'s, a person: delegate to an agent by name",
                        str_at(&r["viewer"], "name")
                    )));
                }
                self.input.insert("delegateId".into(), json!(str_at(&r["viewer"], "id")));
            }
            d => {
                let id = one("delegate", "agent user", d, &nodes(&r["delegates"]), "'linearctl user list --app'")?;
                self.input.insert("delegateId".into(), json!(id));
            }
        }
        match self.milestone.as_str() {
            "" => {}
            "none" => _ = self.input.insert("projectMilestoneId".into(), Value::Null),
            name => {
                let pid = match self.input.get("projectId").and_then(Value::as_str) {
                    Some(p) => p.to_string(),
                    None => project_id.to_string(),
                };
                let mut matched = vec![];
                let mut projects = vec![];
                for m in nodes(&r["milestones"]) {
                    let in_project = pid.is_empty() || (m["project"].is_object() && str_at(&m, "project.id") == pid);
                    if eq_fold(&str_at(&m, "name"), name) && in_project {
                        if m["project"].is_object() {
                            projects.push(str_at(&m, "project.name"));
                        }
                        matched.push(m);
                    }
                }
                match matched.len() {
                    0 => {
                        return Err(missing(format!(
                            "no milestone named {} in the issue's project; see 'linearctl milestone list'",
                            quote_go(name)
                        ))
                        .with(sought("milestone", name)));
                    }
                    1 => {}
                    n => {
                        return Err(ambiguous(format!(
                            "{n} milestones are named {} (projects: {}); pass --project",
                            quote_go(name),
                            projects.join(", ")
                        ))
                        .with(sought("milestone", name))
                        .with(json!({"candidates": candidates(&matched)})));
                    }
                }
                self.input.insert("projectMilestoneId".into(), json!(str_at(&matched[0], "id")));
                // A milestone belongs to a project: an issue without one joins it.
                if pid.is_empty() && matched[0]["project"].is_object() {
                    self.input.insert("projectId".into(), json!(str_at(&matched[0], "project.id")));
                }
            }
        }
        if !self.labels.is_empty() {
            let ids = label_ids(&self.labels, &nodes(&r["issueLabels"]), team_id)?;
            self.input.insert("labelIds".into(), json!(ids));
        }
        Ok(())
    }
}

/// A state or label not found, with the names the team can use instead
/// (its states; workspace labels and its own, never a group) as
/// available. Any other failure, or one this lookup cannot add to, is
/// returned as it is.
fn with_available(run: &mut Run, cl: &crate::client::Client, e: Fail, team_id: &str) -> Fail {
    let kind = e.detail().map(|d| str_at(d, "kind")).unwrap_or_default();
    if !matches!(e.base(), Fail::NotFound(_)) || !matches!(kind.as_str(), "state" | "label") || team_id.is_empty() {
        return e;
    }
    let vars = json!({"team": team_id, "states": kind == "state", "labels": kind == "label"});
    let Ok(r) = cl.query(run, queries::AVAILABLE, vars) else { return e };
    let found = if kind == "state" {
        nodes(&r["team"]["states"])
    } else {
        [nodes(&r["team"]["labels"]), nodes(&r["issueLabels"])].concat()
    };
    let mut names: Vec<String> = vec![];
    for n in &found {
        let name = str_at(n, "name");
        if !names.iter().any(|have| eq_fold(have, &name)) {
            names.push(name);
        }
    }
    e.with(json!({"available": names}))
}

/// A connection's nodes, none when it is absent.
/// Gives every issue row in data all its labels. A row reads its first 20
/// (first multiplies a list's cost) and labelIds, every label's id; the
/// labels past 20 are read by id, 250 to a request, and labelIds dropped.
pub fn complete_labels(run: &mut Run, cl: &crate::client::Client, data: &mut Value) -> R {
    let mut rows: Vec<&mut Value> = vec![];
    issue_rows(data, &mut rows);
    let mut want: Vec<String> = vec![];
    for r in &rows {
        for id in label_ids_missing(r) {
            if !want.contains(&id) {
                want.push(id);
            }
        }
    }
    let mut found: Vec<Value> = vec![];
    for ids in want.chunks(250) {
        let got = cl.query(run, queries::LABELS_BY_ID, json!({"ids": ids}))?;
        found.extend(nodes(&got["issueLabels"]));
    }
    for r in rows {
        for id in label_ids_missing(r) {
            if let (Some(l), Some(have)) =
                (found.iter().find(|l| str_at(l, "id") == id), r["labels"]["nodes"].as_array_mut())
            {
                have.push(l.clone());
            }
        }
        if let Some(m) = r.as_object_mut() {
            m.shift_remove("labelIds");
        }
    }
    Ok(())
}

/// The objects under v that carry labelIds: issue rows.
fn issue_rows<'a>(v: &'a mut Value, out: &mut Vec<&'a mut Value>) {
    match v {
        Value::Object(m) if m.contains_key("labelIds") => out.push(v),
        Value::Object(m) => m.values_mut().for_each(|x| issue_rows(x, out)),
        Value::Array(a) => a.iter_mut().for_each(|x| issue_rows(x, out)),
        _ => {}
    }
}

/// The ids in a row's labelIds that its labels lack.
fn label_ids_missing(r: &Value) -> Vec<String> {
    let have = nodes(&r["labels"]);
    let ids = r["labelIds"].as_array().cloned().unwrap_or_default();
    ids.iter()
        .filter_map(Value::as_str)
        .filter(|id| !have.iter().any(|l| str_at(l, "id") == *id))
        .map(String::from)
        .collect()
}

pub fn nodes(v: &Value) -> Vec<Value> {
    v["nodes"].as_array().cloned().unwrap_or_default()
}

/// The single match for name, or an error naming what was looked for:
/// kind as its JSON has it, word as its message does.
fn one(kind: &str, word: &str, name: &str, nodes: &[Value], hint: &str) -> R<String> {
    match nodes.len() {
        0 => Err(missing(format!("no {word} named {}; see {hint}", quote_go(name))).with(sought(kind, name))),
        1 => Ok(str_at(&nodes[0], "id")),
        n => Err(ambiguous(format!(
            "{n} {word}s are named {}; rename one in Linear, or pass its id in --json-input",
            quote_go(name)
        ))
        .with(sought(kind, name))
        .with(json!({"candidates": candidates(nodes)}))),
    }
}

/// Maps label names to ids, preferring the team's own label, then a
/// workspace label (no team). A group label cannot be put on an issue, so
/// it never matches; a name that only a group carries is not found, saying so.
fn label_ids(names: &[String], nodes: &[Value], team_id: &str) -> R<Vec<String>> {
    let mut ids: Vec<String> = vec![];
    for n in names {
        let (mut team, mut workspace, mut group) = (vec![], vec![], false);
        for l in nodes {
            if !eq_fold(&str_at(l, "name"), n) {
                continue;
            }
            if l["isGroup"].as_bool() == Some(true) {
                group = true;
            } else if !l["team"].is_object() {
                workspace.push(l.clone());
            } else if str_at(l, "team.id") == team_id {
                team.push(l.clone());
            }
        }
        let matched = if team.is_empty() { workspace } else { team };
        if matched.is_empty() && group {
            return Err(missing(format!(
                "label {} is a group, which Linear cannot put on an issue; pass one of its labels (see 'linearctl label list')",
                quote_go(n)
            ))
            .with(sought("label", n)));
        }
        let id = one("label", "label", n, &matched, "'linearctl label list'")?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

fn create_cmd() -> Cmd {
    let c = Cmd::new("create", "Create an issue")
        .long(
            "Create an issue in a team. Names (state, label, project, assignee email) are\n\
resolved to ids first; --dry-run prints the exact input it would send.\n\n\
--yes requires --id, a UUID you choose, so a rerun can never create a second\n\
issue: if one with that id exists, it is returned with changed: false.\n\
--dry-run without --id generates one and prints it as id; pass it to --yes.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl issue create --team ENG --title \"Fix login timeout\" --label bug --dry-run --json\n  linearctl issue create --team ENG --title \"Fix login timeout\" --label bug --yes --id 0b6c…\n  linearctl issue create --json-input issue.json --id 0b6c… --yes",
        )
        .run(create)
        .flag(Flag::str("team", "create in team `KEY` (e.g. ENG)"))
        .flag(Flag::str("id", "the new issue's `UUID`; required with --yes"));
    issue_fields(c).flag(Flag::strs("label", "add label `NAME` (repeatable)")).mutating()
}

/// A client-chosen id for a new issue or comment: --id, or one made up for
/// a dry run.
pub fn chosen_id(ctx: &Ctx, dry: bool, needs: &str) -> R<String> {
    let id = ctx.str("id");
    if id.is_empty() && !dry {
        return Err(usage(format!("--yes needs --id UUID so a rerun cannot {needs}; run --dry-run for one")));
    }
    if id.is_empty() {
        return Ok(new_id());
    }
    if !is_uuid(&id) {
        return Err(usage(format!("--id must be a UUID; got {}", quote_go(&id))));
    }
    Ok(id.to_lowercase())
}

fn create(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let id = chosen_id(ctx, dry, "create a second issue")?;
    let mut w = read_wanted(ctx)?;
    // The team: --team (or its fallbacks), else Linear's own teamId in
    // --json-input; flags win, as they do over every input field.
    let team = team_flag(ctx)?;
    let input_team = w.input.get("teamId").and_then(Value::as_str).unwrap_or("").to_string();
    let team_filter = if !team.is_empty() {
        json!({"key": {"eqIgnoreCase": team}})
    } else if is_uuid(&input_team) {
        json!({"id": {"eq": input_team.to_lowercase()}})
    } else if !input_team.is_empty() {
        return Err(usage(format!("teamId in --json-input must be a team's UUID; got {}", quote_go(&input_team))));
    } else {
        return Err(usage(
            "pass --team KEY, or teamId in --json-input (or set LINEARCTL_TEAM, or default-team with 'linearctl config set')",
        ));
    };
    let team = if team.is_empty() { input_team } else { team };
    if w.input.get("title").and_then(Value::as_str).unwrap_or("").is_empty() {
        return Err(usage("pass --title (or title in --json-input)"));
    }
    let cl = run.client()?;
    let mut vars = w.resolve_vars(&[]);
    vars.insert("team".into(), team_filter);
    vars.insert("id".into(), json!(id));
    vars.insert("withID".into(), json!(true));
    let mut r = cl.query(run, queries::CREATE_RESOLVE, Value::Object(vars))?;
    complete_labels(run, &cl, &mut r["existing"])?;
    if let Some(existing) = nodes(&r["existing"]).first() {
        let it = flatten(existing.clone());
        let text = format!("exists {} {}", str_at(&it, "identifier"), str_at(&it, "url"));
        return run.output(json!({"changed": false, "dryRun": dry, "id": id, "issue": it}), &text);
    }
    let teams = nodes(&r["teams"]);
    let Some(t) = teams.first() else {
        let by = if is_uuid(&team) { "id" } else { "key" };
        return Err(missing(format!("no team with {by} {team}; see 'linearctl team list'")).with(sought("team", &team)));
    };
    w.apply(&r, &nodes(&t["states"]), &nodes(&t["typed"]), &nodes(&t["cycles"]), &str_at(t, "id"), "")
        .map_err(|e| with_available(run, &cl, e, &str_at(t, "id")))?;
    w.input.insert("teamId".into(), json!(str_at(t, "id")));
    w.input.insert("id".into(), json!(id));
    if dry {
        let title = printable(w.input["title"].as_str().unwrap_or(""));
        let text = format!("would create in {}: {title}\nid: {id} (pass --id {id} --yes)", str_at(t, "key"));
        return run.output(json!({"changed": true, "dryRun": true, "id": id, "input": w.input}), &text);
    }
    let mut out = cl.query(run, queries::ISSUE_CREATE, json!({"input": w.input}))?;
    complete_labels(run, &cl, &mut out)?;
    let it = flatten(out["issueCreate"]["issue"].take());
    let text = format!("created {} {}", str_at(&it, "identifier"), str_at(&it, "url"));
    run.output(json!({"changed": true, "dryRun": false, "id": id, "issue": it}), &text)
}

fn update_cmd() -> Cmd {
    let c = Cmd::new("update ISSUE", "Change an issue's fields")
        .long(
            "Change an issue's fields. Only fields that differ from the issue as it is are\n\
sent; when none differ, nothing is sent and the result is changed: false.\n\
--label replaces all labels; --add-label and --remove-label adjust them.\n\
State names resolve within the issue's own team.\n\
--if-updated-at TIME refuses (exit 7, conflict) if the issue changed since you\nread it.\n\
Changes things: pass --dry-run to preview, --yes to apply.",
        )
        .example(
            "  linearctl issue update ENG-123 --state \"In Progress\" --assignee me --dry-run --json\n  linearctl issue update ENG-123 --add-label blocked --priority high --yes",
        )
        .args(Args::Exact(1))
        .run(update);
    issue_fields(c)
        .flag(Flag::strs("label", "set the labels to `NAME`s, replacing the rest (repeatable)"))
        .flag(Flag::strs("add-label", "add label `NAME` (repeatable)"))
        .flag(Flag::str("if-updated-at", "change it only if its updatedAt is still `TIME` (else exit 7)"))
        .flag(Flag::strs("remove-label", "remove label `NAME` (repeatable)"))
        .mutating()
}

fn update(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let r = issue_ref("ISSUE", &ctx.args[0])?;
    let mut w = read_wanted(ctx)?;
    let add = values(ctx, "add-label")?;
    let remove = values(ctx, "remove-label")?;
    if ctx.changed("label") && (!add.is_empty() || !remove.is_empty()) {
        return Err(usage("pass --label, or --add-label/--remove-label, not both"));
    }
    let since = ctx.str("if-updated-at");
    if !since.is_empty() && time::parse_rfc3339(&since).is_none() {
        return Err(usage(format!(
            "--if-updated-at must be the issue's updatedAt, an RFC 3339 time; got {}",
            quote_go(&since)
        )));
    }
    let cl = run.client()?;
    let mut extra = add.clone();
    extra.extend(remove.iter().cloned());
    let mut vars = w.resolve_vars(&extra);
    vars.insert("id".into(), json!(r));
    let res = cl
        .query(run, queries::UPDATE_RESOLVE, Value::Object(vars))
        .map_err(|e| e.not_found(&format!("issue {r}"), fail::FIND_ISSUE))?;
    let is = &res["issue"];
    let identifier = str_at(is, "identifier");
    let updated_at = str_at(is, "updatedAt");
    if !since.is_empty() && !updated_at.is_empty() {
        let then = time::parse_rfc3339(&since);
        if let Some(now) = time::parse_rfc3339(&updated_at)
            && Some(now) != then
        {
            return Err(Fail::Conflict(format!(
                "{identifier} changed at {updated_at}, not {since} as --if-updated-at expects; read it again"
            ))
            .hint(format!("'linearctl issue show {identifier}' reads it; decide again from what it shows")));
        }
    }
    let team_id = str_at(is, "team.id");
    let current_project = if is["project"].is_object() { str_at(is, "project.id") } else { String::new() };
    w.apply(
        &res,
        &nodes(&is["team"]["states"]),
        &nodes(&is["team"]["typed"]),
        &nodes(&is["team"]["cycles"]),
        &team_id,
        &current_project,
    )
    .map_err(|e| with_available(run, &cl, e, &team_id))?;
    let current: Vec<String> = nodes(&is["labels"]).iter().map(|l| str_at(l, "id")).collect();
    if !add.is_empty() || !remove.is_empty() {
        let labels = nodes(&res["issueLabels"]);
        let ids = |names: &[String]| label_ids(names, &labels, &team_id);
        let add_ids = ids(&add).map_err(|e| with_available(run, &cl, e, &team_id))?;
        let remove_ids = ids(&remove).map_err(|e| with_available(run, &cl, e, &team_id))?;
        let added: Vec<&String> = add_ids.iter().filter(|id| !current.contains(id)).collect();
        let removed: Vec<&String> = remove_ids.iter().filter(|id| current.contains(id)).collect();
        if !added.is_empty() {
            w.input.insert("addedLabelIds".into(), json!(added));
        }
        if !removed.is_empty() {
            w.input.insert("removedLabelIds".into(), json!(removed));
        }
    }
    // Drop what already holds, so a rerun sends nothing.
    let id = |v: &Value| if v.is_object() { json!(str_at(v, "id")) } else { Value::Null };
    let description = match &is["description"] {
        Value::String(s) => json!(s),
        _ => Value::Null,
    };
    let now = [
        ("title", json!(str_at(is, "title"))),
        ("description", description),
        ("priority", json!(is["priority"].as_f64().unwrap_or(0.0) as i64)),
        ("stateId", id(&is["state"])),
        ("assigneeId", id(&is["assignee"])),
        ("projectId", id(&is["project"])),
        ("parentId", id(&is["parent"])),
        ("projectMilestoneId", id(&is["projectMilestone"])),
        ("delegateId", id(&is["delegate"])),
        ("cycleId", id(&is["cycle"])),
        ("dueDate", is["dueDate"].clone()),
        // Linear answers estimate as a float; the input is an Int.
        ("estimate", is["estimate"].as_f64().map_or(Value::Null, |e| json!(e as i64))),
    ];
    for (k, v) in now {
        if w.input.get(k).is_some_and(|got| marshal(got) == marshal(&v)) {
            w.input.shift_remove(k);
        }
    }
    if !w.labels.is_empty()
        && let Some(ids) = w.input.get("labelIds").and_then(Value::as_array)
    {
        let mut a: Vec<String> = ids.iter().filter_map(|x| x.as_str().map(String::from)).collect();
        let mut b = current.clone();
        a.sort();
        b.sort();
        if a == b {
            w.input.shift_remove("labelIds");
        }
    }
    let changed = !w.input.is_empty();
    let mut doc = json!({"changed": changed, "dryRun": dry, "identifier": identifier, "input": w.input});
    if !changed {
        return run.output(doc, &format!("unchanged {identifier}"));
    }
    if dry {
        let mut keys: Vec<&String> = w.input.keys().collect();
        keys.sort();
        let keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
        return run.output(doc, &format!("would update {identifier}: {}", keys.join(", ")));
    }
    let mut out = cl.query(run, queries::ISSUE_UPDATE, json!({"id": str_at(is, "id"), "input": w.input}))?;
    complete_labels(run, &cl, &mut out)?;
    doc["issue"] = flatten(out["issueUpdate"]["issue"].take());
    run.output(doc, &format!("updated {identifier}"))
}

fn archive_cmd(archive: bool) -> Cmd {
    let (verb, done) = if archive { ("archive", "archived") } else { ("unarchive", "unarchived") };
    let short = if archive { "Archive an issue" } else { "Restore an archived issue" };
    let mut cap = verb.to_string();
    cap[..1].make_ascii_uppercase();
    let mut c = Cmd::new(&format!("{verb} ISSUE"), short)
        .long(&format!(
            "{cap} an issue. An issue already {done} is changed: false.\nChanges things: pass --dry-run to preview, --yes to apply."
        ))
        .example(&format!("  linearctl issue {verb} ENG-123 --dry-run\n  linearctl issue {verb} ENG-123 --yes --json"))
        .args(Args::Exact(1))
        .mutating();
    c.run =
        Some(if archive { |run, ctx| archive_run(run, ctx, true) } else { |run, ctx| archive_run(run, ctx, false) });
    c
}

fn archive_run(run: &mut Run, ctx: &Ctx, archive: bool) -> R {
    let (verb, done) = if archive { ("archive", "archived") } else { ("unarchive", "unarchived") };
    let dry = dry_run(ctx)?;
    let r = issue_ref("ISSUE", &ctx.args[0])?;
    let cl = run.client()?;
    let res = cl
        .query(run, queries::ISSUE_ARCHIVED, json!({"id": r}))
        .map_err(|e| e.not_found(&format!("issue {r}"), fail::FIND_ISSUE))?;
    let is = &res["issue"];
    let changed = is["archivedAt"].is_null() == archive;
    if changed && !dry {
        let doc = if archive { queries::ISSUE_ARCHIVE } else { queries::ISSUE_UNARCHIVE };
        cl.query(run, doc, json!({"id": str_at(is, "id")}))?;
    }
    let shown = match (changed, dry) {
        (true, true) => format!("would {verb}"),
        (true, false) => done.to_string(),
        (false, _) => format!("already {done}"),
    };
    let identifier = str_at(is, "identifier");
    run.output(
        json!({"changed": changed, "dryRun": dry, "identifier": identifier, "url": str_at(is, "url")}),
        &format!("{shown} {identifier}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The resolve queries declare non-null variables that resolve_vars
    /// must supply; a missing one is an error only Linear reports.
    #[test]
    fn resolve_vars_cover_every_variable() {
        for (doc, extra) in
            [(queries::CREATE_RESOLVE, vec!["team", "id", "withID"]), (queries::UPDATE_RESOLVE, vec!["id"])]
        {
            let header = &doc[doc.find('(').unwrap() + 1..doc.find(") {").unwrap()];
            let mut declared: Vec<String> = header
                .split(',')
                .map(|v| v.trim().trim_start_matches('$').split(':').next().unwrap().to_string())
                .collect();
            let mut supplied: Vec<String> = Wanted::default().resolve_vars(&[]).keys().cloned().collect();
            supplied.extend(extra.iter().map(|s| s.to_string()));
            declared.sort();
            supplied.sort();
            assert_eq!(declared, supplied);
        }
    }

    #[test]
    fn priorities() {
        assert_eq!(priority("--p", "High").unwrap(), 2);
        assert_eq!(priority("--p", "+3").unwrap(), 3);
        assert!(priority("--p", "5").is_err() && priority("--p", "0x1").is_err());
    }
}
