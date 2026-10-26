use crate::cli::{Cmd, Ctx, Flag};
use crate::fail::{R, failed, usage};
use crate::json;
use crate::keys::{SERVED, api_key};
use crate::run::{Run, Sink, execute};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

pub fn command() -> Cmd {
    Cmd::new("serve", "Answer linearctl invocations over stdin and stdout, one JSON line each")
        .long(
            "Answer linearctl invocations over stdin and stdout, for a program that makes many\n\
calls (a TUI, a long agent session). One process serves them all: no start-up, no\n\
new TLS connection, no Keychain read per call.\n\n\
Each line on stdin is a request:   {\"id\": ANY, \"argv\": [\"issue\", \"list\", \"--json\"]}\n\
Each line on stdout answers one:   {\"id\": ANY, \"exit\": N, \"result\": JSON|null,\n                                    \
\"stdout\": TEXT|null, \"stderr\": TEXT}\n\
argv is what follows linearctl on a command line; exit and the JSON in result are\n\
exactly what that command line would give (result is null when it printed text,\n\
which is then in stdout). Requests run one at a time, in order. A malformed line\n\
is answered with exit 2. watch and serve are refused, and so is - as a file:\n\
stdin carries requests. serve ends when stdin closes (exit 0).",
        )
        .example("  printf '{\"id\":1,\"argv\":[\"issue\",\"list\",\"--json\"]}\\n' | linearctl serve --stdio")
        .run(serve)
        .flag(Flag::bool("stdio", "read requests on stdin, answer on stdout"))
}

fn serve(run: &mut Run, ctx: &Ctx) -> R {
    if !ctx.bool("stdio") {
        return Err(usage("pass --stdio (the only transport so far)"));
    }
    if let Ok((key, source)) = api_key()
        && !key.is_empty()
    {
        SERVED.with(|s| *s.borrow_mut() = Some((key, source)));
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = lines.read_until(b'\n', &mut line).map_err(|e| failed(format!("cannot read requests: {e}")))?;
        if n == 0 {
            return Ok(());
        }
        if line.len() > 16 << 20 {
            return Err(failed("cannot read requests: bufio.Scanner: token too long"));
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        if text.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let answer = serve_one(text);
        run.out
            .write_all(json::encode(&answer, false, &[]).as_bytes())
            .and_then(|()| run.out.flush())
            .map_err(|e| failed(format!("cannot answer on stdout: {}", crate::state::cause(&e))))?;
    }
}

/// Runs one request line and returns its answer.
fn serve_one(line: &[u8]) -> Value {
    let refuse = |id: Value, msg: &str| {
        json!({"id": id, "exit": crate::fail::USAGE, "result": {"error": msg, "usage": true}, "stdout": null,
            "stderr": format!("linearctl: {msg}\n")})
    };
    let malformed = "a request is one JSON object: {\"id\": ..., \"argv\": [...]}";
    let Ok(v) = json::parse(line) else { return refuse(Value::Null, malformed) };
    // Go's decoder: keys match ignoring case, argv must be strings.
    let (mut id, mut argv) = (Value::Null, Vec::<String>::new());
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
    if argv.iter().any(|a| a == "-" || a.ends_with("=-")) {
        return refuse(id, "- (stdin) is serve's own input; pass the text in argv, or a file");
    }
    let (code, out, err) = execute(&argv, Sink::Buf(vec![]), Sink::Buf(vec![]));
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
