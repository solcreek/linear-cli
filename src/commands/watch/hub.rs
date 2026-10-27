//! The hub: one background linearctl per API key and endpoint that holds
//! the key's one subscription for every watch on that key. Linear closes
//! every subscription past an account's first with 4003, so watches that
//! each held their own pushed each other off (issue #6).
//!
//! It relays and does nothing more: the id of each changed issue goes to
//! every watch, one JSON line on a Unix socket. Each watch checks its own
//! batches against its own filter, reconciles, and keeps its own known set,
//! as it does on a subscription of its own. When the subscription drops,
//! the hub tells its watches; they hang up, reconnect and replay the gap as
//! they always have, while the hub subscribes again with the same backoff,
//! falling back to issue updates alone after a 4003. It exits GRACE after
//! its last watch leaves.
//!
//! The socket is in a directory only this user can open:
//! $XDG_RUNTIME_DIR/linearctl, else linearctl-UID in the temp dir. It is
//! named after a hash of the key and the endpoint, and the protocol: a hub
//! of a build that speaks other lines has another name. The key itself only
//! ever goes to the hub on its stdin. A lock held while the hub lives keeps
//! it to one hub per name, and the hub's pid is written next to it; it
//! removes its files when it exits, all but a fallback to updates alone,
//! which the next hub on the key starts from for an hour.
//!
//! The hub's lines; a watch writes nothing:
//!   {"type":"hello","protocol":N}               at once, on connecting
//!   {"type":"ready","single":B,"lastSeen":NS}  subscribed (updates alone if single)
//!   {"type":"error","error":S}                  could not subscribe; it hangs up
//!   {"type":"id","id":S}                        an issue changed
//!   {"type":"alive","lastSeen":NS}              every PING: when Linear last spoke
//!   {"type":"failed","error":S,"code":N|null}   the subscription dropped; nothing follows

use super::{Command, Event, HEALTHY, Subscription, TOO_MANY, TOO_MANY_WITHIN, dur, subscribe};
use crate::cli::Ctx;
use crate::client::endpoint;
use crate::fail::{R, failed, usage};
use crate::json;
use crate::run::Run;
use crate::time::{self, SECOND};
use crate::util::str_at;
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// The lines a hub and its watches speak. Changing them takes a new number.
const PROTOCOL: u64 = 1;
/// The hub exits this long after its last watch leaves.
const GRACE: Duration = Duration::from_secs(10);
/// How long a watch waits for a hub's hello: one that does not say it is
/// stuck, and the watch subscribes alone.
const HELLO: Duration = Duration::from_secs(3);
/// A hub starts from a fallback to updates alone this long after one.
const REMEMBER: Duration = Duration::from_secs(3600);
/// The longest socket path: sun_path less its NUL, on macOS.
const SOCKET_PATH: usize = 103;
/// How long a watch waits for a hub it started to listen.
const STARTING: Duration = Duration::from_secs(3);
/// How long a watch waits for the hub to subscribe: the longest backoff,
/// then a connection's timeout.
const SUBSCRIBING: Duration = Duration::from_secs(75);
/// The hub pings Linear and tells its watches when Linear last spoke this
/// often: well inside the 7 s a watch with --heartbeat 1 waits.
const PING: Duration = Duration::from_secs(2);
/// It gives the subscription up after this long without a message, as a
/// watch with the default --heartbeat does.
const QUIET: i64 = 65 * SECOND;
/// A watch that does not take a line this fast is dropped.
const WRITE: Duration = Duration::from_secs(1);

/// Why a watch is not on the hub's subscription.
pub(super) enum Refused {
    /// No hub could be started or reached: subscribe alone.
    Unavailable(String),
    /// The hub could not subscribe: neither can the watch.
    Upstream(String),
}

/// The hub's files for key, without their extensions: .sock, .pid, .lock
/// and .log.
fn base(key: &str) -> Result<PathBuf, String> {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("linearctl"),
        None => std::env::temp_dir().join(format!("linearctl-{}", uid())),
    };
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    // In a directory someone else can write, the socket may not be our hub.
    let m = fs::symlink_metadata(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !m.is_dir() || m.uid() != uid() || m.mode() & 0o077 != 0 {
        return Err(format!("{} is not a directory only this user can open", dir.display()));
    }
    let mut h = sha1::Sha1::default();
    sha1::Digest::update(&mut h, format!("{key}\n{}", endpoint()).as_bytes());
    let b: [u8; 20] = sha1::Digest::finalize(h).into();
    // Short: a socket's path has at most 104 bytes on macOS.
    let hash: String = b[..8].iter().map(|x| format!("{x:02x}")).collect();
    let base = dir.join(format!("{hash}-{PROTOCOL}"));
    if base.with_extension("sock").as_os_str().len() > SOCKET_PATH {
        return Err(format!("{} is too long a path for a socket", dir.display()));
    }
    Ok(base)
}

fn uid() -> u32 {
    // SAFETY: getuid cannot fail and touches no memory.
    unsafe { libc::getuid() }
}

/// Subscribes through the key's hub, starting it if none answers.
pub(super) fn subscribe_shared(key: &str) -> Result<Subscription, Refused> {
    let base = base(key).map_err(Refused::Unavailable)?;
    let sock = base.with_extension("sock");
    // A hub that was exiting as the watch connected hangs up: start another.
    for _ in 0..3 {
        let stream = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(_) => start(key, &base).map_err(Refused::Unavailable)?,
        };
        if let Some(s) = hello(stream)? {
            return Ok(s);
        }
    }
    Err(Refused::Unavailable("the hub hung up".into()))
}

/// Starts a hub, with the key on its stdin, never in its arguments, and
/// connects to it once it listens. Two watches may each start one at once:
/// the lock lets one through, and the other exits at once with 0.
fn start(key: &str, base: &Path) -> Result<UnixStream, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let log_path = base.with_extension("log");
    let sock = base.with_extension("sock");
    let deadline = Instant::now() + STARTING;
    let mut spawns = 0;
    loop {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|e| format!("{}: {e}", log_path.display()))?;
        let mut child = std::process::Command::new(&exe)
            .arg("watch-hub")
            .env_remove("LINEAR_API_KEY")
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log)
            // Its own process group: Ctrl-C on the watch that started it
            // does not reach it.
            .process_group(0)
            .spawn()
            .map_err(|e| e.to_string())?;
        spawns += 1;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(format!("{key}\n").as_bytes());
        }
        loop {
            if let Ok(s) = UnixStream::connect(&sock) {
                // Reaped when it exits; the hub may be another's.
                std::thread::spawn(move || child.wait());
                return Ok(s);
            }
            if Instant::now() >= deadline {
                std::thread::spawn(move || child.wait());
                return Err(format!("no hub listening on {} after {STARTING:?}", sock.display()));
            }
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    return Err(format!("the hub exited ({status}); see {}", log_path.display()));
                }
                // Another hub held the lock; it may have been exiting.
                Ok(Some(_)) if spawns < 3 => break,
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }
}

/// Waits for the hub's hello and its ready, then turns the rest into the
/// events a subscription of the watch's own would give. None when the hub
/// hung up.
fn hello(mut stream: UnixStream) -> Result<Option<Subscription>, Refused> {
    let Some(m) = line(&mut stream, HELLO)
        .map_err(|_| Refused::Unavailable(format!("the hub did not answer within {HELLO:?}")))?
    else {
        return Ok(None);
    };
    if m["type"] != "hello" || m["protocol"] != PROTOCOL {
        return Err(Refused::Unavailable(format!("the hub said {}", json::marshal(&m))));
    }
    let Some(m) = line(&mut stream, SUBSCRIBING)
        .map_err(|_| Refused::Upstream(format!("the hub did not subscribe within {SUBSCRIBING:?}")))?
    else {
        return Ok(None);
    };
    let _ = stream.set_read_timeout(None);
    match m["type"].as_str() {
        Some("ready") => {}
        Some("error") => return Err(Refused::Upstream(str_at(&m, "error"))),
        _ => return Err(Refused::Unavailable(format!("the hub said {}", json::marshal(&m)))),
    }
    let last_seen = Arc::new(AtomicI64::new(m["lastSeen"].as_i64().unwrap_or_else(time::now)));
    let (events_tx, events) = mpsc::channel();
    let (commands, commands_rx) = mpsc::channel();
    let reader = stream.try_clone().map_err(|e| Refused::Unavailable(e.to_string()))?;
    let seen = last_seen.clone();
    std::thread::spawn(move || relay(reader, events_tx, seen));
    std::thread::spawn(move || hang_up(stream, commands_rx));
    Ok(Some(Subscription { events, commands, last_seen, opened: Instant::now(), single: m["single"] == json!(true) }))
}

/// One line from the hub within limit: None when it hung up, Err when it
/// did not answer or the line is not JSON. Byte by byte to the newline:
/// what follows it is the reader's.
fn line(stream: &mut UnixStream, limit: Duration) -> Result<Option<Value>, String> {
    let _ = stream.set_read_timeout(Some(limit));
    let mut line = vec![];
    let mut b = [0u8; 1];
    loop {
        match stream.read(&mut b) {
            Ok(0) => return Ok(None),
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => line.push(b[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    json::parse(&line).map(Some).map_err(|e| e.to_string())
}

fn relay(stream: UnixStream, events: Sender<Event>, seen: Arc<AtomicI64>) {
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(m) = json::parse(line.as_bytes()) else { continue };
        match m["type"].as_str() {
            Some("id") => {
                seen.store(time::now(), Ordering::Relaxed);
                if events.send(Event::Id(str_at(&m, "id"))).is_err() {
                    return;
                }
            }
            Some("alive") => {
                if let Some(t) = m["lastSeen"].as_i64() {
                    seen.store(t, Ordering::Relaxed);
                }
            }
            Some("failed") => {
                let code = m["code"].as_u64().and_then(|c| u16::try_from(c).ok());
                let _ = events.send(Event::Failed(str_at(&m, "error"), code, Instant::now()));
                return;
            }
            _ => {}
        }
    }
    let _ = events.send(Event::Failed("the hub went away".into(), None, Instant::now()));
}

/// The hub pings Linear itself; closing the subscription, or dropping it,
/// hangs up on the hub.
fn hang_up(stream: UnixStream, commands: Receiver<Command>) {
    for c in commands {
        if let Command::Close = c {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// What reaches the hub's loop.
enum Msg {
    Join(UnixStream),
    /// An event of the subscription with this number.
    Up(u64, Event),
}

/// The hub's subscription.
struct Upstream {
    n: u64,
    commands: Sender<Command>,
    last_seen: Arc<AtomicI64>,
    opened: Instant,
}

/// watch-hub: reads the key on stdin, then serves until GRACE after the
/// last watch leaves.
pub(super) fn serve(run: &mut Run, _: &Ctx) -> R {
    // A watch that leaves mid-line must not end the hub.
    // SAFETY: ignoring a signal; no handler runs.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    let mut key = String::new();
    let _ = std::io::stdin().read_line(&mut key);
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(usage("watch-hub reads the API key from stdin"));
    }
    let base = base(&key).map_err(failed)?;
    let lock_path = base.with_extension("lock");
    let lock = loop {
        let lock = private(&lock_path, false).map_err(failed)?;
        // Another hub holds it, and serves the watches.
        if lock.try_lock().is_err() {
            return Ok(());
        }
        // A hub exiting removes it before letting go: a lock on a file no
        // longer there guards nothing.
        let held = lock.metadata().ok().map(|m| (m.dev(), m.ino()));
        if fs::metadata(&lock_path).ok().map(|m| (m.dev(), m.ino())) == held {
            break lock;
        }
    };
    // This hub's log, from the start: the watch that started it opened the
    // file before a hub exiting may have removed it.
    let log_path = base.with_extension("log");
    if let Ok(log) = private(&log_path, true) {
        // SAFETY: dup2 onto stderr of a file this process owns.
        unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&log), 2) };
    }
    let marker = base.with_extension("single");
    let single = remembered(&marker);
    let sock = base.with_extension("sock");
    // A socket left by a hub that died: nothing listens on it.
    let _ = fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).map_err(|e| failed(format!("{}: {e}", sock.display())))?;
    let pid = base.with_extension("pid");
    let _ = private(&pid, true).and_then(|mut f| writeln!(f, "{}", std::process::id()).map_err(|e| e.to_string()));
    let (tx, rx) = mpsc::channel();
    let joins = tx.clone();
    std::thread::spawn(move || {
        let hello = json!({"type": "hello", "protocol": PROTOCOL});
        for s in listener.incoming().flatten() {
            // From here, at once: the hub's loop may be subscribing.
            let _ = s.set_write_timeout(Some(WRITE));
            if say(&s, &hello) && joins.send(Msg::Join(s)).is_err() {
                return;
            }
        }
    });
    let result = relay_loop(run, &key, single, &marker, tx, rx);
    let _ = fs::remove_file(&sock);
    let _ = fs::remove_file(&pid);
    // Kept to say why it failed, and the fallback while it counts.
    if result.is_ok() {
        let _ = fs::remove_file(&log_path);
    }
    if !remembered(&marker) {
        let _ = fs::remove_file(&marker);
    }
    let _ = fs::remove_file(&lock_path);
    drop(lock);
    result
}

/// A hub fell back to updates alone within REMEMBER.
fn remembered(marker: &Path) -> bool {
    fs::metadata(marker).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e < REMEMBER))
}

/// Opens a file only this user can read, truncated or not.
fn private(path: &Path, truncate: bool) -> Result<fs::File, String> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(truncate)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes one line to a watch; false, and hung up, when it is gone or stuck.
fn say(mut s: &UnixStream, v: &Value) -> bool {
    let ok = writeln!(s, "{}", json::marshal(v)).is_ok();
    if !ok {
        let _ = s.shutdown(Shutdown::Both);
    }
    ok
}

fn ready(single: bool, up: &Upstream) -> Value {
    json!({"type": "ready", "single": single, "lastSeen": up.last_seen.load(Ordering::Relaxed)})
}

fn relay_loop(run: &mut Run, key: &str, mut single: bool, marker: &Path, tx: Sender<Msg>, rx: Receiver<Msg>) -> R {
    // Watches on the subscription, watches waiting for one, and watches
    // told it dropped. Those hang up when they get to it, which may be after
    // a long snapshot: until then they are here, and the hub stays.
    let mut watches: Vec<UnixStream> = vec![];
    let mut waiting: Vec<UnixStream> = vec![];
    let mut told: Vec<UnixStream> = vec![];
    let mut up: Option<Upstream> = None;
    let mut n = 0;
    // Updates only (single), once Linear has refused more than one
    // subscription, here or to a hub before it.
    let mut backoff = SECOND;
    let mut next_attempt = Instant::now();
    // Subscribe again without waiting for a watch to ask: the subscription
    // dropped under watches that will be back.
    let mut again = false;
    let mut next_ping = Instant::now() + PING;
    // Since when no watch is here.
    let mut alone = Some(Instant::now());
    loop {
        if watches.is_empty() && waiting.is_empty() && told.is_empty() {
            let since = *alone.get_or_insert_with(Instant::now);
            if since.elapsed() >= GRACE {
                if let Some(u) = up.take() {
                    let _ = u.commands.send(Command::Close);
                }
                return Ok(());
            }
        } else {
            alone = None;
        }
        let mut deadline = next_ping;
        if up.is_none() && (again || !waiting.is_empty()) {
            deadline = deadline.min(next_attempt.max(Instant::now()));
        }
        if let Some(a) = alone {
            deadline = deadline.min(a + GRACE);
        }
        let msg = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let current = |m: u64| up.as_ref().is_some_and(|u| u.n == m);
        let mut lost = None;
        match msg {
            Some(Msg::Join(s)) => {
                let _ = s.set_write_timeout(Some(WRITE));
                match &up {
                    Some(u) => {
                        if say(&s, &ready(single, u)) {
                            watches.push(s);
                        }
                    }
                    None => waiting.push(s),
                }
            }
            Some(Msg::Up(m, Event::Id(id))) if current(m) => {
                let line = json!({"type": "id", "id": id});
                watches.retain(|s| say(s, &line));
            }
            Some(Msg::Up(m, Event::Failed(e, code, ended))) if current(m) => lost = Some((e, code, ended)),
            // An earlier subscription's.
            Some(Msg::Up(..)) | None => {}
        }
        if Instant::now() >= next_ping {
            next_ping = Instant::now() + PING;
            let last = up.as_ref().map_or(0, |u| u.last_seen.load(Ordering::Relaxed));
            let line = json!({"type": "alive", "lastSeen": last});
            // A watch that left is found by writing to it.
            told.retain(|s| say(s, &line));
            if let Some(u) = &up {
                if time::now() - last > QUIET {
                    lost = Some(("no message from Linear for too long".into(), None, Instant::now()));
                } else {
                    let _ = u.commands.send(Command::Ping);
                    watches.retain(|s| say(s, &line));
                }
            }
        }
        if let Some((e, code, ended)) = lost
            && let Some(u) = up.take()
        {
            let lived = ended.saturating_duration_since(u.opened);
            let _ = u.commands.send(Command::Close);
            let limited = code == Some(TOO_MANY) && lived < TOO_MANY_WITHIN && !single;
            if limited {
                single = true;
                // Its mtime says when: the next hub starts from it.
                let _ =
                    private(marker, true).and_then(|mut f| writeln!(f, "{}", time::now()).map_err(|e| e.to_string()));
                run.eprint("linearctl: Linear allows one subscription at a time; subscribing to issue updates\n");
            } else {
                run.eprint(&format!("linearctl: subscription lost ({e}); reconnecting\n"));
            }
            if lived >= HEALTHY {
                backoff = SECOND;
            }
            // Each watch reconnects, and replays what it missed meanwhile.
            let line = json!({"type": "failed", "error": e, "code": code});
            told.extend(watches.drain(..).filter(|s| say(s, &line)));
            // Straight back after switching to one subscription.
            again = true;
            next_attempt = Instant::now();
            if !limited {
                next_attempt += dur(backoff);
                backoff = (2 * backoff).min(30 * SECOND);
            }
        }
        // A hub subscribes when a watch first asks: one that joins later
        // than that sees the 4003 and its fallback as a watch of its own does.
        if up.is_none() && (again || !waiting.is_empty()) && Instant::now() >= next_attempt {
            match subscribe(run, key, single) {
                Ok(s) => {
                    n += 1;
                    let Subscription { events, commands, last_seen, opened, .. } = s;
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        for e in events {
                            if tx.send(Msg::Up(n, e)).is_err() {
                                return;
                            }
                        }
                    });
                    let u = Upstream { n, commands, last_seen, opened };
                    let line = ready(single, &u);
                    watches.extend(waiting.drain(..).filter(|s| say(s, &line)));
                    up = Some(u);
                    again = false;
                    next_ping = Instant::now() + PING;
                }
                Err(e) => {
                    run.eprint(&format!("linearctl: cannot subscribe ({e})\n"));
                    let line = json!({"type": "error", "error": e});
                    for s in waiting.drain(..) {
                        say(&s, &line);
                        let _ = s.shutdown(Shutdown::Both);
                    }
                    next_attempt = Instant::now() + dur(backoff);
                    backoff = (2 * backoff).min(30 * SECOND);
                }
            }
        }
    }
}
