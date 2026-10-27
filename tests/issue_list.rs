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
