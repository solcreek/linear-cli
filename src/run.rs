//! One invocation: parse the arguments, run the command, report how it
//! ended. main runs one; serve runs one per request, into buffers.

use crate::cli::{self, Ctx, Outcome};
use crate::client::Client;
use crate::fail::{self, Fail, R};
use crate::json;
use serde_json::{Value, json};
use std::io::{IsTerminal, Write};
use std::rc::Rc;

pub const VERSION: &str = match option_env!("LINEARCTL_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Where a run writes: the process's stream, or a serve request's buffer.
pub enum Sink {
    Stdout,
    Stderr,
    Buf(Vec<u8>),
}

impl Sink {
    pub fn into_buf(self) -> Vec<u8> {
        match self {
            Sink::Buf(b) => b,
            _ => vec![],
        }
    }
}

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Sink::Stdout => std::io::stdout().write(b),
            Sink::Stderr => std::io::stderr().write(b),
            Sink::Buf(v) => v.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Sink::Stdout => std::io::stdout().flush(),
            Sink::Stderr => std::io::stderr().flush(),
            Sink::Buf(_) => Ok(()),
        }
    }
}

/// The state of one invocation: where it writes, the global flags, and the
/// client it made, whose headers --meta reports.
pub struct Run {
    pub out: Sink,
    pub err: Sink,
    pub json: bool,
    pub meta: bool,
    pub demo: bool,
    pub timeout: u64,
    pub client: Option<Rc<Client>>,
}

impl Run {
    pub fn new(out: Sink, err: Sink) -> Run {
        Run {
            out,
            err,
            json: false,
            meta: std::env::var("LINEARCTL_META").is_ok_and(|v| v == "1"),
            demo: std::env::var("LINEARCTL_DEMO").is_ok_and(|v| v == "1"),
            timeout: 30,
            client: None,
        }
    }

    pub fn stdout_is_terminal(&self) -> bool {
        matches!(self.out, Sink::Stdout) && std::io::stdout().is_terminal()
    }

    /// Prints help for the command ctx found, on stdout.
    pub fn help(&mut self, ctx: &Ctx) {
        let text = cli::help(&ctx.found);
        let _ = self.out.write_all(text.as_bytes());
    }

    /// Writes text to stdout as it is.
    pub fn print(&mut self, s: &str) -> R {
        self.out.write_all(s.as_bytes()).map_err(|e| fail::failed(format!("cannot write stdout: {e}")))
    }

    /// Writes a line of text to stdout.
    pub fn println(&mut self, s: &str) -> R {
        let mut b = s.as_bytes().to_vec();
        b.push(b'\n');
        self.out.write_all(&b).map_err(|e| fail::failed(format!("cannot write stdout: {e}")))
    }

    /// Writes to stderr; a failure to do so is not worth failing over.
    pub fn eprint(&mut self, s: &str) {
        let _ = self.err.write_all(s.as_bytes());
    }

    /// One JSON document: indented for a person at a terminal, compact into
    /// a pipe, where an agent pays for every byte. --meta adds meta.
    pub fn print_json(&mut self, v: Value) -> R {
        self.print_json_keeping(v, &[])
    }

    /// print_json, with the subtrees of these top-level keys passed through
    /// as they came (see json::encode).
    pub fn print_json_keeping(&mut self, mut v: Value, raw: &[&str]) -> R {
        if let (Value::Object(m), true, Some(c)) = (&mut v, self.meta, &self.client) {
            m.insert("meta".into(), c.meta());
        }
        let text = json::encode(&v, self.stdout_is_terminal(), raw);
        self.out.write_all(text.as_bytes()).map_err(|e| fail::failed(format!("write: {e}")))
    }

    /// The command's result: v with --json, else the text.
    pub fn output(&mut self, v: Value, text: &str) -> R {
        if self.json { self.print_json(v) } else { self.println(text) }
    }

    /// The client for this run's requests; without an API key, an auth
    /// error (exit 5).
    pub fn client(&mut self) -> R<Rc<Client>> {
        let c = Rc::new(Client::new(self)?);
        self.client = Some(c.clone());
        Ok(c)
    }

    /// A client with this key, as auth login checks one.
    pub fn client_with(&mut self, key: &str, source: &str) -> Rc<Client> {
        let c = Rc::new(Client::with_key(self, key, source));
        self.client = Some(c.clone());
        c
    }
}

/// Runs one invocation, args without the program name, and returns its exit
/// code and the sinks it wrote to.
pub fn execute(args: &[String], out: Sink, err: Sink) -> (i32, Sink, Sink) {
    let root = crate::commands::root();
    let mut run = Run::new(out, err);
    let result = match cli::parse(&root, args) {
        Outcome::Help(ctx) => {
            run.help(&ctx);
            Ok(())
        }
        Outcome::Version => run.println(&format!("linearctl {VERSION}")),
        Outcome::Usage(msg, ctx) => {
            if let Some(ctx) = &ctx {
                globals(&mut run, ctx);
            }
            Err(Fail::Usage(msg))
        }
        Outcome::Run(ctx) => {
            globals(&mut run, &ctx);
            guard(&run, &ctx).and_then(|()| (ctx.cmd().run.expect("runnable"))(&mut run, &ctx))
        }
    };
    let code = finish(&mut run, args, result);
    let _ = run.out.flush();
    (code, run.out, run.err)
}

fn globals(run: &mut Run, ctx: &Ctx) {
    run.json = ctx.bool("json");
    run.meta = ctx.bool("meta");
    run.demo = ctx.bool("demo");
    run.timeout = ctx.whole("timeout");
}

/// Every mutating command is guarded here, so none can forget to be: it
/// needs exactly one of --dry-run and --yes, and demo mode only previews.
fn guard(run: &Run, ctx: &Ctx) -> R {
    if !ctx.cmd().mutates {
        return Ok(());
    }
    let dry_run = dry_run(ctx)?;
    if run.demo && !dry_run {
        return Err(fail::failed(format!(
            "demo mode is read-only: {} --yes changes nothing; --dry-run shows what it would do",
            ctx.path()
        )));
    }
    Ok(())
}

/// Whether this is a dry run. Neither flag, or both, is a usage error: a
/// mutating command never acts or prompts by default.
pub fn dry_run(ctx: &Ctx) -> R<bool> {
    let (dry, yes) = (ctx.bool("dry-run"), ctx.bool("yes"));
    if dry == yes {
        return Err(fail::usage(format!(
            "{} changes things: pass --dry-run to preview, or --yes to apply (one of them)",
            ctx.cmd().name()
        )));
    }
    Ok(dry)
}

fn finish(run: &mut Run, args: &[String], result: R) -> i32 {
    let Err(e) = result else { return fail::OK };
    // A partial result is already printed, problems included.
    if let Fail::Partial(_) = e.base() {
        return fail::PARTIAL;
    }
    run.eprint(&format!("linearctl: {e}\n"));
    if let Some(h) = e.hint_text() {
        run.eprint(&format!("hint: {h}\n"));
    }
    let usage = matches!(e.base(), Fail::Usage(_));
    if usage {
        run.eprint("Run 'linearctl --help' for usage.\n");
    }
    // Under --json a failure is JSON too. Parsing may have stopped before
    // it saw --json, so look at the arguments themselves.
    if run.json || args.iter().any(|a| a == "--json") {
        let mut doc = json!({"error": e.to_string(), "usage": usage});
        if let Fail::Limited(_, Some(ms)) = e.base() {
            doc["retryAt"] = json!(crate::time::rfc3339(ms * 1_000_000));
        }
        if let Some(h) = e.hint_text() {
            doc["hint"] = json!(h);
        }
        let _ = run.print_json(doc);
    }
    e.code()
}
