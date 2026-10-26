//! watch streams changes to the issues a filter matches, one JSON document
//! per line. Linear's GraphQL subscriptions (WebSocket, graphql-transport-ws,
//! undocumented but working with a personal API key, verified 2026-10-26)
//! say which issues changed; each batch of changed ids is then checked
//! against the filter with one issue list request, so a change that makes an
//! issue stop matching is reported as a remove. When subscriptions are not
//! available it polls instead.

use super::issue::{filter_flags, list_filter, nodes, when};
use crate::cli::{Cmd, Ctx, Flag};
use crate::client::{Client, endpoint};
use crate::fail::{R, failed, usage};
use crate::json::{self, quote_go};
use crate::queries::{self, ISSUE_ROW_FIELDS};
use crate::run::{Run, VERSION};
use crate::time::{self, SECOND};
use crate::util::*;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Known ids at start: up to 2,500 issues.
const SNAPSHOT_PAGES: u64 = 10;
const RECONCILE_SKEW: i64 = 5 * SECOND;

pub fn command() -> Cmd {
    let c = Cmd::new("watch", "Stream changes to the issues a filter matches, one JSON line each")
        .long(
            "Stream changes to the issues a filter matches. With --json each line is one event:\n  \
ready      {mode, since, known, everything}: up to date from since\n  \
upsert     {issue}: an issue that matches, in issue list's row shape\n  \
remove     {id, identifier, reason}: it stopped matching; reason is filter,\n             \
archived or deleted\n  \
heartbeat  {at, mode, rateLimit}: every --heartbeat seconds\n  \
gap        {from, to, reason}: the stream was down (reconnect) or fell back\n             \
to polling (fallback); what changed meanwhile follows as upserts\n\
It runs until interrupted (exit 130) or a request fails, which ends it with the\n\
usual exit codes (5 auth, 6 rate-limited, 1 failed) and a last {error} line.\n\n\
Linear's subscriptions name the changed issues; linearctl checks each batch\n\
against the filter with one request (at most one per 2 s) and reconciles every\n\
--reconcile seconds. --mode poll asks Linear every --poll-interval seconds instead.\n\
Removes are reported for issues known to match: up to 2,500 found at start, and\n\
any since; beyond that (everything: true) every non-matching change is a remove.",
        )
        .example(
            "  linearctl watch --team ENG --json\n  linearctl watch --assignee me --state-type started --since 2026-10-26T09:00:00Z --json",
        )
        .run(watch);
    let c = filter_flags(c)
        .flag(Flag::str("since", "first replay changes since `WHEN` (a date, RFC 3339 time, 7d, 24h)"))
        .flag(Flag::str("mode", "`MODE`: auto (subscribe, else poll), subscription, poll").default_to("auto"))
        .flag(Flag::whole("heartbeat", 30, "print a heartbeat every `N` seconds"))
        .flag(Flag::whole("reconcile", 300, "with subscriptions, catch up on missed changes every `N` s"))
        .flag(Flag::whole("poll-interval", 60, "when polling, ask Linear every `N` seconds"))
        // For tests; the default protects the rate limit.
        .flag(Flag::whole("batch-ms", 2000, "check changed issues at most every `MS` milliseconds").hidden());
    fields_flag_spec(c, ISSUE_ROW_FIELDS)
}

fn watch(run: &mut Run, ctx: &Ctx) -> R {
    let filter = list_filter(ctx)?;
    let fields = fields_flag(ctx, ISSUE_ROW_FIELDS)?;
    let mode = ctx.str("mode");
    if !["auto", "subscription", "poll"].contains(&mode.as_str()) {
        return Err(usage(format!("--mode must be auto, subscription or poll; got {}", quote_go(&mode))));
    }
    let mut since = None;
    let s = ctx.str("since");
    if !s.is_empty() {
        let t = when(&s).map_err(|e| {
            let msg = e.to_string();
            usage(format!("--since: {}", msg.strip_prefix("--updated-since ").unwrap_or(&msg)))
        })?;
        since = time::parse_rfc3339(&t);
    }
    let seconds = |name: &str, min: u64| -> R<i64> {
        let n = ctx.whole(name);
        if n < min {
            return Err(usage(format!("--{name} is at least {min}")));
        }
        Ok(n as i64 * SECOND)
    };
    let heartbeat = seconds("heartbeat", 1)?;
    let reconcile = seconds("reconcile", 10)?;
    let poll = seconds("poll-interval", 10)?;
    let batch = ctx.whole("batch-ms") as i64 * 1_000_000;
    let cl = run.client()?;
    let mut w = Watcher {
        cl,
        filter: filter.map(Value::Object),
        fields,
        known: HashMap::new(),
        everything: false,
        mode: String::new(),
        since: 0,
    };
    w.run(run, &mode, since, heartbeat, reconcile, poll, batch)
}

struct Watcher {
    cl: std::rc::Rc<Client>,
    /// None: every issue.
    filter: Option<Value>,
    fields: Vec<String>,
    /// Issues known to match: id → identifier.
    known: HashMap<String, String>,
    /// The snapshot was cut short: report every non-match.
    everything: bool,
    mode: String,
    /// Changes from here on (Unix ns) are covered.
    since: i64,
}

fn dur(ns: i64) -> Duration {
    Duration::from_nanos(ns.max(0) as u64)
}

impl Watcher {
    /// Prints one event: a JSON line with --json, else a line of text.
    fn emit(&self, run: &mut Run, event: &str, mut doc: Map<String, Value>) -> R {
        doc.insert("event".into(), json!(event));
        let doc = Value::Object(doc);
        let line = if run.json {
            json::marshal(&doc)
        } else {
            match event {
                "upsert" => {
                    let it = &doc["issue"];
                    format!(
                        "upsert {}\t{}\t{}",
                        str_at(it, "identifier"),
                        printable(&str_at(it, "state.name")),
                        printable(&str_at(it, "title"))
                    )
                }
                "remove" => {
                    let ident = match &doc["identifier"] {
                        Value::String(s) => s.clone(),
                        _ => "%!s(<nil>)".into(),
                    };
                    format!("remove {ident} ({})", str_at(&doc, "reason"))
                }
                _ => format!("{event} {}", json::marshal(&doc)),
            }
        };
        run.println(&line)?;
        let _ = run.out.flush();
        Ok(())
    }

    fn upsert(&mut self, run: &mut Run, it: &Value) -> R {
        self.known.insert(str_at(it, "id"), str_at(it, "identifier"));
        let mut doc = Map::new();
        doc.insert("issue".into(), pick(flatten(it.clone()), &self.fields));
        self.emit(run, "upsert", doc)
    }

    /// The watcher's filter with one more condition.
    fn and(&self, cond: Value) -> Value {
        match &self.filter {
            None => cond,
            Some(f) => json!({"and": [f, cond]}),
        }
    }

    /// Learns which issues match now, to know what a remove is.
    fn snapshot(&mut self, run: &mut Run) -> R {
        let mut cursor = String::new();
        for _ in 0..SNAPSHOT_PAGES {
            let vars = json!({"filter": self.filter, "first": PAGE_SIZE, "after": optional(&cursor)});
            let data = self.cl.query(run, queries::WATCH_IDS, vars)?;
            let p = Page::from(&data["issues"]);
            for n in &p.nodes {
                self.known.insert(str_at(n, "id"), str_at(n, "identifier"));
            }
            if !p.has_next {
                return Ok(());
            }
            cursor = p.end_cursor;
        }
        self.everything = true;
        Ok(())
    }

    /// Reports issues that no longer match, with why.
    fn gone(&mut self, run: &mut Run, ids: &[String]) -> R {
        let report: Vec<&String> = ids.iter().filter(|id| self.known.contains_key(*id) || self.everything).collect();
        if report.is_empty() {
            return Ok(());
        }
        let data = self.cl.query(run, queries::WATCH_GONE, json!({"ids": report}))?;
        let found = nodes(&data["issues"]);
        for id in report {
            let (mut reason, mut ident) = ("deleted", self.known.get(id).cloned().unwrap_or_default());
            for n in found.iter().filter(|n| str_at(n, "id") == *id) {
                ident = str_at(n, "identifier");
                reason = if n["trashed"] == json!(true) {
                    "deleted"
                } else if !n["archivedAt"].is_null() {
                    "archived"
                } else {
                    "filter"
                };
            }
            self.known.remove(id);
            let mut doc = Map::new();
            doc.insert("id".into(), json!(id));
            doc.insert("identifier".into(), optional(&ident));
            doc.insert("reason".into(), json!(reason));
            self.emit(run, "remove", doc)?;
        }
        Ok(())
    }

    /// One request for a batch of changed ids: those that match are
    /// upserts, the rest may be removes.
    fn check(&mut self, run: &mut Run, ids: &[String]) -> R {
        let vars = json!({"filter": self.and(json!({"id": {"in": ids}})), "first": PAGE_SIZE});
        let data = self.cl.query(run, queries::ISSUE_LIST, vars)?;
        let items = nodes(&data["issues"]);
        let mut matched = std::collections::HashSet::new();
        for it in &items {
            matched.insert(str_at(it, "id"));
            self.upsert(run, it)?;
        }
        let rest: Vec<String> = ids.iter().filter(|id| !matched.contains(*id)).cloned().collect();
        self.gone(run, &rest)
    }

    /// Catches up on everything updated since from: what matches is
    /// upserted, what changed and no longer matches is removed.
    fn reconcile(&mut self, run: &mut Run, from: i64) -> R {
        let started = time::now();
        let gte = json!({"updatedAt": {"gte": time::rfc3339(from)}});
        let filter = self.and(gte.clone());
        let cl = self.cl.clone();
        let (items, _) = collect(PAGE_SIZE * SNAPSHOT_PAGES, String::new(), |first, after| {
            let data = cl.query(run, queries::ISSUE_LIST, json!({"filter": filter, "first": first, "after": optional(after)}))?;
            Ok(Page::from(&data["issues"]))
        })?;
        let mut matched = std::collections::HashSet::new();
        for it in &items {
            matched.insert(str_at(it, "id"));
            self.upsert(run, it)?;
        }
        let changed = self.cl.query(run, queries::WATCH_CHANGED, json!({"filter": gte}))?;
        let rest: Vec<String> =
            nodes(&changed["issues"]).iter().map(|n| str_at(n, "id")).filter(|id| !matched.contains(id)).collect();
        self.gone(run, &rest)?;
        self.since = started - RECONCILE_SKEW;
        Ok(())
    }

    fn heartbeat(&self, run: &mut Run) -> R {
        let mut doc = Map::new();
        doc.insert("at".into(), json!(time::rfc3339(time::now())));
        doc.insert("mode".into(), json!(self.mode));
        doc.insert("rateLimit".into(), if self.cl.has_header() { self.cl.rate_limit() } else { Value::Null });
        self.emit(run, "heartbeat", doc)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, run: &mut Run, mode: &str, since: Option<i64>, heartbeat: i64, reconcile: i64, poll: i64, batch: i64) -> R {
        let start = time::now();
        let mut sub = None;
        if mode != "poll" {
            match subscribe(run, &self.cl.key) {
                Ok(s) => sub = Some(s),
                Err(e) if mode == "subscription" => return Err(failed(e)),
                Err(e) => run.eprint(&format!("linearctl: subscriptions unavailable ({e}); polling every {}\n", time::duration(poll))),
            }
        }
        self.mode = if sub.is_some() { "subscription" } else { "poll" }.into();
        self.snapshot(run)?;
        self.since = start - RECONCILE_SKEW;
        if let Some(s) = since {
            self.reconcile(run, s)?;
            self.since = start - RECONCILE_SKEW;
        }
        let mut doc = Map::new();
        doc.insert("mode".into(), json!(self.mode));
        doc.insert("since".into(), json!(time::rfc3339(self.since)));
        doc.insert("known".into(), json!(self.known.len()));
        doc.insert("everything".into(), json!(self.everything));
        self.emit(run, "ready", doc)?;

        let now = Instant::now();
        let mut next_beat = now + dur(heartbeat);
        let mut every = if sub.is_some() { reconcile } else { poll };
        let mut next_catch_up = now + dur(every);
        let mut flush: Option<Instant> = None;
        let mut pending: Vec<String> = vec![];
        let mut failed_now: Option<String> = None;
        let mut backoff = SECOND;
        loop {
            let event = if let Some(e) = failed_now.take() {
                Some(Event::Failed(e))
            } else {
                let mut deadline = next_beat.min(next_catch_up);
                if let Some(f) = flush {
                    deadline = deadline.min(f);
                }
                let wait = deadline.saturating_duration_since(Instant::now());
                match &sub {
                    Some(s) => match s.events.recv_timeout(wait) {
                        Ok(e) => Some(e),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => Some(Event::Failed("the stream ended".into())),
                    },
                    None => {
                        std::thread::sleep(wait);
                        None
                    }
                }
            };
            match event {
                Some(Event::Id(id)) => {
                    if !pending.contains(&id) {
                        pending.push(id);
                    }
                    if pending.len() == 1 {
                        flush = Some(Instant::now() + dur(batch));
                    }
                    if pending.len() as u64 >= PAGE_SIZE {
                        flush = Some(Instant::now());
                    }
                    continue;
                }
                Some(Event::Failed(e)) => {
                    // The stream is down: reconnect, then replay the gap.
                    let down = time::now();
                    run.eprint(&format!("linearctl: subscription lost ({e}); reconnecting\n"));
                    if let Some(s) = sub.take() {
                        s.close();
                    }
                    for _ in 0..5 {
                        std::thread::sleep(dur(backoff));
                        backoff = (2 * backoff).min(30 * SECOND);
                        if let Ok(s) = subscribe(run, &self.cl.key) {
                            sub = Some(s);
                            break;
                        }
                    }
                    let reason = if sub.is_none() {
                        self.mode = "poll".into();
                        every = poll;
                        next_catch_up = Instant::now() + dur(every);
                        "fallback"
                    } else {
                        backoff = SECOND;
                        "reconnect"
                    };
                    let to = time::now();
                    let mut doc = Map::new();
                    doc.insert("from".into(), json!(time::rfc3339(self.since)));
                    doc.insert("to".into(), json!(time::rfc3339(to)));
                    doc.insert("reason".into(), json!(reason));
                    doc.insert("mode".into(), json!(self.mode));
                    doc.insert("downFor".into(), json!(time::duration(time::now() - down)));
                    self.emit(run, "gap", doc)?;
                    let from = self.since;
                    self.reconcile(run, from)?;
                    continue;
                }
                None => {}
            }
            let now = Instant::now();
            if flush.is_some_and(|f| now >= f) {
                flush = None;
                if !pending.is_empty() {
                    let ids = std::mem::take(&mut pending);
                    self.check(run, &ids)?;
                }
            }
            if now >= next_catch_up {
                next_catch_up = tick(next_catch_up, every);
                let from = self.since;
                self.reconcile(run, from)?;
            }
            if Instant::now() >= next_beat {
                next_beat = tick(next_beat, heartbeat);
                if let Some(s) = &sub {
                    let quiet = time::now() - s.last_seen.load(Ordering::Relaxed);
                    if quiet > 2 * heartbeat + 5 * SECOND {
                        failed_now = Some("no message from Linear for too long".into());
                        continue;
                    }
                    s.ping();
                }
                self.heartbeat(run)?;
            }
        }
    }
}

/// A ticker's next tick: one period on, skipping ticks already missed.
fn tick(prev: Instant, every: i64) -> Instant {
    let mut next = prev + dur(every);
    let now = Instant::now();
    while next <= now {
        next += dur(every);
    }
    next
}

enum Event {
    /// The id of an issue that changed.
    Id(String),
    Failed(String),
}

enum Command {
    Ping,
    Close,
}

/// A graphql-transport-ws connection subscribed to issue changes.
struct Subscription {
    events: Receiver<Event>,
    commands: Sender<Command>,
    /// When a message last came, in Unix ns.
    last_seen: Arc<AtomicI64>,
}

impl Subscription {
    fn ping(&self) {
        let _ = self.commands.send(Command::Ping);
    }
    fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
}

/// The GraphQL endpoint's WebSocket form.
fn ws_url() -> String {
    let u = endpoint();
    match u.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => format!("ws://{}", u.strip_prefix("http://").unwrap_or(&u)),
    }
}

/// A plain or TLS connection.
enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(s) => s.get_ref(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(b),
            Stream::Tls(s) => s.read(b),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(b),
            Stream::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

fn connect(url: &str, timeout: Duration) -> Result<Stream, String> {
    let (tls, rest) = match url.split_once("://") {
        Some(("wss", r)) => (true, r),
        Some((_, r)) => (false, r),
        None => (false, url),
    };
    let host_port = rest.split('/').next().unwrap_or("");
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h.trim_matches(['[', ']']), p.parse().map_err(|_| format!("bad port in {url}"))?),
        _ => (host_port, if tls { 443 } else { 80 }),
    };
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|e| e.to_string())?.collect();
    let mut last = format!("cannot resolve {host}");
    for addr in addrs {
        let tcp = match TcpStream::connect_timeout(&addr, timeout) {
            Ok(t) => t,
            Err(e) => {
                last = e.to_string();
                continue;
            }
        };
        let _ = tcp.set_nodelay(true);
        if !tls {
            return Ok(Stream::Plain(tcp));
        }
        let config = tls_config()?;
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let conn = rustls::ClientConnection::new(config, name).map_err(|e| e.to_string())?;
        return Ok(Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))));
    }
    Err(last)
}

/// TLS as the HTTP client has it: ring, the platform's certificates.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone()).map_err(|e| e.to_string())?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

type Socket = tungstenite::WebSocket<Stream>;

fn send(ws: &mut Socket, v: &Value) -> Result<(), String> {
    ws.send(tungstenite::Message::text(json::marshal(v))).map_err(|e| e.to_string())
}

/// Opens a subscription to issue changes. The key goes in the
/// Authorization header of the upgrade: Linear rejects it in
/// connection_init.
fn subscribe(run: &Run, key: &str) -> Result<Subscription, String> {
    if run.demo {
        return Err("demo mode has no live changes".into());
    }
    let url = ws_url();
    let timeout = if run.timeout > 0 { Duration::from_secs(run.timeout) } else { Duration::from_secs(3600) };
    let stream = connect(&url, timeout)?;
    let _ = stream.tcp().set_read_timeout(Some(timeout));
    let _ = stream.tcp().set_write_timeout(Some(timeout));
    let host = url.split_once("://").map_or("", |(_, r)| r.split('/').next().unwrap_or(""));
    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
        .header("Sec-WebSocket-Protocol", "graphql-transport-ws")
        .header("Authorization", key)
        .header("User-Agent", format!("linearctl/{VERSION}"))
        .body(())
        .map_err(|e| e.to_string())?;
    let config = tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(1 << 20));
    let (mut ws, _) = tungstenite::client::client_with_config(request, stream, Some(config)).map_err(|e| e.to_string())?;
    send(&mut ws, &json!({"type": "connection_init", "payload": {}}))?;
    let acked = match ws.read() {
        Ok(tungstenite::Message::Text(t)) => json::parse(t.as_bytes()).is_ok_and(|v| v["type"] == "connection_ack"),
        _ => false,
    };
    if !acked {
        return Err(format!("no connection_ack from {url}"));
    }
    for (id, doc) in [
        ("created", queries::WATCH_CREATED),
        ("updated", queries::WATCH_UPDATED),
        ("archived", queries::WATCH_ARCHIVED),
        ("unarchived", queries::WATCH_UNARCHIVED),
    ] {
        send(&mut ws, &json!({"id": id, "type": "subscribe", "payload": {"query": doc}}))?;
    }
    // Reads time out often, so the reader can also send.
    let _ = ws.get_ref().tcp().set_read_timeout(Some(Duration::from_millis(100)));
    let (events_tx, events) = mpsc::channel();
    let (commands, commands_rx) = mpsc::channel();
    let last_seen = Arc::new(AtomicI64::new(time::now()));
    let seen = last_seen.clone();
    std::thread::spawn(move || read(ws, events_tx, commands_rx, seen));
    Ok(Subscription { events, commands, last_seen })
}

fn read(mut ws: Socket, events: Sender<Event>, commands: Receiver<Command>, seen: Arc<AtomicI64>) {
    loop {
        while let Ok(c) = commands.try_recv() {
            match c {
                Command::Ping => {
                    let _ = send(&mut ws, &json!({"type": "ping"}));
                }
                Command::Close => {
                    let _ = ws.get_ref().tcp().shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        }
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(e) => {
                let _ = events.send(Event::Failed(e.to_string()));
                return;
            }
        };
        seen.store(time::now(), Ordering::Relaxed);
        let text = match msg {
            tungstenite::Message::Text(t) => t.to_string(),
            tungstenite::Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            tungstenite::Message::Close(f) => {
                let why = f.map(|f| format!("status = {}, reason = {}", u16::from(f.code), f.reason)).unwrap_or_default();
                let _ = events.send(Event::Failed(format!("closed: {why}")));
                return;
            }
            _ => continue,
        };
        let Ok(m) = json::parse(text.as_bytes()) else { continue };
        match m["type"].as_str() {
            Some("next") => {
                if let Some(data) = m["payload"]["data"].as_object() {
                    for v in data.values() {
                        let id = str_at(v, "id");
                        if !id.is_empty() && events.send(Event::Id(id)).is_err() {
                            return;
                        }
                    }
                }
            }
            Some("ping") => {
                let _ = send(&mut ws, &json!({"type": "pong"}));
            }
            Some(kind @ ("error" | "complete")) => {
                let payload = m.get("payload").map(json::marshal).unwrap_or_default();
                let id = str_at(&m, "id");
                let _ = events.send(Event::Failed(format!("subscription {id} ended: {kind} {}", printable(&payload))));
                let _ = ws.get_ref().tcp().shutdown(std::net::Shutdown::Both);
                return;
            }
            _ => {}
        }
    }
}
