//! watch against a fake Linear whose subscriptions misbehave as the real
//! one has: closing every connection right after it subscribes, or closing
//! with 4003 any connection that asks for more than one subscription.

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
}

/// What the fake saw.
#[derive(Default)]
struct Seen {
    /// The subscription ids of each connection, in order.
    connections: Vec<Vec<String>>,
}

struct Fake {
    url: String,
    seen: Arc<Mutex<Seen>>,
    /// Sockets still open, to push a change down.
    open: Arc<Mutex<Vec<tungstenite::WebSocket<TcpStream>>>>,
}

impl Fake {
    fn start(behavior: Behavior) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Seen::default()));
        let open = Arc::new(Mutex::new(vec![]));
        let (s, o) = (seen.clone(), open.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, o) = (s.clone(), o.clone());
                std::thread::spawn(move || serve(stream, behavior, s, o));
            }
        });
        Fake { url, seen, open }
    }

    fn connections(&self) -> Vec<Vec<String>> {
        self.seen.lock().unwrap().connections.clone()
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
    seen: Arc<Mutex<Seen>>,
    open: Arc<Mutex<Vec<tungstenite::WebSocket<TcpStream>>>>,
) {
    let mut head = [0u8; 2048];
    let n = stream.peek(&mut head).unwrap_or(0);
    if String::from_utf8_lossy(&head[..n]).to_ascii_lowercase().contains("upgrade: websocket") {
        websocket(stream, behavior, seen, open);
    } else {
        graphql(stream);
    }
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
    let close = |ws: &mut tungstenite::WebSocket<TcpStream>, code: u16| {
        let _ = ws.close(Some(CloseFrame { code: CloseCode::from(code), reason: "".into() }));
        let _ = ws.flush();
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
                if matches!(behavior, Behavior::OneSubscription) && seen.lock().unwrap().connections[index].len() == 1 {
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
                if matches!(behavior, Behavior::OneSubscription) && subs > 1 {
                    close(&mut ws, 4003);
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Answers watch's queries: no issues match at start; an issue asked for by
/// id is there and matches.
fn graphql(stream: TcpStream) {
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
        let nodes: Vec<Value> = match op {
            "IssueList" => {
                let ids = &req["variables"]["filter"]["and"][1]["id"]["in"];
                let ids = if ids.is_array() { ids } else { &req["variables"]["filter"]["id"]["in"] };
                ids.as_array()
                    .into_iter()
                    .flatten()
                    .map(|id| json!({"id": id, "identifier": "ENG-1", "title": "One", "state": {"name": "Todo"}}))
                    .collect()
            }
            _ => vec![],
        };
        let data = match op {
            "WatchGone" | "WatchChanged" => json!({"issues": {"nodes": nodes}}),
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

/// A running watch: its JSON lines, and its stderr when it ends.
struct Watch {
    child: Child,
    lines: Receiver<Value>,
}

impl Watch {
    fn start(fake: &Fake) -> Watch {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let home = std::env::temp_dir().join(format!("linearctl-watch-{}-{nanos}", std::process::id()));
        let mut child = Command::new(BIN)
            .args(["watch", "--json", "--batch-ms", "50", "--heartbeat", "60"])
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
                if tx.send(serde_json::from_str(&line).unwrap_or(Value::String(line))).is_err() {
                    return;
                }
            }
        });
        Watch { child, lines }
    }

    fn next(&self, event: &str, limit: Duration) -> Value {
        let deadline = Instant::now() + limit;
        loop {
            let v = self.lines.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            match v {
                Ok(v) if v["event"] == event => return v,
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
    let fake = Fake::start(Behavior::OneSubscription);
    let w = Watch::start(&fake);
    w.next("ready", Duration::from_secs(10));
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
}
