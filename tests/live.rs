//! The live tests talk to api.linear.app with a real key and change a real
//! workspace, so they run only when asked: LINEARCTL_LIVE_TEAM names a team
//! whose issues may be created and archived (a sandbox), and LINEAR_API_KEY a
//! key for it. Each run leaves three archived issues behind, titled
//! "linearctl live test …"; linearctl has no delete.
//!
//!     LINEARCTL_LIVE_TEAM=SANDBOX LINEAR_API_KEY=… make live

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BIN: &str = env!("CARGO_BIN_EXE_linearctl");

/// The sandbox team, or None (and a note) when the run did not ask for one.
fn live_team() -> Option<String> {
    let team = std::env::var("LINEARCTL_LIVE_TEAM").unwrap_or_default();
    if team.is_empty() || std::env::var("LINEAR_API_KEY").unwrap_or_default().is_empty() {
        eprintln!("live: skipped; set LINEARCTL_LIVE_TEAM (a sandbox team) and LINEAR_API_KEY");
        return None;
    }
    Some(team)
}

/// A directory of its own for one run's LINEARCTL_HOME and working directory.
fn scratch() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("linearctl-live-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// linearctl against the real API: the key from the environment, nothing
/// else of linearctl's, an empty home.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(BIN);
    c.args(args).current_dir(dir);
    for (k, _) in std::env::vars() {
        if k.starts_with("LINEARCTL_") {
            c.env_remove(k);
        }
    }
    c.env("LINEARCTL_HOME", dir.join(".home"));
    c
}

/// Runs a command to completion and returns its JSON answer.
fn live(args: &[&str]) -> Value {
    let dir = scratch();
    let mut all = args.to_vec();
    all.push("--json");
    let out = command(&dir, &all).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn at<'a>(v: &'a Value, path: &str) -> &'a str {
    path.split('.').fold(v, |v, k| &v[k]).as_str().unwrap_or("")
}

/// A running watch, its lines as they come.
struct Watch {
    child: Child,
    lines: Receiver<Value>,
    /// It said Linear allows one subscription: it watches updates alone and
    /// catches creates and archives up every 60 s.
    single: Arc<AtomicBool>,
}

impl Watch {
    fn start(team: &str) -> Watch {
        let dir = scratch();
        let mut child = command(
            &dir,
            &["watch", "--team", team, "--json", "--batch-ms", "200", "--heartbeat", "5", "--reconcile", "86400"],
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let v = serde_json::from_str(&line).unwrap_or(Value::String(line));
                if tx.send(v).is_err() {
                    return;
                }
            }
        });
        let err = child.stderr.take().unwrap();
        let single = Arc::new(AtomicBool::new(false));
        let s = single.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                if line.contains("allows one subscription") {
                    s.store(true, Ordering::SeqCst);
                }
                eprintln!("{line}");
            }
        });
        Watch { child, lines, single }
    }

    /// Lets the first seconds pass, gaps and all: when Linear allows one
    /// subscription, watch learns it from the first connection and
    /// reconnects with updates alone.
    fn settle(&self, d: Duration) {
        let deadline = Instant::now() + d;
        while let Ok(v) = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            eprintln!("live: settling: {v}");
        }
    }

    fn single(&self) -> bool {
        self.single.load(Ordering::SeqCst)
    }

    /// The first line with this event for which ok holds, within limit. A
    /// gap fails it: watch then catches up over HTTP, which could deliver
    /// the change without the subscription the test is about.
    fn wait(&self, event: &str, limit: Duration, ok: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + limit;
        loop {
            match self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(v) if v["event"] == "gap" => panic!("the subscription dropped while waiting for {event}: {v}"),
                Ok(v) if v["event"] == event && ok(&v) => return v,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => panic!("no {event} within {limit:?}"),
                Err(RecvTimeoutError::Disconnected) => panic!("watch ended waiting for {event}"),
            }
        }
    }

    /// Interrupts it as Ctrl-C would; its exit code.
    #[cfg(unix)]
    fn stop(mut self) -> Option<i32> {
        unsafe { libc::kill(self.child.id() as i32, libc::SIGINT) };
        self.child.wait().unwrap().code()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Archives the issue unless the test got that far. Best effort: this runs
/// while a failed test unwinds, where a second panic would abort the run.
struct Archive(String, bool);

impl Drop for Archive {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        let dir = std::env::temp_dir().join(format!("linearctl-live-cleanup-{}", std::process::id()));
        let archived = std::fs::create_dir_all(&dir)
            .and_then(|()| command(&dir, &["issue", "archive", &self.0, "--yes", "--json"]).output());
        match archived {
            Ok(out) if out.status.success() => eprintln!("live: archived {}", self.0),
            Ok(out) => eprintln!("live: could not archive {}: {}", self.0, String::from_utf8_lossy(&out.stderr).trim()),
            Err(e) => eprintln!("live: could not archive {}: {e}", self.0),
        }
    }
}

fn new_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).unwrap();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// Runs a command that may exit 3 (an --id not found) and says whether
/// issue list listed ident.
fn listed(args: &[&str], ident: &str) -> bool {
    let dir = scratch();
    let mut all = args.to_vec();
    all.extend(["--id", ident, "--json"]);
    let out = command(&dir, &all).output().unwrap();
    let code = out.status.code();
    assert!(matches!(code, Some(0 | 3)), "{all:?}: {}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    v["items"].as_array().unwrap().iter().any(|it| at(it, "identifier") == ident)
}

/// Waits up to 60 s for issue list --blocked to list ident or not, as
/// want says: Linear's filter may follow a change a moment later.
fn wait_blocked(ident: &str, want: bool, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if listed(&["issue", "list", "--blocked"], ident) == want {
            eprintln!("live: {ident} {} --blocked: {why}", if want { "in" } else { "out of" });
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{ident} still {} --blocked 60 s after: {why}",
            if want { "out of" } else { "in" }
        );
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// --blocked is Linear's hasBlockedByRelations, and Linear counts a
/// blocker only while it is open: completed or canceled, it drops out;
/// reopened, it counts again. Each side of the relation carries the
/// blocker's state, in issue show and in issue list's inverseRelations.
/// Whether a duplicate-state blocker counts is printed, not asserted: it
/// was never measured. Both issues are archived at the end.
#[test]
fn blocked_counts_only_an_open_blocker() {
    let Some(team) = live_team() else { return };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let create = |what: &str| {
        let title = format!("linearctl live test {stamp} {what}");
        let v = live(&["issue", "create", "--team", &team, "--title", &title, "--id", &new_id(), "--yes"]);
        at(&v, "issue.identifier").to_string()
    };
    let blocker = create("blocker");
    let mut keep_blocker = Archive(blocker.clone(), false);
    let blocked = create("blocked");
    let mut keep_blocked = Archive(blocked.clone(), false);
    eprintln!("live: {blocker} blocks {blocked}");
    live(&["relation", "create", &blocker, "blocks", &blocked, "--yes"]);
    wait_blocked(&blocked, true, "an open blocker");

    let state_of_blocker = || {
        let v = live(&["issue", "list", "--id", &blocked, "--fields", "identifier,inverseRelations"]);
        let rel = &v["items"][0]["inverseRelations"][0];
        assert_eq!(at(rel, "issue.identifier"), blocker, "inverseRelations {v}");
        let shown = live(&["issue", "show", &blocked, "--fields", "inverseRelations"]);
        assert_eq!(shown["issue"]["inverseRelations"][0]["issue"]["state"], rel["issue"]["state"]);
        at(rel, "issue.state.type").to_string()
    };
    live(&["issue", "update", &blocker, "--state-type", "completed", "--yes"]);
    assert_eq!(state_of_blocker(), "completed");
    wait_blocked(&blocked, false, "its blocker completed");

    live(&["issue", "update", &blocker, "--state-type", "unstarted", "--yes"]);
    assert_eq!(state_of_blocker(), "unstarted");
    wait_blocked(&blocked, true, "its blocker reopened");

    live(&["issue", "update", &blocker, "--state-type", "canceled", "--yes"]);
    assert_eq!(state_of_blocker(), "canceled");
    wait_blocked(&blocked, false, "its blocker canceled");

    let dir = scratch();
    let dup =
        command(&dir, &["issue", "update", &blocker, "--state-type", "duplicate", "--yes", "--json"]).output().unwrap();
    if dup.status.success() {
        std::thread::sleep(Duration::from_secs(10));
        let counts = listed(&["issue", "list", "--blocked"], &blocked);
        eprintln!(
            "live: a blocker in a duplicate-type state {} (after 10 s)",
            if counts { "counts" } else { "does not count" }
        );
    } else {
        eprintln!("live: no duplicate-type state to try: {}", String::from_utf8_lossy(&dup.stdout).trim());
    }

    for (ident, keep) in [(&blocked, &mut keep_blocked), (&blocker, &mut keep_blocker)] {
        live(&["issue", "archive", ident, "--yes"]);
        keep.1 = true;
    }
}

/// A change made in Linear reaches watch: an issue created, its title
/// changed, then archived. With every subscription each is seen as it
/// happens. When Linear allows one, the title change still is (issueUpdated),
/// and the create and archive come by the catch-up within its 60 s.
#[test]
#[cfg(unix)]
fn watch_sees_changes_as_they_happen() {
    let Some(team) = live_team() else { return };
    let w = Watch::start(&team);
    let ready = w.wait("ready", Duration::from_secs(60), |_| true);
    assert_eq!(ready["mode"], "subscription", "ready {ready}");
    w.settle(Duration::from_secs(5));
    let single = w.single();
    eprintln!("live: {}", if single { "one subscription: updates alone" } else { "every subscription" });

    let title = format!("linearctl live test {}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
    let created = live(&["issue", "create", "--team", &team, "--title", &title, "--id", &new_id(), "--yes"]);
    let ident = at(&created, "issue.identifier").to_string();
    let mut cleanup = Archive(ident.clone(), false);
    eprintln!("live: created {ident}");
    let of_issue = |v: &Value| at(v, "issue.identifier") == ident;
    // Through a subscription: 30 s is generous. By catch-up: up to 60 s.
    let fast = Duration::from_secs(30);
    let limit = if single { Duration::from_secs(90) } else { fast };

    let start = Instant::now();
    let up = w.wait("upsert", limit, of_issue);
    eprintln!("live: create seen after {:?}", start.elapsed());
    assert_eq!(at(&up, "issue.title"), title);

    let renamed = format!("{title} (updated)");
    let start = Instant::now();
    live(&["issue", "update", &ident, "--title", &renamed, "--yes"]);
    w.wait("upsert", fast, |v| of_issue(v) && at(v, "issue.title") == renamed);
    eprintln!("live: update seen after {:?}", start.elapsed());

    let start = Instant::now();
    live(&["issue", "archive", &ident, "--yes"]);
    cleanup.1 = true;
    let gone = w.wait("remove", limit, |v| at(v, "identifier") == ident);
    eprintln!("live: archive seen after {:?}", start.elapsed());
    assert_eq!(gone["reason"], "archived", "remove {gone}");
    assert_eq!(w.stop(), Some(130), "watch exits 130 on Ctrl-C");
}
