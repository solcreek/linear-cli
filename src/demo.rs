//! Demo mode (--demo or LINEARCTL_DEMO=1) answers every command from a
//! fictional workspace built into the binary, with no API key and no
//! network. It swaps the transport, not the commands: each command sends
//! its usual GraphQL document, and answer resolves it against the
//! workspace, so demo output has the shapes of real output by construction.
//! Writes are refused; --dry-run works, since it only reads.
//!
//! demo/workspace.json, at the repository root, is written for people:
//! issues name their state, labels and users, and times are relative to
//! "anchor", which is moved to now. linear-tui drafts it; linearctl ships it.

use crate::client::Answer;
use crate::gql::{self, Directive, Field, Selection};
use crate::json::{self, sprint_float};
use crate::time;
use serde_json::{Map, Value, json};
use std::cell::OnceCell;
use std::collections::HashMap;

const SEED: &str = include_str!("../demo/workspace.json");
const ORG: &str = "northwind";

/// A value in the workspace: a scalar, a node (an object others can point
/// at; the graph has cycles, an issue's comments point back at it) or a
/// list.
#[derive(Clone)]
enum V {
    S(Value),
    N(usize),
    L(Vec<V>),
}

type Node = HashMap<String, V>;

struct World {
    nodes: Vec<Node>,
    root: HashMap<String, Vec<V>>,
    viewer: usize,
    org: usize,
}

thread_local! {
    static WORLD: OnceCell<World> = const { OnceCell::new() };
}

/// A stable UUID for a name.
fn demo_id(kind: &str, name: &str) -> String {
    let mut h = sha1::Sha1::default();
    sha1::Digest::update(&mut h, format!("{kind}:{name}").as_bytes());
    let mut b: [u8; 20] = sha1::Digest::finalize(h).into();
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b[..16].iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

fn slug(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in lower.bytes() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c as char);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let mut s = out.trim_matches('-').to_string();
    if s.len() > 60 {
        s.truncate(60);
        s = s.trim_end_matches('-').to_string();
    }
    s
}

/// fmt.Sprint of a JSON value, as Go printed it.
fn sprint(v: &Value) -> String {
    match v {
        Value::Null => "<nil>".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => sprint_float(n.as_f64().unwrap_or(0.0)),
        Value::Array(a) => format!("[{}]", a.iter().map(sprint).collect::<Vec<_>>().join(" ")),
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            format!("map[{}]", keys.iter().map(|k| format!("{k}:{}", sprint(&m[*k]))).collect::<Vec<_>>().join(" "))
        }
    }
}

/// A float64 as Go would have held it, written as Go writes it.
fn num(f: f64) -> Value {
    serde_json::from_str(&json::float(f)).unwrap_or(Value::Null)
}

fn s(v: &str) -> V {
    V::S(Value::String(v.into()))
}

const NULL: V = V::S(Value::Null);

fn build() -> World {
    let seed: Value = serde_json::from_str(SEED).expect("demo/workspace.json");
    let anchor = time::parse_rfc3339(seed["anchor"].as_str().unwrap_or("")).unwrap_or(0);
    let now = time::now();
    let shift = now - anchor;
    let at = |v: &Value| -> V {
        let s = v.as_str().unwrap_or("");
        if let Some(t) = time::parse_rfc3339(s) {
            return V::S(json!(time::rfc3339_ms(t + shift)));
        }
        if let Some(t) = time::parse_date(s) {
            return V::S(json!(time::date(t + shift)));
        }
        NULL
    };
    let mut w = World { nodes: vec![], root: HashMap::new(), viewer: 0, org: 0 };
    let add = |w: &mut World, n: Node| {
        w.nodes.push(n);
        w.nodes.len() - 1
    };
    let url_key = seed["organization"]["urlKey"].as_str().unwrap_or("").to_string();
    w.org = add(
        &mut w,
        HashMap::from([
            ("id".into(), s(&demo_id("org", ORG))),
            ("name".into(), V::S(seed["organization"]["name"].clone())),
            ("urlKey".into(), V::S(seed["organization"]["urlKey"].clone())),
        ]),
    );
    let viewer = seed["viewer"].as_str().unwrap_or("").to_string();
    let mut users: HashMap<String, usize> = HashMap::new();
    for u in seed["users"].as_array().into_iter().flatten() {
        let email = u["email"].as_str().unwrap_or("").to_string();
        let id = add(
            &mut w,
            HashMap::from([
                ("__typename".into(), s("User")),
                ("id".into(), s(&demo_id("user", &email))),
                ("name".into(), V::S(u["name"].clone())),
                ("displayName".into(), V::S(u["displayName"].clone())),
                ("email".into(), s(&email)),
                ("active".into(), V::S(json!(true))),
                ("admin".into(), V::S(json!(u["admin"] == json!(true)))),
                ("guest".into(), V::S(json!(false))),
                ("app".into(), V::S(json!(false))),
                ("isMe".into(), V::S(json!(email == viewer))),
            ]),
        );
        users.insert(email, id);
        w.root.entry("users".into()).or_default().push(V::N(id));
    }
    w.viewer = users.get(&viewer).copied().unwrap_or(0);
    let mut team = usize::MAX;
    for t in seed["teams"].as_array().into_iter().flatten() {
        team = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&demo_id("team", t["key"].as_str().unwrap_or("")))),
                ("key".into(), V::S(t["key"].clone())),
                ("name".into(), V::S(t["name"].clone())),
                ("description".into(), V::S(t["description"].clone())),
                ("private".into(), V::S(json!(t["private"] == json!(true)))),
            ]),
        );
        w.root.entry("teams".into()).or_default().push(V::N(team));
    }
    let team_v = if team == usize::MAX { NULL } else { V::N(team) };
    let mut states: HashMap<String, usize> = HashMap::new();
    for (i, st) in seed["states"].as_array().into_iter().flatten().enumerate() {
        let name = st["name"].as_str().unwrap_or("").to_string();
        let id = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&demo_id("state", &name))),
                ("name".into(), s(&name)),
                ("type".into(), V::S(st["type"].clone())),
                ("color".into(), V::S(st["color"].clone())),
                ("position".into(), V::S(num(i as f64))),
                ("team".into(), team_v.clone()),
            ]),
        );
        states.insert(name, id);
        w.root.entry("workflowStates".into()).or_default().push(V::N(id));
    }
    let mut labels: HashMap<String, usize> = HashMap::new();
    for l in seed["labels"].as_array().into_iter().flatten() {
        let name = l["name"].as_str().unwrap_or("").to_string();
        let id = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&demo_id("label", &name))),
                ("name".into(), s(&name)),
                ("color".into(), V::S(l["color"].clone())),
                ("isGroup".into(), V::S(json!(false))),
                ("team".into(), NULL),
                ("parent".into(), NULL),
            ]),
        );
        labels.insert(name, id);
        w.root.entry("issueLabels".into()).or_default().push(V::N(id));
    }
    let mut cycles: HashMap<String, usize> = HashMap::new();
    let mut next: Option<(usize, String)> = None;
    let mut previous: Option<(usize, String)> = None;
    for c in seed["cycles"].as_array().into_iter().flatten() {
        let (starts, ends) = (at(&c["startsAt"]), at(&c["endsAt"]));
        let text = |v: &V| if let V::S(Value::String(x)) = v { x.clone() } else { String::new() };
        let (st, en) = (text(&starts), text(&ends));
        let (sn, enn) = (time::parse_rfc3339(&st).unwrap_or(0), time::parse_rfc3339(&en).unwrap_or(0));
        let number = sprint(&c["number"]);
        let id = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&demo_id("cycle", &number))),
                ("number".into(), V::S(c["number"].clone())),
                ("name".into(), NULL),
                ("endsAt".into(), ends.clone()),
                ("description".into(), NULL),
                ("startsAt".into(), starts.clone()),
                ("completedAt".into(), if now >= enn { ends.clone() } else { NULL }),
                ("createdAt".into(), starts.clone()),
                ("updatedAt".into(), starts),
                ("archivedAt".into(), NULL),
                ("autoArchivedAt".into(), NULL),
                ("isActive".into(), V::S(json!(now >= sn && now < enn))),
                ("isNext".into(), V::S(json!(false))),
                ("isPrevious".into(), V::S(json!(false))),
                ("isPast".into(), V::S(json!(now >= enn))),
                ("isFuture".into(), V::S(json!(now < sn))),
                // Linear's day-by-day series; the demo has no history.
                ("progress".into(), V::S(json!(0.0))),
                ("currentProgress".into(), V::S(json!({}))),
                ("progressHistory".into(), V::S(json!({}))),
                ("scopeHistory".into(), V::S(json!([]))),
                ("completedScopeHistory".into(), V::S(json!([]))),
                ("inProgressScopeHistory".into(), V::S(json!([]))),
                ("issueCountHistory".into(), V::S(json!([]))),
                ("completedIssueCountHistory".into(), V::S(json!([]))),
                ("inheritedFrom".into(), NULL),
                ("issues".into(), V::L(vec![])),
                ("uncompletedIssuesUponClose".into(), V::L(vec![])),
                ("team".into(), team_v.clone()),
            ]),
        );
        if now < sn && next.as_ref().is_none_or(|(_, ns)| st < *ns) {
            next = Some((id, st.clone()));
        }
        if now >= enn && previous.as_ref().is_none_or(|(_, pe)| en > *pe) {
            previous = Some((id, en.clone()));
        }
        cycles.insert(number, id);
        w.root.entry("cycles".into()).or_default().push(V::N(id));
    }
    if let Some((id, _)) = next {
        w.nodes[id].insert("isNext".into(), V::S(json!(true)));
    }
    if let Some((id, _)) = previous {
        w.nodes[id].insert("isPrevious".into(), V::S(json!(true)));
    }
    if team != usize::MAX {
        let states_list = w.root.get("workflowStates").cloned().unwrap_or_default();
        let cycles_list = w.root.get("cycles").cloned().unwrap_or_default();
        w.nodes[team].insert("states".into(), V::L(states_list));
        // The demo's labels belong to the workspace.
        w.nodes[team].insert("labels".into(), V::L(vec![]));
        w.nodes[team].insert("cycles".into(), V::L(cycles_list));
    }
    let status_names = [
        ("backlog", "Backlog"),
        ("planned", "Planned"),
        ("started", "In Progress"),
        ("paused", "Paused"),
        ("completed", "Completed"),
        ("canceled", "Canceled"),
    ];
    let mut projects: HashMap<String, usize> = HashMap::new();
    for p in seed["projects"].as_array().into_iter().flatten() {
        let name = p["name"].as_str().unwrap_or("").to_string();
        let full = demo_id("project", &name);
        let slug_id = full[24..].to_string();
        let kind = p["status"].as_str().unwrap_or("").to_string();
        let status_name = status_names.iter().find(|(k, _)| *k == kind).map_or("", |(_, n)| n);
        let status = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&demo_id("status", &kind))),
                ("name".into(), s(status_name)),
                ("type".into(), s(&kind)),
            ]),
        );
        let lead = users.get(&sprint(&p["lead"])).map_or(NULL, |u| V::N(*u));
        let id = add(
            &mut w,
            HashMap::from([
                ("id".into(), s(&full)),
                ("name".into(), s(&name)),
                ("slugId".into(), s(&slug_id)),
                ("url".into(), s(&format!("https://linear.app/{url_key}/project/{}-{slug_id}", slug(&name)))),
                ("description".into(), s("")),
                ("content".into(), s("")),
                ("startDate".into(), NULL),
                ("targetDate".into(), at(&p["targetDate"])),
                ("progress".into(), V::S(num(0.0))),
                ("status".into(), V::N(status)),
                ("lead".into(), lead),
                ("teams".into(), V::L(vec![team_v.clone()])),
                ("projectMilestones".into(), V::L(vec![])),
                ("issues".into(), V::L(vec![])),
            ]),
        );
        projects.insert(name, id);
        w.root.entry("projects".into()).or_default().push(V::N(id));
    }
    let priority_labels = ["No priority", "Urgent", "High", "Medium", "Low"];
    let mut issues: HashMap<String, usize> = HashMap::new();
    let seeds = seed["issues"].as_array().cloned().unwrap_or_default();
    for sd in &seeds {
        let ident = sd["identifier"].as_str().unwrap_or("").to_string();
        let number: i64 = ident.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let prio = sd["priority"].as_f64().unwrap_or(0.0) as usize;
        let title = sd["title"].as_str().unwrap_or("").to_string();
        let url = format!("https://linear.app/{url_key}/issue/{ident}/{}", slug(&title));
        let who = sprint(&sd["assignee"]);
        let lookup = |m: &HashMap<String, usize>, key: &Value| {
            if key.is_null() { NULL } else { m.get(&sprint(key)).map_or(NULL, |i| V::N(*i)) }
        };
        let mut m: Node = HashMap::from([
            ("__typename".into(), s("Issue")),
            ("id".into(), s(&demo_id("issue", &ident))),
            ("identifier".into(), s(&ident)),
            ("number".into(), V::S(num(number as f64))),
            ("title".into(), V::S(sd["title"].clone())),
            ("description".into(), V::S(sd["description"].clone())),
            ("priority".into(), V::S(num(prio as f64))),
            ("priorityLabel".into(), s(priority_labels.get(prio).copied().unwrap_or(""))),
            ("url".into(), s(&url)),
            (
                "branchName".into(),
                s(&format!("{}/{}-{}", who.split('@').next().unwrap_or(""), ident.to_lowercase(), slug(&title))),
            ),
            ("createdAt".into(), at(&sd["createdAt"])),
            ("updatedAt".into(), at(&sd["updatedAt"])),
            ("startedAt".into(), at(&sd["startedAt"])),
            ("completedAt".into(), at(&sd["completedAt"])),
            ("canceledAt".into(), NULL),
            ("archivedAt".into(), NULL),
            ("trashed".into(), V::S(json!(false))),
            ("dueDate".into(), at(&sd["dueDate"])),
            ("estimate".into(), V::S(sd["estimate"].clone())),
            ("state".into(), states.get(sd["state"].as_str().unwrap_or("")).map_or(NULL, |i| V::N(*i))),
            ("team".into(), team_v.clone()),
            ("assignee".into(), lookup(&users, &sd["assignee"])),
            ("creator".into(), lookup(&users, &sd["creator"])),
            ("delegate".into(), NULL),
            ("project".into(), lookup(&projects, &sd["project"])),
            ("projectMilestone".into(), NULL),
            ("parent".into(), NULL),
            ("children".into(), V::L(vec![])),
            ("relations".into(), V::L(vec![])),
            ("inverseRelations".into(), V::L(vec![])),
            ("comments".into(), V::L(vec![])),
            ("attachments".into(), V::L(vec![])),
        ]);
        m.insert(
            "cycle".into(),
            if sd["cycle"].is_number() { cycles.get(&sprint(&sd["cycle"])).map_or(NULL, |i| V::N(*i)) } else { NULL },
        );
        let ls: Vec<V> = sd["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| labels.get(l.as_str().unwrap_or("")).map_or(NULL, |i| V::N(*i)))
            .collect();
        m.insert("labels".into(), V::L(ls));
        let id = add(&mut w, m);
        for a in sd["attachments"].as_array().into_iter().flatten() {
            let att = add(
                &mut w,
                HashMap::from([
                    ("id".into(), s(&demo_id("attachment", &sprint(&a["url"])))),
                    ("title".into(), V::S(a["title"].clone())),
                    ("url".into(), V::S(a["url"].clone())),
                    ("issue".into(), V::N(id)),
                ]),
            );
            push(&mut w.nodes[id], "attachments", V::N(att));
            w.root.entry("attachments".into()).or_default().push(V::N(att));
        }
        for c in sd["comments"].as_array().into_iter().flatten() {
            let cid = demo_id("comment", c["id"].as_str().unwrap_or(""));
            let parent = match c["parent"].as_str() {
                Some(p) => {
                    let pid = add(&mut w, HashMap::from([("id".into(), s(&demo_id("comment", p)))]));
                    V::N(pid)
                }
                None => NULL,
            };
            let cm = add(
                &mut w,
                HashMap::from([
                    ("id".into(), s(&cid)),
                    ("body".into(), V::S(c["body"].clone())),
                    ("createdAt".into(), at(&c["createdAt"])),
                    ("url".into(), s(&format!("{url}#comment-{}", &cid[..8]))),
                    ("user".into(), users.get(&sprint(&c["user"])).map_or(NULL, |u| V::N(*u))),
                    ("issue".into(), V::N(id)),
                    ("parent".into(), parent),
                ]),
            );
            push(&mut w.nodes[id], "comments", V::N(cm));
            w.root.entry("comments".into()).or_default().push(V::N(cm));
        }
        issues.insert(ident, id);
        w.root.entry("issues".into()).or_default().push(V::N(id));
        if let Some(V::N(p)) = w.nodes[id].get("project").cloned() {
            push(&mut w.nodes[p], "issues", V::N(id));
        }
    }
    for sd in &seeds {
        let id = issues[sd["identifier"].as_str().unwrap_or("")];
        if let Some(&p) = sd["parent"].as_str().and_then(|p| issues.get(p)) {
            w.nodes[id].insert("parent".into(), V::N(p));
            push(&mut w.nodes[p], "children", V::N(id));
        }
        for r in sd["relations"].as_array().into_iter().flatten() {
            let Some(&other) = issues.get(&sprint(&r["issue"])) else { continue };
            let ident = sd["identifier"].as_str().unwrap_or("");
            let other_ident = text_of(&w.nodes[other], "identifier");
            let rel = add(
                &mut w,
                HashMap::from([
                    ("id".into(), s(&demo_id("relation", &format!("{ident}{}{other_ident}", sprint(&r["type"]))))),
                    ("type".into(), V::S(r["type"].clone())),
                    ("issue".into(), V::N(id)),
                    ("relatedIssue".into(), V::N(other)),
                ]),
            );
            push(&mut w.nodes[id], "relations", V::N(rel));
            push(&mut w.nodes[other], "inverseRelations", V::N(rel));
        }
    }
    cycle_issues(&mut w);
    for k in ["projectMilestones", "notifications", "attachments", "comments", "cycles", "projects", "customViews"] {
        w.root.entry(k.into()).or_default();
    }
    w
}

/// Each cycle's issues, and its progress as Linear documents it: completed
/// points plus a quarter of in-progress points, over all points; 0 without
/// points. The demo stands in for Linear here; linearctl itself computes none.
fn cycle_issues(w: &mut World) {
    let issues: Vec<usize> = w
        .root
        .get("issues")
        .into_iter()
        .flatten()
        .filter_map(|v| if let V::N(i) = v { Some(*i) } else { None })
        .collect();
    let cycles: Vec<usize> = w
        .root
        .get("cycles")
        .into_iter()
        .flatten()
        .filter_map(|v| if let V::N(i) = v { Some(*i) } else { None })
        .collect();
    for c in cycles {
        let (mut total, mut done, mut started) = (0.0, 0.0, 0.0);
        for &i in &issues {
            if !matches!(w.nodes[i].get("cycle"), Some(V::N(x)) if *x == c) {
                continue;
            }
            push(&mut w.nodes[c], "issues", V::N(i));
            let points = match w.nodes[i].get("estimate") {
                Some(V::S(v)) => v.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };
            let kind = match w.nodes[i].get("state") {
                Some(V::N(st)) => text_of(&w.nodes[*st], "type"),
                _ => String::new(),
            };
            total += points;
            match kind.as_str() {
                "completed" => done += points,
                "started" => started += points,
                _ => {}
            }
        }
        let progress = if total > 0.0 { (done + 0.25 * started) / total } else { 0.0 };
        w.nodes[c].insert("progress".into(), V::S(json!(progress)));
    }
}

fn push(n: &mut Node, k: &str, v: V) {
    if let Some(V::L(l)) = n.get_mut(k) {
        l.push(v);
    }
}

fn text_of(n: &Node, k: &str) -> String {
    match n.get(k) {
        Some(V::S(v)) => sprint(v),
        _ => String::new(),
    }
}

/// Resolves a GraphQL request against the demo workspace, as Linear would
/// answer it.
pub fn answer(query: &str, variables: &Value) -> Answer {
    let fail = |msg: String| Answer {
        data: Value::Null,
        errors: vec![json!({"message": msg, "extensions": {"code": "DEMO"}})],
    };
    let doc = match gql::parse(query) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let Some(op) = doc.operations.first().filter(|o| o.kind == "query") else {
        return fail("demo mode is read-only: a fictional workspace, nothing is sent or changed".into());
    };
    WORLD.with(|cell| {
        let w = cell.get_or_init(build);
        let r = Resolver { w, vars: variables };
        let mut data = Map::new();
        for sel in &op.selections {
            let Selection::Field(f) = sel else { continue };
            if !r.included(&f.directives) {
                continue;
            }
            match r.root_field(f) {
                Ok(v) => _ = data.insert(f.key().to_string(), v),
                Err(e) => return Answer { data: Value::Null, errors: vec![json!({"message": e})] },
            }
        }
        Answer { data: sorted(Value::Object(data)), errors: vec![] }
    })
}

/// Objects with their keys sorted, as Go's encoder wrote the answer.
fn sorted(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut entries: Vec<(String, Value)> = m.into_iter().map(|(k, x)| (k, sorted(x))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(sorted).collect()),
        v => v,
    }
}

struct Resolver<'a> {
    w: &'a World,
    vars: &'a Value,
}

impl Resolver<'_> {
    fn args(&self, f: &Field) -> Map<String, Value> {
        f.args.iter().map(|(k, v)| (k.clone(), v.value(self.vars))).collect()
    }

    fn included(&self, ds: &[Directive]) -> bool {
        for d in ds {
            if let Some((_, lit)) = d.args.iter().find(|(n, _)| n == "if") {
                let v = lit.value(self.vars);
                if (d.name == "include" && v != json!(true)) || (d.name == "skip" && v == json!(true)) {
                    return false;
                }
            }
        }
        true
    }

    fn root_field(&self, f: &Field) -> Result<Value, String> {
        let args = self.args(f);
        let find = |list: &str, kind: &str, keys: &[&str]| -> Result<Value, String> {
            let id = sprint(args.get("id").unwrap_or(&Value::Null)).to_lowercase();
            for n in self.w.root.get(list).into_iter().flatten() {
                let V::N(i) = n else { continue };
                for k in keys {
                    if text_of(&self.w.nodes[*i], k).to_lowercase() == id && self.w.nodes[*i].contains_key(*k) {
                        return Ok(self.value(n, f));
                    }
                }
            }
            Err(format!("Entity not found: {kind}"))
        };
        match f.name.as_str() {
            "viewer" => return Ok(self.value(&V::N(self.w.viewer), f)),
            "organization" => return Ok(self.value(&V::N(self.w.org), f)),
            "issue" => return find("issues", "Issue", &["id", "identifier"]),
            "project" => return find("projects", "Project", &["id", "slugId"]),
            "comment" => return find("comments", "Comment", &["id"]),
            "team" => return find("teams", "Team", &["id", "key"]),
            "searchIssues" => {
                let term = sprint(args.get("term").unwrap_or(&Value::Null)).to_lowercase();
                let mut hits = vec![];
                for n in self.w.root.get("issues").into_iter().flatten() {
                    let V::N(i) = n else { continue };
                    let node = &self.w.nodes[*i];
                    let mut text = format!("{} {}", text_of(node, "title"), text_of(node, "description"));
                    if args.get("includeComments") == Some(&json!(true))
                        && let Some(V::L(cs)) = node.get("comments")
                    {
                        for c in cs {
                            if let V::N(ci) = c {
                                text.push(' ');
                                text.push_str(&text_of(&self.w.nodes[*ci], "body"));
                            }
                        }
                    }
                    if text.to_lowercase().contains(&term) {
                        hits.push(n.clone());
                    }
                }
                return Ok(self.value(&V::L(hits), f));
            }
            _ => {}
        }
        match self.w.root.get(&f.name) {
            Some(list) => Ok(self.value(&V::L(list.clone()), f)),
            None => Err(format!("demo mode does not answer {}", f.name)),
        }
    }

    /// Resolves v for field f: a connection, an object, or a scalar.
    fn value(&self, v: &V, f: &Field) -> Value {
        if f.selections.is_empty() {
            return self.plain(v);
        }
        match v {
            V::L(items) => {
                let (page, cursor, more) = self.connection(items, &self.args(f));
                let mut out = Map::new();
                for sel in &f.selections {
                    let Selection::Field(sf) = sel else { continue };
                    if !self.included(&sf.directives) {
                        continue;
                    }
                    match sf.name.as_str() {
                        "nodes" => {
                            let nodes: Vec<Value> = page.iter().map(|n| self.object(n, &sf.selections)).collect();
                            out.insert(sf.key().into(), Value::Array(nodes));
                        }
                        "pageInfo" => {
                            let mut info = Map::new();
                            for s in &sf.selections {
                                if let Selection::Field(x) = s {
                                    let v = match x.name.as_str() {
                                        "hasNextPage" => json!(more),
                                        "endCursor" => json!(cursor),
                                        _ => Value::Null,
                                    };
                                    info.insert(x.key().into(), v);
                                }
                            }
                            out.insert(sf.key().into(), Value::Object(info));
                        }
                        _ => {}
                    }
                }
                Value::Object(out)
            }
            V::N(_) => self.object(v, &f.selections),
            V::S(_) => Value::Null,
        }
    }

    /// A value asked for without a selection: as it is.
    fn plain(&self, v: &V) -> Value {
        match v {
            V::S(x) => x.clone(),
            V::L(l) => Value::Array(l.iter().map(|x| self.plain(x)).collect()),
            V::N(_) => json!({}),
        }
    }

    fn object(&self, v: &V, set: &[Selection]) -> Value {
        let V::N(i) = v else { return Value::Null };
        let node = &self.w.nodes[*i];
        let mut out = Map::new();
        for sel in set {
            match sel {
                Selection::Field(f) => {
                    if self.included(&f.directives) {
                        let value = node.get(&f.name).map_or(Value::Null, |x| self.value(x, f));
                        merge(out.entry(f.key()).or_insert(Value::Null), value);
                    }
                }
                Selection::Inline { on, selections } => {
                    if on.as_ref().is_none_or(|t| *t == text_of(node, "__typename"))
                        && let Value::Object(m) = self.object(v, selections)
                    {
                        out.extend(m);
                    }
                }
                Selection::Spread => {}
            }
        }
        Value::Object(out)
    }

    /// Filters, orders and pages a list as Linear's connections do.
    fn connection(&self, all: &[V], args: &Map<String, Value>) -> (Vec<V>, String, bool) {
        let mut items: Vec<V> = vec![];
        for n in all {
            if let V::N(i) = n {
                let archived = !matches!(self.w.nodes[*i].get("archivedAt"), None | Some(V::S(Value::Null)));
                if args.get("includeArchived") != Some(&json!(true)) && archived {
                    continue;
                }
            }
            if let Some(Value::Object(f)) = args.get("filter")
                && !self.matches(Some(n), f)
            {
                continue;
            }
            items.push(n.clone());
        }
        let key = |n: &V, k: &str| match n {
            V::N(i) => text_of(&self.w.nodes[*i], k),
            _ => String::new(),
        };
        if args.get("sort").is_some_and(|s| !s.is_null()) {
            let desc = json::marshal(&args["sort"]).contains("Descending");
            let rank = |n: &V| {
                let p = match n {
                    V::N(i) => match self.w.nodes[*i].get("priority") {
                        Some(V::S(x)) => x.as_f64().unwrap_or(0.0),
                        _ => 0.0,
                    },
                    _ => 0.0,
                };
                // No priority sorts after low, whichever way.
                if p == 0.0 {
                    5.0
                } else if desc {
                    p
                } else {
                    -p
                }
            };
            items.sort_by(|a, b| rank(a).partial_cmp(&rank(b)).unwrap_or(std::cmp::Ordering::Equal));
        } else if let Some(o) = args.get("orderBy").filter(|o| !o.is_null()) {
            let k = sprint(o);
            items.sort_by_key(|n| std::cmp::Reverse(key(n, &k)));
        }
        let mut start = 0;
        if let Some(Value::String(a)) = args.get("after") {
            start = a.strip_prefix('c').unwrap_or(a).parse().unwrap_or(0);
        }
        let first = match args.get("first") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(50.0) as i64,
            _ => 50,
        };
        let start = start.min(items.len());
        let end = ((start as i64 + first).max(start as i64) as usize).min(items.len());
        (items[start..end].to_vec(), format!("c{end}"), end < items.len())
    }

    /// The value of key in a node, None when the node is null.
    fn get<'b>(&'b self, node: Option<&'b V>, k: &str) -> Option<&'b V> {
        match node {
            Some(V::N(i)) => self.w.nodes[*i].get(k),
            _ => None,
        }
    }

    /// Whether node passes a Linear filter. None is a null node.
    fn matches(&self, node: Option<&V>, filter: &Map<String, Value>) -> bool {
        let node = node.filter(|n| matches!(n, V::N(_)));
        for (k, cond) in filter {
            match k.as_str() {
                "and" => {
                    if !cond
                        .as_array()
                        .into_iter()
                        .flatten()
                        .all(|f| self.matches(node, f.as_object().unwrap_or(&Map::new())))
                    {
                        return false;
                    }
                    continue;
                }
                "or" => {
                    if !cond
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|f| self.matches(node, f.as_object().unwrap_or(&Map::new())))
                    {
                        return false;
                    }
                    continue;
                }
                "null" => {
                    if node.is_none() != (*cond == json!(true)) {
                        return false;
                    }
                    continue;
                }
                _ => {}
            }
            if node.is_none() {
                return false;
            }
            let empty = Map::new();
            let c = cond.as_object().unwrap_or(&empty);
            let v = self.get(node, k);
            match v {
                Some(V::L(items)) => {
                    if !self.match_list(items, c) {
                        return false;
                    }
                }
                Some(n @ V::N(_)) => {
                    if !self.matches(Some(n), c) {
                        return false;
                    }
                }
                _ => {
                    let scalar = match v {
                        Some(V::S(x)) => x.clone(),
                        _ => Value::Null,
                    };
                    let ident = self.get(node, "identifier");
                    if k == "id" && ident.is_some_and(|i| !matches!(i, V::S(Value::Null))) {
                        // Linear's issue id filter also takes identifiers.
                        let ident = match ident {
                            Some(V::S(x)) => x.clone(),
                            _ => Value::Null,
                        };
                        if !compare(&scalar, c) && !compare(&ident, c) {
                            return false;
                        }
                        continue;
                    }
                    if scalar.is_null() && !only_comparators(c) {
                        if !self.matches(None, c) {
                            return false;
                        }
                        continue;
                    }
                    if !compare(&scalar, c) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn match_list(&self, items: &[V], c: &Map<String, Value>) -> bool {
        let empty = Map::new();
        for (op, f) in c {
            let sub = f.as_object().unwrap_or(&empty);
            match op.as_str() {
                "some" => {
                    if !items.iter().any(|it| self.matches(Some(it), sub)) {
                        return false;
                    }
                }
                "every" => {
                    if !items.iter().all(|it| self.matches(Some(it), sub)) {
                        return false;
                    }
                }
                "length" if !compare(&num(items.len() as f64), sub) => {
                    return false;
                }
                _ => {}
            }
        }
        true
    }
}

const COMPARATORS: &[&str] = &[
    "eq",
    "neq",
    "in",
    "nin",
    "eqIgnoreCase",
    "neqIgnoreCase",
    "contains",
    "containsIgnoreCase",
    "notContains",
    "startsWith",
    "endsWith",
    "gt",
    "gte",
    "lt",
    "lte",
    "null",
];

/// Puts value where a field's answer goes. A field selected twice, as
/// cycle { id } and cycle { name }, answers once with both selections, as
/// GraphQL merges them.
fn merge(into: &mut Value, value: Value) {
    match (into, value) {
        (Value::Object(have), Value::Object(more)) => {
            for (k, v) in more {
                merge(have.entry(k).or_insert(Value::Null), v);
            }
        }
        (slot, v) => *slot = v,
    }
}

fn only_comparators(c: &Map<String, Value>) -> bool {
    c.keys().all(|k| COMPARATORS.contains(&k.as_str()))
}

/// Applies comparators (eq, in, gte, null…) to a scalar.
fn compare(v: &Value, c: &Map<String, Value>) -> bool {
    use crate::util::eq_fold as fold;
    for (op, want) in c {
        let (s, ws) = (sprint(v), sprint(want));
        let ok = match op.as_str() {
            "eq" => !v.is_null() && s == ws,
            "neq" => s != ws,
            "eqIgnoreCase" => !v.is_null() && fold(&s, &ws),
            "neqIgnoreCase" => !fold(&s, &ws),
            "in" | "nin" => {
                let found = want.as_array().into_iter().flatten().any(|x| !v.is_null() && sprint(x) == s);
                found == (op == "in")
            }
            "contains" => s.contains(&ws),
            "containsIgnoreCase" => s.to_lowercase().contains(&ws.to_lowercase()),
            "notContains" => !s.contains(&ws),
            "startsWith" => s.starts_with(&ws),
            "endsWith" => s.ends_with(&ws),
            "gt" | "gte" | "lt" | "lte" => !v.is_null() && order(v, want, op),
            "null" => v.is_null() == (*want == json!(true)),
            _ => return false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Compares numbers as numbers and times as times.
fn order(v: &Value, want: &Value, op: &str) -> bool {
    let d = if let Value::Number(a) = v {
        a.as_f64().unwrap_or(0.0) - want.as_f64().unwrap_or(0.0)
    } else {
        match (time::parse_rfc3339(&sprint(v)), time::parse_rfc3339(&sprint(want))) {
            (Some(a), Some(b)) => (a - b) as f64,
            _ => return false,
        }
    };
    match op {
        "gt" => d > 0.0,
        "gte" => d >= 0.0,
        "lt" => d < 0.0,
        _ => d <= 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_slugs_as_before() {
        // Recorded from the Go implementation.
        assert_eq!(
            slug("Checkout fails when a coupon code has trailing whitespace"),
            "checkout-fails-when-a-coupon-code-has-trailing-whitespace"
        );
        assert_eq!(slug("  Ünïcode & co!  "), "n-code-co");
        assert_eq!(demo_id("team", "ENG").len(), 36);
        assert_eq!(&demo_id("team", "ENG")[14..15], "5");
    }

    #[test]
    fn refuses_writes() {
        let a = answer("mutation { issueCreate(input: {}) { success } }", &Value::Null);
        assert!(a.errors[0]["message"].as_str().unwrap().contains("read-only"));
    }
}
