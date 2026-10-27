//! watch against a fake Linear whose subscriptions misbehave as the real
//! one has: closing every connection right after it subscribes, or closing
//! with 4003 any connection that asks for more than one subscription. Then
//! the watches of one key sharing one subscription through a hub.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;

const BIN: &str = env!("CARGO_BIN_EXE_linearctl");
const KEY: &str = "lin_api_test0000000000000000";

#[derive(Clone, Copy)]
enum Behavior {
    /// Every connection is closed (1011) right after its subscriptions.
    CloseEverything,
    /// A connection asking for a second subscription is closed with 4003.
    OneSubscription,
    /// As OneSubscription, and so is one subscribing while another
    /// connection holds the account's subscription, as Linear does.
    OneSubscriptionPerAccount,
}

/// What the fake saw.
#[derive(Default)]
struct Seen {
    /// The subscription ids of each connection, in order.
    connections: Vec<Vec<String>>,
    /// The connections closed with 4003.
    too_many: usize,
    /// The GraphQL operations asked for, in order.
    requests: Vec<String>,
}

struct Fake {
    url: String,
    seen: Arc<Mutex<Seen>>,
    /// Sockets still open, to push a change down.
    open: Arc<Mutex<Vec<tungstenite::WebSocket<TcpStream>>>>,
}

impl Fake {
    fn start(behavior: Behavior) -> Fake {
        Fake::start_with(behavior, Data::default())
    }

    fn start_with(behavior: Behavior, data: Data) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let open = Arc::new(Mutex::new(vec![]));
        let (s, o) = (seen.clone(), open.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, o) = (s.clone(), o.clone());
                std::thread::spawn(move || serve(stream, behavior, data, s, o));
            }
        });
        Fake { url, seen, open }
    }

    fn connections(&self) -> Vec<Vec<String>> {
        self.seen.lock().unwrap().connections.clone()
    }

    fn too_many(&self) -> usize {
        self.seen.lock().unwrap().too_many
    }

    /// How many of the operations asked for so far are op.
    fn requests(&self, op: &str) -> usize {
        self.seen.lock().unwrap().requests.iter().filter(|r| *r == op).count()
    }

    /// Connections kept open that are still connected.
    fn live(&self) -> usize {
        live(&self.open)
    }

    /// Waits until exactly one kept connection is live, holding updates.
    fn settled(&self, limit: Duration) {
        let deadline = Instant::now() + limit;
        while self.live() != 1 {
            assert!(Instant::now() < deadline, "{} live connections: {:?}", self.live(), self.connections());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Tells every open socket that issue id was updated.
    fn push_update(&self, id: &str) {
        let msg = json!({"type": "next", "id": "updated", "payload": {"data": {"issueUpdated": {"id": id}}}});
        for ws in self.open.lock().unwrap().iter_mut() {
            let _ = ws.send(tungstenite::Message::text(msg.to_string()));
        }
    }
}

fn serve(
    stream: TcpStream,
    behavior: Behavior,
    data: Data,
    seen: Arc<Mutex<Seen>>,
    open: Arc<Mutex<Vec<tungstenite::WebSocket<TcpStream>>>>,
) {
    let mut head = [0u8; 2048];
    let n = stream.peek(&mut head).unwrap_or(0);
    if String::from_utf8_lossy(&head[..n]).to_ascii_lowercase().contains("upgrade: websocket") {
        websocket(stream, behavior, seen, open);
    } else {
        graphql(stream, data, seen);
    }
}

/// Drops the kept connections whose client has gone, and counts the rest.
/// Nothing reads them, so what a client sent (pings) is read and discarded
/// here first: a closed connection then reads as its end.
fn live(open: &Mutex<Vec<tungstenite::WebSocket<TcpStream>>>) -> usize {
    let mut open = open.lock().unwrap();
    open.retain(|ws| {
        let mut tcp = ws.get_ref();
        let _ = tcp.set_nonblocking(true);
        let mut buf = [0u8; 4096];
        let alive = loop {
            match tcp.read(&mut buf) {
                Ok(0) => break false,
                Ok(_) => continue,
                Err(e) => break e.kind() == std::io::ErrorKind::WouldBlock,
            }
        };
        let _ = tcp.set_nonblocking(false);
        alive
    });
    open.len()
}

/// graphql-transport-ws, as Linear speaks it.
#[allow(clippy::result_large_err)] // tungstenite's handshake callback returns its own Response
fn websocket(
    stream: TcpStream,
    behavior: Behavior,
    seen: Arc<Mutex<Seen>>,
    open: Arc<Mutex<Vec<tungstenite::WebSocket<TcpStream>>>>,
) {
    let callback = |_: &tungstenite::handshake::server::Request, mut res: tungstenite::handshake::server::Response| {
        res.headers_mut().insert("Sec-WebSocket-Protocol", "graphql-transport-ws".parse().unwrap());
        Ok(res)
    };
    let Ok(mut ws) = tungstenite::accept_hdr(stream, callback) else { return };
    let index = {
        let mut s = seen.lock().unwrap();
        s.connections.push(vec![]);
        s.connections.len() - 1
    };
    // Closes as Linear does, waiting for the client's answer: dropping the
    // socket with its messages unread would reset it on Windows, and the
    // close frame with its code could be lost.
    let close = |ws: &mut tungstenite::WebSocket<TcpStream>, code: u16| {
        if code == 4003 {
            seen.lock().unwrap().too_many += 1;
        }
        let _ = ws.close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }));
        let _ = ws.flush();
        while ws.read().is_ok() {}
    };
    let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(300)));
    loop {
        let msg = match ws.read() {
            Ok(tungstenite::Message::Text(t)) => t.to_string(),
            Ok(_) => continue,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                // Quiet: the subscriptions are in. Keep the socket for pushes.
                if matches!(behavior, Behavior::CloseEverything) && !seen.lock().unwrap().connections[index].is_empty()
                {
                    close(&mut ws, 1011);
                    return;
                }
                if matches!(behavior, Behavior::OneSubscription | Behavior::OneSubscriptionPerAccount)
                    && seen.lock().unwrap().connections[index].len() == 1
                {
                    let _ = ws.get_ref().set_read_timeout(None);
                    open.lock().unwrap().push(ws);
                    return;
                }
                continue;
            }
            Err(_) => return,
        };
        let v: Value = serde_json::from_str(&msg).unwrap_or(Value::Null);
        match v["type"].as_str() {
            Some("connection_init") => {
                let _ = ws.send(tungstenite::Message::text(r#"{"type":"connection_ack"}"#));
            }
            Some("subscribe") => {
                let subs = {
                    let mut s = seen.lock().unwrap();
                    s.connections[index].push(v["id"].as_str().unwrap_or("").to_string());
                    s.connections[index].len()
                };
                let taken = matches!(behavior, Behavior::OneSubscriptionPerAccount) && live(&open) > 0;
                if matches!(behavior, Behavior::OneSubscription | Behavior::OneSubscriptionPerAccount) && subs > 1
                    || taken
                {
                    close(&mut ws, 4003);
                    return;
                }
            }
            _ => {}
        }
    }
}

/// What the fake's GraphQL holds.
#[derive(Clone, Copy, Default)]
struct Data {
    /// How long the snapshot query (WatchIDs) takes to answer.
    snapshot: Duration,
    /// Issues that match at start and have since been archived, ids arch-N:
    /// in the snapshot, changed since any time, no longer matching.
    archived: usize,
}

/// One page of ids from 0..n, as Linear pages: first (or the query's own
/// limit) from after.
fn page(req: &Value, n: usize, limit: usize) -> Value {
    let first = req["variables"]["first"].as_u64().map_or(limit, |f| f as usize);
    let from = req["variables"]["after"].as_str().and_then(|a| a.parse().ok()).unwrap_or(0);
    let to = n.min(from + first);
    let nodes: Vec<Value> =
        (from..to).map(|i| json!({"id": format!("arch-{i}"), "identifier": format!("ENG-{}", 100 + i)})).collect();
    json!({"nodes": nodes, "pageInfo": {"hasNextPage": to < n, "endCursor": to.to_string()}})
}

/// Answers watch's queries: at start only the archived issues match; an
/// issue asked for by id is there and matches, unless it is archived, or
/// the filter names a team whose key its id lacks (id-OPS-1 is not ENG's).
fn graphql(stream: TcpStream, fake: Data, seen: Arc<Mutex<Seen>>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut stream = stream;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            let h = h.trim_end().to_ascii_lowercase();
            if h.is_empty() {
                break;
            }
            if let Some(v) = h.strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let req: Value = serde_json::from_slice(&body).unwrap();
        let query = req["query"].as_str().unwrap_or("");
        let op = query.split_whitespace().nth(1).unwrap_or("").split('(').next().unwrap_or("");
        seen.lock().unwrap().requests.push(op.to_string());
        if op == "WatchIDs" {
            std::thread::sleep(fake.snapshot);
        }
        let nodes: Vec<Value> = match op {
            "IssueList" => {
                let ids = &req["variables"]["filter"]["and"][1]["id"]["in"];
                let ids = if ids.is_array() { ids } else { &req["variables"]["filter"]["id"]["in"] };
                let team = req["variables"]["filter"]["and"][0]["team"]["key"]["eq"].as_str().map(|k| format!("-{k}-"));
                ids.as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str())
                    .filter(|id| !id.starts_with("arch-"))
                    .filter(|id| team.as_ref().is_none_or(|t| id.contains(t.as_str())))
                    .map(|id| {
                        let ident = id.strip_prefix("id-").unwrap_or(id);
                        json!({"id": id, "identifier": ident, "title": "One", "state": {"name": "Todo"}})
                    })
                    .collect()
            }
            // At most 250 a request, as the query asks.
            "WatchGone" => req["variables"]["ids"]
                .as_array()
                .into_iter()
                .flatten()
                .take(250)
                .filter_map(|id| id.as_str()?.strip_prefix("arch-")?.parse::<usize>().ok())
                .map(|i| {
                    json!({"id": format!("arch-{i}"), "identifier": format!("ENG-{}", 100 + i),
                           "archivedAt": "2026-10-26T00:00:00.000Z", "trashed": false})
                })
                .collect(),
            _ => vec![],
        };
        let data = match op {
            "WatchGone" => json!({"issues": {"nodes": nodes}}),
            "WatchIDs" => json!({"issues": page(&req, fake.archived, 250)}),
            // Archiving leaves updatedAt: only archivedAt finds an archive.
            "WatchChanged" => {
                let f = req["variables"]["filter"].to_string();
                let n = if f.contains("archivedAt") { fake.archived } else { 0 };
                json!({"issues": page(&req, n, 250)})
            }
            _ => json!({"issues": {"nodes": nodes, "pageInfo": {"hasNextPage": false, "endCursor": null}}}),
        };
        let body = json!({"data": data}).to_string();
        let res = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        if stream.write_all(res.as_bytes()).is_err() {
            return;
        }
    }
}

/// A running watch: its JSON lines, each with when it came, and its stderr
/// when it ends.
struct Watch {
    child: Child,
    lines: Receiver<(Instant, Value)>,
}

/// A directory for the hubs' sockets, as XDG_RUNTIME_DIR: the watches given
/// the same one share a hub. Short, as a socket's path must be.
struct Runtime(std::path::PathBuf);

impl Runtime {
    fn new() -> Runtime {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmp = if cfg!(unix) { std::path::PathBuf::from("/tmp") } else { std::env::temp_dir() };
        let dir = tmp.join(format!("lcw-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Runtime(dir)
    }

    /// The pid of the one hub there, from its pid file.
    fn hub(&self) -> Option<i32> {
        let mut pids = vec![];
        for e in std::fs::read_dir(self.0.join("linearctl")).ok()?.flatten() {
            if e.path().extension().is_some_and(|x| x == "pid") {
                pids.push(std::fs::read_to_string(e.path()).ok()?.trim().parse().ok()?);
            }
        }
        assert!(pids.len() <= 1, "more than one hub: {pids:?}");
        pids.pop()
    }

    /// Waits for a hub there other than not.
    fn hub_other_than(&self, not: Option<i32>, limit: Duration) -> i32 {
        let deadline = Instant::now() + limit;
        loop {
            match self.hub() {
                Some(p) if Some(p) != not => return p,
                _ => assert!(Instant::now() < deadline, "no new hub within {limit:?}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(p) = self.hub() {
            kill(p);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn kill(pid: i32) {
    // SAFETY: a signal to a process; no memory is touched.
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

impl Watch {
    /// Watch against the fake. The environment is kept but for LINEAR* and
    /// XDG_* (LINEARCTL_DEMO would poll demo data instead), and *_PROXY,
    /// which would send the request for 127.0.0.1 to a proxy, as in
    /// tests/hints.rs.
    fn start(fake: &Fake) -> Watch {
        Watch::start_in(fake, &Runtime::new(), &[])
    }

    /// Watch with more arguments, its hub's socket under runtime.
    fn start_in(fake: &Fake, runtime: &Runtime, args: &[&str]) -> Watch {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("linearctl-watch-{}-{nanos}", std::process::id()));
        let mut cmd = Command::new(BIN);
        for (k, _) in std::env::vars_os() {
            let steers = |k: &str| {
                k.starts_with("LINEAR") || k.starts_with("XDG_") || k.to_ascii_uppercase().ends_with("_PROXY")
            };
            if k.to_str().is_some_and(steers) {
                cmd.env_remove(k);
            }
        }
        let mut child = cmd
            .args(["watch", "--json", "--batch-ms", "50", "--heartbeat", "60"])
            .args(args)
            .env("XDG_RUNTIME_DIR", &runtime.0)
            .env("LINEARCTL_API_URL", &fake.url)
            .env("LINEAR_API_KEY", KEY)
            .env("LINEARCTL_HOME", home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let out = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let v = serde_json::from_str(&line).unwrap_or(Value::String(line));
                if tx.send((Instant::now(), v)).is_err() {
                    return;
                }
            }
        });
        Watch { child, lines }
    }

    fn next(&self, event: &str, limit: Duration) -> Value {
        self.next_at(event, limit).1
    }

    /// The next line with this event, and when it came.
    fn next_at(&self, event: &str, limit: Duration) -> (Instant, Value) {
        let deadline = Instant::now() + limit;
        loop {
            let v = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            match v {
                Ok((at, v)) if v["event"] == event => return (at, v),
                Ok(_) => {}
                Err(_) => panic!("no {event} within {limit:?}"),
            }
        }
    }

    /// Stops it and returns its stderr.
    fn stderr(mut self) -> String {
        let _ = self.child.kill();
        let mut err = String::new();
        let _ = self.child.stderr.take().unwrap().read_to_string(&mut err);
        err
    }
}

/// A server that closes every connection at once is asked again after 1, 2
/// and 4 s, not every second: each lost connection was not healthy, so the
/// backoff keeps growing.
#[test]
fn reconnects_back_off_when_every_connection_drops() {
    let fake = Fake::start(Behavior::CloseEverything);
    let w = Watch::start(&fake);
    w.next("ready", Duration::from_secs(10));
    std::thread::sleep(Duration::from_secs(8));
    let n = fake.connections().len();
    let err = w.stderr();
    // Connected at 0, then after 1, 2, 4 s of backoff: 4 by 8 s (1 s apart
    // it would be 7 or more).
    assert!((3..=4).contains(&n), "{n} connections in 8 s; stderr:\n{err}");
}

/// When Linear allows one subscription, watch subscribes to updates alone,
/// straight away, stays connected, and still sees an update.
#[test]
fn one_subscription_falls_back_to_updates() {
    falls_back(Fake::start(Behavior::OneSubscription), &[]);
}

/// The 4003 comes while watch is still taking its snapshot, and waits for
/// it: how long the connection lived is when it closed, not when watch got
/// to it, so watch still falls back.
#[test]
fn one_subscription_falls_back_after_a_slow_snapshot() {
    let data = Data { snapshot: Duration::from_secs(11), ..Data::default() };
    falls_back(Fake::start_with(Behavior::OneSubscription, data), &[]);
}

/// With --no-share a watch holds a subscription of its own, as it did
/// before the hub: it falls back the same way, and no hub starts.
#[test]
fn no_share_subscribes_alone() {
    let runtime = falls_back(Fake::start(Behavior::OneSubscription), &["--no-share"]);
    assert!(!runtime.0.join("linearctl").exists(), "a hub's directory was made");
}

fn falls_back(fake: Fake, args: &[&str]) -> Runtime {
    let runtime = Runtime::new();
    let w = Watch::start_in(&fake, &runtime, args);
    w.next("ready", Duration::from_secs(20));
    let gap = w.next("gap", Duration::from_secs(5));
    assert_eq!(gap["reason"], "reconnect", "{gap}");
    // The second connection subscribes to updates only, and stays.
    let deadline = Instant::now() + Duration::from_secs(5);
    while fake.open.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "no connection kept: {:?}", fake.connections());
        std::thread::sleep(Duration::from_millis(50));
    }
    fake.push_update("id-ENG-1");
    let up = w.next("upsert", Duration::from_secs(5));
    assert_eq!(up["issue"]["identifier"], "ENG-1", "{up}");
    std::thread::sleep(Duration::from_secs(2));
    let connections = fake.connections();
    let err = w.stderr();
    assert_eq!(connections.len(), 2, "{connections:?}; stderr:\n{err}");
    assert_eq!(connections[0].len(), 2, "the first asked for more than one: {connections:?}");
    assert_eq!(connections[1], vec!["updated"], "{connections:?}");
    assert!(err.contains("Linear allows one subscription at a time"), "stderr:\n{err}");
    runtime
}

/// With issue updates alone subscribed, archives come only by catching up:
/// every one of them, not just the first 250 changed.
#[test]
fn catching_up_reports_every_archive() {
    let fake = Fake::start_with(Behavior::OneSubscription, Data { archived: 300, ..Data::default() });
    let w = Watch::start(&fake);
    let ready = w.next("ready", Duration::from_secs(10));
    assert_eq!(ready["known"], 300, "{ready}");
    w.next("gap", Duration::from_secs(5));
    let mut gone = std::collections::HashSet::new();
    while gone.len() < 300 {
        let v = w.next("remove", Duration::from_secs(5));
        assert_eq!(v["reason"], "archived", "{v}");
        gone.insert(v["id"].as_str().unwrap().to_string());
    }
    w.stderr();
}

/// Starts a watch on ENG, then one on OPS, on one key: the first starts the
/// hub, whose first connection asks for four subscriptions and falls back
/// to updates alone, as one watch of its own does. The second joins it.
#[cfg(unix)]
fn two_watches(fake: &Fake, runtime: &Runtime) -> (Watch, Watch) {
    let eng = Watch::start_in(fake, runtime, &["--team", "ENG"]);
    eng.next("ready", Duration::from_secs(10));
    eng.next("gap", Duration::from_secs(5));
    fake.settled(Duration::from_secs(5));
    let ops = Watch::start_in(fake, runtime, &["--team", "OPS"]);
    ops.next("ready", Duration::from_secs(10));
    (eng, ops)
}

/// Two watches on one key with different filters share the hub's one
/// subscription: each gets its own changes, the second neither opens a
/// connection nor is closed with 4003, and each checks each batch itself.
#[cfg(unix)]
#[test]
fn watches_of_one_key_share_one_subscription() {
    let fake = Fake::start(Behavior::OneSubscriptionPerAccount);
    let runtime = Runtime::new();
    let (eng, ops) = two_watches(&fake, &runtime);
    let checks = fake.requests("IssueList");
    let pushed = Instant::now();
    fake.push_update("id-ENG-7");
    let (at, up) = eng.next_at("upsert", Duration::from_secs(5));
    assert_eq!(up["issue"]["identifier"], "ENG-7", "{up}");
    eprintln!("ENG-7 reached its watch in {:?}", at - pushed);
    // Past the other watch's batch window, so each change is a batch.
    std::thread::sleep(Duration::from_millis(200));
    fake.push_update("id-OPS-3");
    let up = ops.next("upsert", Duration::from_secs(5));
    assert_eq!(up["issue"]["identifier"], "OPS-3", "{up}");
    std::thread::sleep(Duration::from_millis(200));
    // The next upsert ENG's watch prints is ENG-8, not OPS-3.
    fake.push_update("id-ENG-8");
    let up = eng.next("upsert", Duration::from_secs(5));
    assert_eq!(up["issue"]["identifier"], "ENG-8", "{up}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(fake.requests("IssueList") - checks, 6, "three batches, each checked by both watches");
    let connections = fake.connections();
    let too_many = fake.too_many();
    let live = fake.live();
    let (eng_err, ops_err) = (eng.stderr(), ops.stderr());
    let errs = format!("ENG stderr:\n{eng_err}\nOPS stderr:\n{ops_err}");
    // The hub's fallback, as one watch's: four asked for and closed, then
    // updates alone. Nothing for the second watch.
    assert_eq!(connections.len(), 2, "{connections:?}; {errs}");
    assert_eq!(connections[1], vec!["updated"], "{connections:?}");
    assert_eq!(too_many, 1, "{connections:?}; {errs}");
    assert_eq!(live, 1);
    assert!(!ops_err.contains("subscription lost"), "{errs}");
    // It learned from the hub that updates alone are subscribed.
    assert!(ops_err.contains("Linear allows one subscription at a time"), "{errs}");
}

/// Killing the hub is a gap for each watch; reconnecting starts a new hub,
/// and changes arrive again.
#[cfg(unix)]
#[test]
fn a_killed_hub_is_a_gap_and_the_watches_recover() {
    let fake = Fake::start(Behavior::OneSubscriptionPerAccount);
    let runtime = Runtime::new();
    let (eng, ops) = two_watches(&fake, &runtime);
    let first = runtime.hub().expect("no hub");
    kill(first);
    let killed = Instant::now();
    for w in [&eng, &ops] {
        let gap = w.next("gap", Duration::from_secs(10));
        assert_eq!(gap["reason"], "reconnect", "{gap}");
        assert_eq!(gap["mode"], "subscription", "{gap}");
    }
    runtime.hub_other_than(Some(first), Duration::from_secs(10));
    upserts_eventually(&fake, &eng, "id-ENG-9");
    upserts_eventually(&fake, &ops, "id-OPS-9");
    eprintln!("both watches had a change again {:?} after the kill", killed.elapsed());
    assert_eq!(fake.live(), 1, "{:?}", fake.connections());
    // The new hub started from the first one's fallback: updates alone, and
    // no second 4003.
    let connections = fake.connections();
    assert_eq!(fake.too_many(), 1, "{connections:?}");
    assert!(connections[1..].iter().all(|c| c == &["updated"]), "{connections:?}");
    let (eng_err, ops_err) = (eng.stderr(), ops.stderr());
    assert!(eng_err.contains("subscription lost (the hub went away)"), "ENG stderr:\n{eng_err}");
    assert!(ops_err.contains("subscription lost (the hub went away)"), "OPS stderr:\n{ops_err}");
}

/// A hub that does not say hello, stopped here, is passed by: the watch
/// subscribes alone within seconds rather than wait on it.
#[cfg(unix)]
#[test]
fn a_stuck_hub_is_passed_by() {
    let fake = Fake::start(Behavior::OneSubscription);
    let runtime = Runtime::new();
    let first = Watch::start_in(&fake, &runtime, &[]);
    first.next("ready", Duration::from_secs(10));
    let hub = runtime.hub().expect("no hub");
    // SAFETY: a signal to a process; no memory is touched.
    unsafe { libc::kill(hub, libc::SIGSTOP) };
    let started = Instant::now();
    let second = Watch::start_in(&fake, &runtime, &[]);
    second.next("ready", Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(8), "ready after {:?}", started.elapsed());
    let err = second.stderr();
    assert!(err.contains("cannot share a subscription (the hub did not answer within 3s)"), "stderr:\n{err}");
    first.stderr();
}

/// A hub removes its files when it exits, all but the fallback to updates
/// alone that the next hub starts from.
#[cfg(unix)]
#[test]
fn a_hub_leaves_only_its_fallback() {
    let fake = Fake::start(Behavior::OneSubscription);
    let runtime = Runtime::new();
    let w = Watch::start_in(&fake, &runtime, &[]);
    w.next("ready", Duration::from_secs(10));
    w.next("gap", Duration::from_secs(5));
    let hub = runtime.hub().expect("no hub");
    w.stderr();
    // The hub exits 10 s after its last watch leaves.
    let deadline = Instant::now() + Duration::from_secs(20);
    // SAFETY: signal 0 checks that the process exists.
    while unsafe { libc::kill(hub, 0) } == 0 {
        assert!(Instant::now() < deadline, "the hub is still running");
        std::thread::sleep(Duration::from_millis(100));
    }
    let left: Vec<String> = std::fs::read_dir(runtime.0.join("linearctl"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.len() == 1 && left[0].ends_with(".single"), "{left:?}");
}

/// Pushes id until w upserts it: the watch may still be reconnecting, and
/// the fake replays nothing.
#[cfg(unix)]
fn upserts_eventually(fake: &Fake, w: &Watch, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    let ident = id.strip_prefix("id-").unwrap();
    loop {
        assert!(Instant::now() < deadline, "no upsert of {ident}");
        fake.push_update(id);
        let until = Instant::now() + Duration::from_millis(500);
        while let Ok((_, v)) = w.lines.recv_timeout(until.saturating_duration_since(Instant::now())) {
            if v["event"] == "upsert" && v["issue"]["identifier"] == ident {
                return;
            }
        }
    }
}

/// Numbers for issue #6, not a check: requests per pushed change and push
/// to upsert latency with N watches on one key, shared and not.
///     cargo test --test watch measure -- --ignored --nocapture
#[cfg(unix)]
#[test]
#[ignore = "a measurement"]
fn measure_watches_on_one_key() {
    for batch in ["50", "1"] {
        eprintln!("--batch-ms {batch}:");
        for (n, share) in [(1, false), (1, true), (2, true), (3, true), (2, false)] {
            measure(n, share, batch);
        }
    }
}

#[cfg(unix)]
fn measure(n: usize, share: bool, batch: &str) {
    let fake = Fake::start(Behavior::OneSubscriptionPerAccount);
    let runtime = Runtime::new();
    let teams = ["ENG", "OPS", "DES"];
    let mut watches = vec![];
    for team in &teams[..n] {
        let mut args = vec!["--team", team, "--batch-ms", batch];
        if !share {
            args.push("--no-share");
        }
        let w = Watch::start_in(&fake, &runtime, &args);
        w.next("ready", Duration::from_secs(10));
        if watches.is_empty() {
            w.next("gap", Duration::from_secs(5));
            fake.settled(Duration::from_secs(5));
        }
        watches.push(w);
    }
    // Let a watch that lost a 4003 fight back off.
    std::thread::sleep(Duration::from_secs(3));
    let rounds = 20;
    let mut late: Vec<Vec<Duration>> = vec![vec![]; n];
    let mut per_push = vec![];
    for r in 0..rounds {
        for (i, team) in teams[..n].iter().enumerate() {
            let before = fake.requests("IssueList") + fake.requests("WatchGone");
            let pushed = Instant::now();
            fake.push_update(&format!("id-{team}-{r}"));
            let deadline = pushed + Duration::from_secs(1);
            while let Ok((at, v)) = watches[i].lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                if v["event"] == "upsert" && v["issue"]["identifier"] == format!("{team}-{r}") {
                    late[i].push(at - pushed);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(150));
            per_push.push(fake.requests("IssueList") + fake.requests("WatchGone") - before);
        }
    }
    per_push.sort();
    per_push.dedup();
    eprintln!(
        "N={n} {}: {} connections, {} closed with 4003, {} live at the end; requests per pushed change: {per_push:?}",
        if share { "shared" } else { "--no-share" },
        fake.connections().len(),
        fake.too_many(),
        fake.live(),
    );
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    for (i, l) in late.iter_mut().enumerate() {
        l.sort();
        if l.is_empty() {
            eprintln!("  {}: none of {rounds} within 1 s", teams[i]);
            continue;
        }
        let mean = l.iter().map(|d| ms(*d)).sum::<f64>() / l.len() as f64;
        eprintln!(
            "  {}: {} of {rounds} within 1 s; push to upsert ms: p50 {:.1}, mean {:.1}, max {:.1}",
            teams[i],
            l.len(),
            ms(l[l.len() / 2]),
            mean,
            ms(l[l.len() - 1])
        );
    }
    for w in watches {
        w.stderr();
    }
}
