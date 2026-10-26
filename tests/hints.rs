//! The `hint` a failure carries, checked on the built binary against a fake
//! Linear: under --json a `hint` field, without it a `hint:` line on stderr.
//! (docs/design.md, "Exit codes".)

#[allow(dead_code)]
#[path = "../src/testserver.rs"]
mod testserver;

use serde_json::Value;
use std::process::Command;
use testserver::{Response, serve};

/// Runs linearctl with args against the fake at url, with no stored state:
/// its exit code, stdout and stderr. The environment is kept but for
/// LINEAR* and XDG_*, as tests/golden does: Windows needs SystemRoot and
/// the like to open a socket.
fn linearctl(url: &str, args: &[&str]) -> (i32, String, String) {
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("hints-home");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_linearctl"));
    for (k, _) in std::env::vars_os() {
        if k.to_str().is_some_and(|k| k.starts_with("LINEAR") || k.starts_with("XDG_")) {
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
    (out.status.code().unwrap(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

/// A fake Linear that answers every query with this data.
fn answering(data: &'static str) -> testserver::Server {
    serve(move |_| Response::json(200, &format!(r#"{{"data":{data}}}"#)))
}

#[test]
fn conflict_hints_at_reading_the_issue_again() {
    let api = answering(r#"{"issue":{"id":"id-ENG-5","identifier":"ENG-5","updatedAt":"2026-10-02T08:00:00.000Z"}}"#);
    let args = ["issue", "update", "ENG-5", "--title", "New title", "--if-updated-at", "2026-10-01T00:00:00Z"];
    let (code, out, _) = linearctl(&api.url, &[&args[..], &["--yes", "--json"]].concat());
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, 7, "{out}");
    assert!(doc["error"].as_str().unwrap().contains("changed at 2026-10-02T08:00:00.000Z"), "{out}");
    assert_eq!(doc["hint"], "'linearctl issue show ENG-5' reads it; decide again from what it shows");
    // One request, the read: nothing was sent to change it.
    assert_eq!(api.requests.lock().unwrap().len(), 1);
}

#[test]
fn missing_cycle_hints_at_cycle_list() {
    let api = answering(r#"{"matches":{"nodes":[]},"cycles":{"nodes":[]}}"#);
    let (code, out, _) = linearctl(&api.url, &["cycle", "show", "next", "--team", "ENG", "--json"]);
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(code, 1, "{out}");
    assert!(doc["error"].as_str().unwrap().starts_with("not found: cycle next"), "{out}");
    assert_eq!(doc["hint"], "'linearctl cycle list --team KEY' lists a team's cycles");
    // Without --json, the same hint is a line on stderr.
    let (code, out, err) = linearctl(&api.url, &["cycle", "show", "next", "--team", "ENG"]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.ends_with("hint: 'linearctl cycle list --team KEY' lists a team's cycles\n"), "{err}");
}
