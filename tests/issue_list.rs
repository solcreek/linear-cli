//! What issue list sends, checked on the built binary against a fake Linear.

#[allow(dead_code)]
#[path = "../src/testserver.rs"]
mod testserver;

use serde_json::{Value, json};
use std::process::Command;
use testserver::{Response, serve};

/// Runs linearctl with args against the fake at url, with no stored state,
/// as tests/hints.rs does: its exit code and stdout.
fn linearctl(url: &str, args: &[&str]) -> (i32, String) {
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("issue-list-home");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_linearctl"));
    for (k, _) in std::env::vars_os() {
        let steers =
            |k: &str| k.starts_with("LINEAR") || k.starts_with("XDG_") || k.to_ascii_uppercase().ends_with("_PROXY");
        if k.to_str().is_some_and(steers) {
            cmd.env_remove(k);
        }
    }
    let out = cmd
        .args(args)
        .env("LINEAR_API_KEY", "lin_api_test0000000000000000")
        .env("LINEARCTL_API_URL", url)
        .env("LINEARCTL_HOME", &home)
        .env("HOME", &home)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap())
}

/// A fake Linear with no issues.
fn linear() -> testserver::Server {
    serve(|_| {
        let page = json!({"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}});
        Response::json(200, &json!({"data": {"issues": page}}).to_string())
    })
}

/// A label as an issue row has it.
fn label(n: u32) -> Value {
    json!({"id": format!("l{n}"), "name": format!("L{n}"), "color": "#000000"})
}

/// A fake Linear with two issues: ENG-1 with n labels, of which the row
/// reads at most 20, and ENG-2 with one. Labels by id answer from all n.
fn labelled(n: u32) -> testserver::Server {
    serve(move |req| {
        let body: Value = serde_json::from_str(&req.body).unwrap();
        let data = if body["query"].as_str().unwrap_or("").starts_with("query LabelsByID") {
            let ids = body["variables"]["ids"].as_array().cloned().unwrap_or_default();
            let found: Vec<Value> = (1..=n).map(label).filter(|l| ids.contains(&l["id"])).collect();
            json!({"issueLabels": {"nodes": found}})
        } else {
            let row = |id: &str, n: u32| {
                json!({"id": id, "identifier": id, "title": "t",
                    "labels": {"nodes": (1..=n.min(20)).map(label).collect::<Vec<_>>()},
                    "labelIds": (1..=n).map(|i| format!("l{i}")).collect::<Vec<_>>()})
            };
            let nodes = json!([row("ENG-1", n), row("ENG-2", 1)]);
            json!({"issues": {"nodes": nodes, "pageInfo": {"hasNextPage": false, "endCursor": null}}})
        };
        Response::json(200, &json!({"data": data}).to_string())
    })
}

/// The variables of each request the fake got.
fn sent(api: &testserver::Server) -> Vec<Value> {
    let reqs = api.requests.lock().unwrap();
    reqs.iter().map(|r| serde_json::from_str::<Value>(&r.body).unwrap()["variables"].clone()).collect()
}

#[test]
fn state_names_match_ignoring_case() {
    let api = linear();
    let (code, out) = linearctl(&api.url, &["issue", "list", "--state", "todo", "--json"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = linearctl(&api.url, &["issue", "list", "--state", "todo", "--state", "in review", "--json"]);
    assert_eq!(code, 0, "{out}");
    let got = sent(&api);
    assert_eq!(got[0]["filter"], json!({"state": {"name": {"eqIgnoreCase": "todo"}}}));
    // Linear has no case-insensitive "in": an or of eqIgnoreCase.
    assert_eq!(
        got[1]["filter"],
        json!({"and": [{"or": [
            {"state": {"name": {"eqIgnoreCase": "todo"}}},
            {"state": {"name": {"eqIgnoreCase": "in review"}}}
        ]}]})
    );
}

#[test]
fn rows_carry_every_label() {
    let api = labelled(22);
    let (code, out) = linearctl(&api.url, &["issue", "list", "--json"]);
    assert_eq!(code, 0, "{out}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    let names = |i: usize| -> Vec<String> {
        doc["items"][i]["labels"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().into()).collect()
    };
    assert_eq!(names(0), (1..=22).map(|i| format!("L{i}")).collect::<Vec<_>>());
    assert_eq!(names(1), ["L1"]);
    // labelIds is how linearctl knows; rows do not print it.
    assert!(doc["items"].as_array().unwrap().iter().all(|it| it.get("labelIds").is_none()), "{out}");
    // One more request, for the two labels past 20.
    let got = sent(&api);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1]["ids"], json!(["l21", "l22"]));
}

#[test]
fn rows_with_all_their_labels_cost_no_request() {
    let api = labelled(20);
    let (code, out) = linearctl(&api.url, &["issue", "list", "--json"]);
    assert_eq!(code, 0, "{out}");
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["items"][0]["labels"].as_array().unwrap().len(), 20);
    assert_eq!(sent(&api).len(), 1);
}

/// A fake Linear that knows team ENG and finds no issues; each request's
/// operation name and variables.
fn teams() -> testserver::Server {
    serve(|req| {
        let body: Value = serde_json::from_str(&req.body).unwrap();
        let data = if body["query"].as_str().unwrap_or("").starts_with("query TeamFind") {
            let eng = body["variables"]["key"] == "ENG";
            json!({"teams": {"nodes": if eng { json!([{"id": "team-eng"}]) } else { json!([]) }}})
        } else {
            let page = json!({"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}});
            json!({"issues": page, "searchIssues": page})
        };
        Response::json(200, &json!({"data": data}).to_string())
    })
}

fn ops(api: &testserver::Server) -> Vec<(String, Value)> {
    let reqs = api.requests.lock().unwrap();
    reqs.iter()
        .map(|r| {
            let body: Value = serde_json::from_str(&r.body).unwrap();
            let q = body["query"].as_str().unwrap_or("").trim_start_matches("query ");
            (q.split(['(', ' ', '{']).next().unwrap_or("").to_string(), body["variables"].clone())
        })
        .collect()
}

#[test]
fn a_search_of_one_team_passes_its_id_as_team_id() {
    let api = teams();
    let (code, out) = linearctl(&api.url, &["issue", "list", "--query", "checkout", "--team", "eng", "--json"]);
    assert_eq!(code, 0, "{out}");
    let got = ops(&api);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!((got[0].0.as_str(), &got[0].1["key"]), ("TeamFind", &json!("ENG")));
    assert_eq!(got[1].0, "IssueSearch");
    assert_eq!(got[1].1["teamId"], "team-eng");
    // The filter stays.
    assert_eq!(got[1].1["filter"], json!({"team": {"key": {"eq": "ENG"}}}));
}

#[test]
fn team_id_only_for_a_search_of_one_known_team() {
    let api = teams();
    let cases: [&[&str]; 3] = [
        &["--query", "x", "--team", "ENG", "--team", "WEB"], // teamId takes one
        &["--team", "ENG"],                                  // not a search
        &["--query", "x", "--team", "NOPE"],                 // looked up, not found
    ];
    for args in cases {
        let (code, out) = linearctl(&api.url, &[&["issue", "list", "--json"], args].concat());
        assert_eq!(code, 0, "{args:?}: {out}");
    }
    let got = ops(&api);
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["IssueSearch", "IssueList", "TeamFind", "IssueSearch"]);
    assert!(got.iter().all(|(_, v)| v.get("teamId").is_none()), "{got:?}");
}

/// A fake Linear whose list, search and saved view all answer ENG-1, and
/// whose IssueRelations answers what it is asked for: ENG-1's parent and
/// one relation each way.
fn related() -> testserver::Server {
    serve(|req| {
        let body: Value = serde_json::from_str(&req.body).unwrap();
        let q = body["query"].as_str().unwrap_or("");
        let page = json!({"nodes": [{"id": "id-1", "identifier": "ENG-1", "title": "t"}],
            "pageInfo": {"hasNextPage": false, "endCursor": null}});
        let side = |n: &str, t: &str| json!({"id": n, "identifier": n, "title": "t", "state": {"id": t, "name": t, "type": t}});
        let rel = |a: &str, b: &str| {
            json!({"nodes": [{"id": "r", "type": "blocks", "issue": side(a, "started"), "relatedIssue": side(b, "unstarted")}],
                "pageInfo": {"hasNextPage": false, "endCursor": null}})
        };
        let data = if q.starts_with("query IssueRelations") {
            let v = &body["variables"];
            let mut it = json!({"id": "id-1"});
            if v["withParent"] == true {
                it["parent"] = side("ENG-0", "completed");
            }
            if v["withRelations"] == true {
                it["relations"] = rel("ENG-1", "ENG-2");
            }
            if v["withInverse"] == true {
                it["inverseRelations"] = rel("ENG-3", "ENG-1");
            }
            json!({"issues": {"nodes": [it]}})
        } else if q.starts_with("query IssueListView") {
            json!({"matches": {"nodes": [{"id": "v", "name": "V", "modelName": "Issue"}]},
                "customViews": {"nodes": [{"issues": page}]}})
        } else {
            json!({"issues": page, "searchIssues": page})
        };
        Response::json(200, &json!({"data": data}).to_string())
    })
}

#[test]
fn relation_fields_come_by_id_after_the_rows_on_every_path() {
    for path in [&[][..], &["--query", "x"][..], &["--view", "V"][..]] {
        let api = related();
        let args =
            [&["issue", "list", "--fields", "identifier,parent,relations,inverseRelations", "--json"], path].concat();
        let (code, out) = linearctl(&api.url, &args);
        assert_eq!(code, 0, "{path:?}: {out}");
        let doc: Value = serde_json::from_str(&out).unwrap();
        let it = &doc["items"][0];
        assert_eq!(it["parent"]["state"]["type"], "completed", "{path:?}: {out}");
        assert_eq!(it["relations"][0]["relatedIssue"]["identifier"], "ENG-2");
        assert_eq!(it["relations"][0]["relatedIssue"]["state"]["type"], "unstarted");
        assert_eq!(it["inverseRelations"][0]["issue"]["state"]["name"], "started");
        let got = ops(&api);
        assert_eq!(got.len(), 2, "{path:?}: {got:?}");
        assert_eq!(got[1].0, "IssueRelations");
        assert_eq!(got[1].1["ids"], json!(["id-1"]));
        for with in ["withParent", "withRelations", "withInverse"] {
            assert_eq!(got[1].1[with], true, "{path:?}: {got:?}");
        }
    }
}

#[test]
fn rows_without_relation_fields_cost_no_request() {
    let api = related();
    for fields in [&[][..], &["--fields", "identifier,state"][..]] {
        let (code, out) = linearctl(&api.url, &[&["issue", "list", "--json"], fields].concat());
        assert_eq!(code, 0, "{out}");
        let doc: Value = serde_json::from_str(&out).unwrap();
        for k in ["parent", "relations", "inverseRelations"] {
            assert!(doc["items"][0].get(k).is_none(), "{fields:?}: {out}");
        }
    }
    assert!(ops(&api).iter().all(|(n, _)| n == "IssueList"), "{:?}", ops(&api));
}
