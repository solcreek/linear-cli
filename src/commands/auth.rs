use crate::cli::Cmd;
use crate::cli::Ctx;
use crate::fail::{R, failed, usage};
use crate::keys::store;
use crate::queries;
use crate::run::{Run, dry_run};
use crate::state::cause;
use crate::util::*;
use serde_json::{Value, json};
use std::io::{IsTerminal, Read};

pub fn command() -> Cmd {
    let mut login = Cmd::new("login", "Check an API key read from stdin and store it")
        .long(
            "Check an API key read from stdin with Linear, then store it: in the macOS Keychain,\n\
or in credentials.json under the config directory elsewhere and whenever\n\
LINEARCTL_HOME is set. LINEAR_API_KEY, when set, wins over a stored key.\n\
Create a key at https://linear.app/settings/account/security.\n\
Changes things: pass --dry-run to check the key only, --yes to store it.",
        )
        .example("  pbpaste | linearctl auth login --yes\n  linearctl auth login --dry-run --json < key.txt")
        .run(login)
        .mutating();
    login.stdin = true;
    let status = Cmd::new("status", "Show whose key is in use, where it comes from, and the rate limit left")
        .example("  linearctl auth status --json")
        .run(status);
    let logout = Cmd::new("logout", "Remove the stored API key")
        .long("Remove the stored API key. LINEAR_API_KEY, if set, still applies.\nChanges things: pass --dry-run to preview, --yes to apply.")
        .example("  linearctl auth logout --yes")
        .run(logout)
        .mutating();
    Cmd::group("auth", "Store, check or remove the Linear API key", vec![login, status, logout])
}

/// The viewer and organization as Linear's Viewer query answers them, in
/// the order linearctl has always printed them.
fn who(data: &Value) -> (Value, Value) {
    let v = &data["viewer"];
    let o = &data["organization"];
    (
        json!({"id": str_at(v, "id"), "name": str_at(v, "name"), "email": str_at(v, "email")}),
        json!({"name": str_at(o, "name"), "urlKey": str_at(o, "urlKey")}),
    )
}

fn login(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let key = secret_from_stdin()?;
    let c = run.client_with(&key, "stdin");
    let data = c.query(run, queries::VIEWER, Value::Null)?;
    let (user, org) = who(&data);
    let s = store();
    let (stored, source) = s.read()?;
    let changed = stored != key;
    if changed && !dry {
        s.write(&key)?;
    }
    let verb = match (changed, dry) {
        (true, true) => "would store",
        (true, false) => "stored",
        (false, _) => "already stored",
    };
    if std::env::var("LINEAR_API_KEY").is_ok_and(|k| !k.is_empty()) {
        run.eprint("linearctl: note: LINEAR_API_KEY is set and wins over the stored key\n");
    }
    let text = format!(
        "{verb} the key for {} ({}) in the {source}",
        printable(&str_at(&user, "name")),
        printable(&str_at(&org, "name"))
    );
    output_ordered(run, json!({"changed": changed, "dryRun": dry, "store": source, "user": user, "organization": org}), &text)
}

/// --json output where user and organization keep their field order.
fn output_ordered(run: &mut Run, v: Value, text: &str) -> R {
    if run.json { run.print_json_keeping(v, &["user", "organization"]) } else { run.println(text) }
}

fn status(run: &mut Run, _: &Ctx) -> R {
    let c = run.client()?;
    let data = c.query(run, queries::VIEWER, Value::Null)?;
    let (user, org) = who(&data);
    let limits = c.rate_limit();
    let mut text = format!(
        "{} <{}> in {}; key from {}",
        printable(&str_at(&user, "name")),
        printable(&str_at(&user, "email")),
        printable(&str_at(&org, "name")),
        c.source
    );
    if let Some(r) = limits.get("requests") {
        text.push_str(&format!("; {} requests left this hour", str_at(r, "remaining")));
    }
    output_ordered(run, json!({"source": c.source, "user": user, "organization": org, "rateLimit": limits}), &text)
}

fn logout(run: &mut Run, ctx: &Ctx) -> R {
    let dry = dry_run(ctx)?;
    let s = store();
    let (stored, source) = s.read()?;
    let mut changed = !stored.is_empty();
    if changed && !dry {
        changed = s.forget()?;
    }
    let verb = match (changed, dry) {
        (true, true) => "would remove",
        (true, false) => "removed",
        (false, _) => "no key stored",
    };
    if std::env::var("LINEAR_API_KEY").is_ok_and(|k| !k.is_empty()) {
        run.eprint("linearctl: note: LINEAR_API_KEY is still set\n");
    }
    run.output(json!({"changed": changed, "dryRun": dry, "store": source}), &format!("{verb} ({source})"))
}

/// A key piped on stdin: never from a terminal, one trailing newline
/// dropped, never empty. Errors name the input, never its content.
fn secret_from_stdin() -> R<String> {
    if std::io::stdin().is_terminal() {
        return Err(usage("pipe the API key on stdin (e.g. pbpaste | linearctl auth login --yes); stdin is a terminal"));
    }
    let mut data = vec![];
    std::io::stdin().take(4096).read_to_end(&mut data).map_err(|e| failed(format!("cannot read stdin: {}", cause(&e))))?;
    let s = String::from_utf8_lossy(&data).into_owned();
    let key = s.strip_suffix('\n').unwrap_or(&s);
    let key = key.strip_suffix('\r').unwrap_or(key).to_string();
    if key.is_empty() {
        return Err(usage("stdin is empty; pipe the API key"));
    }
    if !is_key(&key) {
        return Err(usage("stdin does not look like a Linear API key (letters, digits, _ and -, 16 to 256 of them)"));
    }
    Ok(key)
}
