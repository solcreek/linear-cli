use crate::cli::{Cmd, Ctx, Flag};
use crate::fail::{Fail, R, failed, usage};
use crate::json;
use crate::keys::{SERVED, api_key};
use crate::run::{Run, Sink, execute};
use crate::util::with_request_stdin;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, mpsc};

/// The most requests --concurrency runs at once.
const MAX_CONCURRENCY: u64 = 32;

pub fn command() -> Cmd {
    Cmd::new("serve", "Answer linearctl invocations over stdin and stdout, one JSON line each")
        .long(
            "Answer linearctl invocations over stdin and stdout, for a program that makes many\n\
calls (a TUI, a long agent session). One process serves them all: no start-up, no\n\
new TLS connection, no Keychain read per call.\n\n\
Each line on stdin is a request:   {\"id\": ANY, \"argv\": [\"issue\", \"list\", \"--json\"],\n                                    \
\"stdin\": TEXT (optional)}\n\
Each line on stdout answers one:   {\"id\": ANY, \"exit\": N, \"result\": JSON|null,\n                                    \
\"stdout\": TEXT|null, \"stderr\": TEXT}\n\
argv is what follows linearctl on a command line; exit and the JSON in result are\n\
exactly what that command line would give (result is null when it printed text,\n\
which is then in stdout). stdin is that command's standard input, so - as a file\n\
(--json-input=-, --body-file=-) reads it; \"\" is an empty one. Without stdin, - is\n\
refused: serve's own stdin carries requests. A malformed line is answered with\n\
exit 2; watch and serve are refused.\n\n\
Requests run one at a time, in order. With --concurrency N, up to N run at once\n\
and answers come as each finishes: match them by id. They share one key, one\n\
connection pool and Linear's one rate budget; a request Linear limits answers\n\
exit 6 like any other. Nothing orders two writes: send the second after the\n\
first one's answer when it must come after. serve ends when stdin closes and\n\
every request has its answer (exit 0).",
        )
        .example("  printf '{\"id\":1,\"argv\":[\"issue\",\"list\",\"--json\"]}\\n' | linearctl serve --stdio")
        .run(serve)
        .flag(Flag::bool("stdio", "read requests on stdin, answer on stdout"))
        .flag(Flag::whole("concurrency", 1, "run up to `N` requests at once (at most 32), out of order"))
}

/// What the reader and the workers send to the thread that answers.
enum Msg {
    Answer(Value),
    /// Requests can no longer be read; answer those already read, then end.
    Stop(Fail),
}

/// Reads request lines from stdin, giving each non-blank one to send. A
/// read that fails, or a line over 16 MiB, ends the requests.
fn read_requests(mut send: impl FnMut(Vec<u8>) -> bool) -> R {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    loop {
        let mut line = Vec::new();
        let n = lines.read_until(b'\n', &mut line).map_err(|e| failed(format!("cannot read requests: {e}")))?;
        if n == 0 {
            return Ok(());
        }
        if line.len() > 16 << 20 {
            return Err(failed("cannot read requests: bufio.Scanner: token too long"));
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if line.ends_with(b"\n") {
            line.pop();
        }
        if line.ends_with(b"\r") {
            line.pop();
        }
        if !send(line) {
            return Ok(());
        }
    }
}

/// Writes one answer line to stdout.
fn answer(run: &mut Run, a: &Value) -> R {
    run.out
        .write_all(json::encode(a, false, &[]).as_bytes())
        .and_then(|()| run.out.flush())
        .map_err(|e| failed(format!("cannot answer on stdout: {}", crate::state::cause(&e))))
}

fn serve(run: &mut Run, ctx: &Ctx) -> R {
    if !ctx.bool("stdio") {
        return Err(usage("pass --stdio (the only transport so far)"));
    }
    let workers = ctx.whole("concurrency");
    if !(1..=MAX_CONCURRENCY).contains(&workers) {
        return Err(usage(format!("--concurrency must be 1 to {MAX_CONCURRENCY}; got {workers}")));
    }
    if let Ok((key, source)) = api_key()
        && !key.is_empty()
    {
        SERVED.with(|s| *s.borrow_mut() = Some((key, source)));
    }
    if workers == 1 {
        let mut failure = Ok(());
        read_requests(|line| {
            failure = answer(run, &serve_one(&line));
            failure.is_ok()
        })?;
        return failure;
    }
    // Each worker runs requests as the single-threaded loop does; the key
    // serve read is copied to each, as SERVED is per thread.
    let key = SERVED.with(|s| s.borrow().clone());
    let (jobs, queue) = mpsc::channel::<Vec<u8>>();
    let queue = Arc::new(Mutex::new(queue));
    let (tell, answers) = mpsc::channel::<Msg>();
    for _ in 0..workers {
        let (queue, tell, key) = (queue.clone(), tell.clone(), key.clone());
        std::thread::spawn(move || {
            SERVED.with(|s| *s.borrow_mut() = key);
            loop {
                let line = match queue.lock() {
                    Ok(q) => q.recv(),
                    Err(_) => return,
                };
                let Ok(line) = line else { return };
                if tell.send(Msg::Answer(serve_one(&line))).is_err() {
                    return;
                }
            }
        });
    }
    std::thread::spawn(move || {
        if let Err(e) = read_requests(|line| jobs.send(line).is_ok()) {
            let _ = tell.send(Msg::Stop(e));
        }
    });
    // Every sender gone: stdin closed and each request read is answered.
    let mut stop = Ok(());
    for msg in answers {
        match msg {
            Msg::Answer(a) => answer(run, &a)?,
            Msg::Stop(e) => stop = Err(e),
        }
    }
    stop
}

/// Runs one request line and returns its answer.
fn serve_one(line: &[u8]) -> Value {
    let refuse = |id: Value, msg: &str| {
        json!({"id": id, "exit": crate::fail::USAGE, "result": {"error": msg, "usage": true, "code": "USAGE"}, "stdout": null,
            "stderr": format!("linearctl: {msg}\n")})
    };
    let malformed = "a request is one JSON object: {\"id\": ..., \"argv\": [...]}";
    let Ok(v) = json::parse(line) else { return refuse(Value::Null, malformed) };
    // Go's decoder: keys match ignoring case, argv must be strings.
    let (mut id, mut argv, mut stdin) = (Value::Null, Vec::<String>::new(), None);
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k.eq_ignore_ascii_case("id") {
                    id = x;
                } else if k.eq_ignore_ascii_case("argv") {
                    match x {
                        Value::Null => argv.clear(),
                        Value::Array(a) => {
                            let mut out = vec![];
                            for s in a {
                                match s {
                                    Value::String(s) => out.push(s),
                                    Value::Null => out.push(String::new()),
                                    _ => return refuse(Value::Null, malformed),
                                }
                            }
                            argv = out;
                        }
                        _ => return refuse(Value::Null, malformed),
                    }
                } else if k.eq_ignore_ascii_case("stdin") {
                    match x {
                        Value::Null => stdin = None,
                        Value::String(t) => stdin = Some(t.into_bytes()),
                        _ => return refuse(Value::Null, malformed),
                    }
                }
            }
        }
        Value::Null => {}
        _ => return refuse(Value::Null, malformed),
    }
    if argv.is_empty() {
        return refuse(id, "argv is empty");
    }
    if argv[0] == "serve" || argv[0] == "watch" {
        return refuse(id, &format!("{} cannot run inside serve; run it as its own process", argv[0]));
    }
    if stdin.is_none() && argv.iter().any(|a| a == "-" || a.ends_with("=-")) {
        return refuse(id, "- (stdin) is serve's own input; give the request a \"stdin\", or pass the text in argv");
    }
    let (code, out, err) = with_request_stdin(stdin, || execute(&argv, Sink::Buf(vec![]), Sink::Buf(vec![])));
    let (out, err) = (out.into_buf(), err.into_buf());
    let mut answer = json!({"id": id, "exit": code, "result": null, "stdout": null,
        "stderr": String::from_utf8_lossy(&err)});
    if let Ok(result) = json::parse(&out) {
        answer["result"] = result;
    } else if !out.is_empty() {
        answer["stdout"] = json!(String::from_utf8_lossy(&out));
    }
    answer
}
