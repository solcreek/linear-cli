//! What the commands share: checking inputs, shaping Linear's answers into
//! lists, and the flags several commands take.

use crate::cli::{Cmd, Ctx, Flag, Kind};
use crate::fail::{R, failed, usage};
use crate::json::{self, quote_go};
use crate::run::Run;
use serde_json::{Map, Value, json};
use std::io::{IsTerminal, Read};

/// Linear's cap on a page ("first must not be greater than 250").
pub const PAGE_SIZE: u64 = 250;

// --- patterns -------------------------------------------------------------

/// ENG-123: a team key (a letter, then up to 19 letters, digits or _), a
/// dash, and up to 9 digits.
pub fn is_ident(s: &str) -> bool {
    let Some((team, number)) = s.split_once('-') else { return false };
    let t = team.as_bytes();
    !t.is_empty()
        && t.len() <= 20
        && t[0].is_ascii_alphabetic()
        && t.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        && (1..=9).contains(&number.len())
        && number.bytes().all(|c| c.is_ascii_digit())
}

pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// A team key: 1 to 20 letters, digits or _.
pub fn is_team(s: &str) -> bool {
    (1..=20).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Go's \s: the ASCII space characters.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r')
}

/// x@y.z, with no spaces and one @.
pub fn is_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    let ok = |p: &str| !p.is_empty() && !p.contains('@') && !p.contains(is_space);
    if !ok(local) || !ok(domain) {
        return false;
    }
    // [^@\s]+\.[^@\s]+: some dot with text on both sides.
    domain.char_indices().any(|(i, c)| c == '.' && i > 0 && i + 1 < domain.len())
}

/// A project's slugId: 12 lowercase hex digits.
pub fn is_slug(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// What a stored API key may look like; it also keeps the key safe to hand
/// to `security -i`, which parses its input like a shell line.
pub fn is_key(s: &str) -> bool {
    (16..=256).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

// --- inputs ---------------------------------------------------------------

/// Refuses an empty argument or a control character in one.
pub fn plain(what: &str, s: &str) -> R<String> {
    if s.is_empty() {
        return Err(usage(format!("{what} is empty")));
    }
    if s.chars().any(char::is_control) {
        return Err(usage(format!("{what} contains a control character")));
    }
    Ok(s.to_string())
}

/// Text from files or the network made safe for a terminal.
pub fn printable(s: &str) -> String {
    s.chars().map(|c| if c.is_control() && c != '\t' { '?' } else { c }).collect()
}

/// printable, keeping newlines: a description or a comment's body.
pub fn printable_text(s: &str) -> String {
    s.split('\n').map(|l| printable(l.strip_suffix('\r').unwrap_or(l))).collect::<Vec<_>>().join("\n")
}

/// Go's strings.EqualFold, for names.
pub fn eq_fold(a: &str, b: &str) -> bool {
    a.chars().flat_map(char::to_lowercase).eq(b.chars().flat_map(char::to_lowercase))
}

pub fn or_dash(s: String) -> String {
    if s.is_empty() { "-".into() } else { s }
}

/// A URL as Go's url.Parse splits it, for the checks the commands make.
pub struct Url {
    pub scheme: String,
    pub has_user: bool,
    pub host: String,
    /// Decoded, as url.URL.Path.
    pub path: String,
}

pub fn parse_url(s: &str) -> Option<Url> {
    let (scheme, rest) = s.split_once("://")?;
    if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (has_user, host) = match authority.rsplit_once('@') {
        Some((_, h)) => (true, h),
        None => (false, authority),
    };
    if host.contains(|c: char| c == ' ' || c == '%' || c.is_control()) {
        return None;
    }
    if let Some((_, port)) = host.rsplit_once(':')
        && !host.ends_with(']') && !port.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
    Some(Url { scheme: scheme.to_ascii_lowercase(), has_user, host: host.to_string(), path: unescape(path)? })
}

/// Percent-decoding as url.Parse does it for a path; a bad escape fails.
fn unescape(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// An ISSUE argument as Linear's issue(id:) takes it: ENG-123, a UUID, or a
/// linear.app issue URL.
pub fn issue_ref(what: &str, s: &str) -> R<String> {
    plain(what, s)?;
    if s.starts_with("https://") {
        let parts: Option<Vec<String>> = parse_url(s)
            .filter(|u| u.host == "linear.app")
            .map(|u| u.path.trim_matches('/').split('/').map(String::from).collect());
        return match parts {
            Some(p) if p.len() >= 3 && p[1] == "issue" && is_ident(&p[2]) => Ok(p[2].to_uppercase()),
            _ => Err(usage(format!("{what}: not a linear.app issue URL: {s}"))),
        };
    }
    if is_ident(s) {
        return Ok(s.to_uppercase());
    }
    if is_uuid(s) {
        return Ok(s.to_lowercase());
    }
    Err(usage(format!("{what} must be an identifier like ENG-123, a UUID, or a linear.app issue URL; got {}", quote_go(s))))
}

/// Finds one issue by ref without failing when there is none: issue(id:)
/// answers a missing issue with an error for the whole query.
pub fn issue_filter(r: &str) -> Value {
    if is_uuid(r) {
        return json!({"id": {"eq": r}});
    }
    let (team, number) = r.split_once('-').unwrap_or((r, ""));
    let n: i64 = number.parse().unwrap_or(0);
    json!({"team": {"key": {"eqIgnoreCase": team}}, "number": {"eq": n}})
}

/// A random UUID v4, the form Linear takes for a client-chosen id.
pub fn new_id() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// A repeatable flag's values, each checked by plain.
pub fn values(ctx: &Ctx, name: &str) -> R<Vec<String>> {
    let vs = ctx.strs(name);
    if ctx.changed(name) && vs.is_empty() {
        return Err(usage(format!("--{name} is empty")));
    }
    for v in &vs {
        plain(&format!("--{name}"), v)?;
    }
    Ok(vs)
}

/// --team, else LINEARCTL_TEAM, else default-team in config; "" when none.
pub fn team_flag(ctx: &Ctx) -> R<String> {
    Ok(teams_flag(ctx)?.into_iter().next().unwrap_or_default())
}

/// --team, which issue list takes repeatedly, else the single default.
pub fn teams_flag(ctx: &Ctx) -> R<Vec<String>> {
    let mut given = if ctx.kind("team") == Some(Kind::Strs) {
        values(ctx, "team")?
    } else {
        let t = ctx.str("team");
        if t.is_empty() { vec![] } else { vec![t] }
    };
    for t in &mut given {
        if !is_team(t) {
            return Err(usage(format!("--team must be a team key like ENG; got {}", quote_go(t))));
        }
        *t = t.to_uppercase();
    }
    if !given.is_empty() {
        return Ok(given);
    }
    let team = default_team()?;
    Ok(if team.is_empty() { vec![] } else { vec![team] })
}

/// LINEARCTL_TEAM, else default-team in config.
pub fn default_team() -> R<String> {
    let (mut team, mut from) = (std::env::var("LINEARCTL_TEAM").unwrap_or_default(), "LINEARCTL_TEAM");
    if team.is_empty() {
        team = read_config()?.get("default-team").and_then(Value::as_str).unwrap_or("").to_string();
        from = "default-team in config";
    }
    if !team.is_empty() && !is_team(&team) {
        return Err(usage(format!("{from} must be a team key like ENG; got {}", quote_go(&team))));
    }
    Ok(team.to_uppercase())
}

/// config.json, empty when there is none. Values are strings.
pub fn read_config() -> R<Map<String, Value>> {
    let dir = crate::state::home("config");
    if dir.is_empty() {
        return Ok(Map::new());
    }
    let path = format!("{dir}/config.json");
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(failed(format!("cannot read {path}: {}", crate::state::cause(&e)))),
    };
    let invalid = |e: String| failed(format!("{path} is not valid JSON: {e}"));
    let v = json::parse(&data).map_err(invalid)?;
    let mut cfg = Map::new();
    match v {
        Value::Null => {}
        Value::Object(m) => {
            for (k, x) in m {
                match x {
                    Value::String(_) => _ = cfg.insert(k, x),
                    Value::Null => {}
                    other => return Err(invalid(json::type_error(&other, &k, "string"))),
                }
            }
        }
        other => return Err(invalid(json::type_error(&other, "", "map[string]string"))),
    }
    Ok(cfg)
}

/// The value of --NAME or --NAME-file (PATH, or - for stdin); None when
/// neither was given.
pub fn text(ctx: &Ctx, name: &str) -> R<Option<String>> {
    let (given, file_given) = (ctx.changed(name), ctx.changed(&format!("{name}-file")));
    if given && file_given {
        return Err(usage(format!("pass --{name} or --{name}-file, not both")));
    }
    if !file_given {
        return Ok(given.then(|| ctx.str(name)));
    }
    let data = read_input(&format!("--{name}-file"), &ctx.str(&format!("{name}-file")))?;
    Ok(Some(String::from_utf8_lossy(&data).into_owned()))
}

/// Reads PATH, or stdin for "-". Stdin from a terminal is refused: the
/// command would otherwise wait for input nobody knows to type.
pub fn read_input(what: &str, path: &str) -> R<Vec<u8>> {
    if path == "-" {
        if std::io::stdin().is_terminal() {
            return Err(usage(format!("{what} -: pipe the input; stdin is a terminal")));
        }
        let mut data = vec![];
        std::io::stdin().take(16 << 20).read_to_end(&mut data).map_err(|e| failed(format!("cannot read stdin: {e}")))?;
        return Ok(data);
    }
    plain(what, path)?;
    std::fs::read(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => failed(format!("no such file: {path}")),
        _ => failed(format!("cannot read {path}: {}", crate::state::cause(&e))),
    })
}

// --- answers --------------------------------------------------------------

/// Replaces every {"nodes": [...]} with its list, so a connection reads as
/// an array in our JSON.
pub fn flatten(v: Value) -> Value {
    match v {
        Value::Object(mut m) => {
            if m.len() == 1 && m.contains_key("nodes") {
                return flatten(m.remove("nodes").unwrap());
            }
            Value::Object(m.into_iter().map(|(k, x)| (k, flatten(x))).collect())
        }
        Value::Array(a) => Value::Array(a.into_iter().map(flatten).collect()),
        v => v,
    }
}

/// --fields as a list, checked against what the command returns.
pub fn fields_flag(ctx: &Ctx, known: &[&str]) -> R<Vec<String>> {
    let raw = ctx.str("fields");
    if raw.is_empty() {
        return Ok(vec![]);
    }
    raw.split(',')
        .map(|f| {
            let f = f.trim_matches(|c: char| c.is_whitespace());
            if known.contains(&f) {
                Ok(f.to_string())
            } else {
                Err(usage(format!("--fields: no field {}; known: {}", quote_go(f), known.join(","))))
            }
        })
        .collect()
}

/// Keeps only fields of an item (all when fields is empty).
pub fn pick(item: Value, fields: &[String]) -> Value {
    if fields.is_empty() {
        return item;
    }
    let mut out = Map::new();
    for f in fields {
        out.insert(f.clone(), item.get(f).cloned().unwrap_or(Value::Null));
    }
    Value::Object(out)
}

/// Adds --fields and lists the known fields in the long help.
pub fn fields_flag_spec(mut c: Cmd, known: &[&str]) -> Cmd {
    c.flags.push(Flag::str("fields", "return only these comma-separated `FIELDS` (see Fields above)"));
    if c.long.is_empty() {
        c.long = format!("{}.", c.short);
    }
    c.long.push_str("\n\n");
    c.long.push_str(&wrap(&format!("Fields: {}", known.join(", ")), 80));
    c
}

/// Breaks text into lines of at most width columns, between words.
pub fn wrap(text: &str, width: usize) -> String {
    let mut lines = vec![];
    let mut line = String::new();
    for w in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + w.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(w);
    }
    lines.push(line);
    lines.join("\n")
}

pub fn limit_flag(def: u64) -> Flag {
    Flag::whole("limit", def, "show at most `N` items; 0 for all")
}

pub fn cursor_flag() -> Flag {
    Flag::str("cursor", "continue after `CURSOR` (nextCursor from the previous page)")
}

pub fn cursor_value(ctx: &Ctx) -> R<String> {
    let c = ctx.str("cursor");
    if c.is_empty() { Ok(c) } else { plain("--cursor", &c) }
}

/// A GraphQL variable that is null when empty.
pub fn optional(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}

/// One connection page as Linear returns it.
pub struct Page {
    pub nodes: Vec<Value>,
    pub has_next: bool,
    pub end_cursor: String,
}

impl Page {
    pub fn from(v: &Value) -> Page {
        Page {
            nodes: v["nodes"].as_array().cloned().unwrap_or_default(),
            has_next: v["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false),
            end_cursor: v["pageInfo"]["endCursor"].as_str().unwrap_or("").to_string(),
        }
    }
}

/// Pages through a connection until limit items (0: all). fetch gets one
/// page of at most first items after a cursor.
pub fn collect(limit: u64, mut cursor: String, mut fetch: impl FnMut(u64, &str) -> R<Page>) -> R<(Vec<Value>, String)> {
    let mut items = vec![];
    loop {
        let mut first = PAGE_SIZE;
        if limit > 0 && limit - (items.len() as u64) < first {
            first = limit - items.len() as u64;
        }
        let p = fetch(first, &cursor)?;
        items.extend(p.nodes);
        if !p.has_next {
            return Ok((items, String::new()));
        }
        cursor = p.end_cursor;
        if limit > 0 && items.len() as u64 >= limit {
            return Ok((items, cursor));
        }
    }
}

/// The list shape: Linear connections have no count, so total is null;
/// truncated says whether more remain, and nextCursor is where --cursor
/// continues.
pub fn list_doc(run: &mut Run, items: &[Value], next: &str, fields: &[String]) -> Value {
    let out: Vec<Value> = items.iter().map(|it| pick(flatten(it.clone()), fields)).collect();
    if !next.is_empty() {
        run.eprint(&format!("{} shown, more remain; --cursor {next} for the next page, --limit 0 for all\n", items.len()));
    }
    json!({"items": out, "total": null, "shown": out.len(), "truncated": !next.is_empty(), "nextCursor": optional(next)})
}

/// A nested field of a decoded item ("state.name") as text, "" if absent.
pub fn str_at(item: &Value, path: &str) -> String {
    let mut v = item;
    for k in path.split('.') {
        match v {
            Value::Object(m) => v = m.get(k).unwrap_or(&Value::Null),
            _ => return String::new(),
        }
    }
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => json::sprint_float(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Prints items one per line, tab-separated, sanitized for a terminal.
pub fn rows(run: &mut Run, items: &[Value], cols: &[&str]) -> R {
    for it in items {
        let vals: Vec<String> = cols.iter().map(|c| or_dash(printable(&str_at(it, c)))).collect();
        run.println(&vals.join("\t"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(is_ident("ENG-123") && is_ident("a_1-1") && !is_ident("1ENG-1") && !is_ident("ENG-") && !is_ident("ENG-1234567890"));
        assert!(is_uuid("0b6c5f0e-0000-4000-8000-000000000001") && !is_uuid("0b6c5f0e00004000800000000000000001"));
        assert!(is_email("a@b.co") && !is_email("a@b") && !is_email("a b@c.d") && !is_email("a@@b.c") && !is_email("@b.c"));
        assert!(is_email("a@b.c.d") && !is_email("a@.b") && is_email("a@x.b"));
    }

    #[test]
    fn issue_refs() {
        assert_eq!(issue_ref("ISSUE", "eng-1").unwrap(), "ENG-1");
        assert_eq!(issue_ref("ISSUE", "https://linear.app/acme/issue/eng-7/title").unwrap(), "ENG-7");
        assert!(issue_ref("ISSUE", "https://linear.app/acme/project/x").is_err());
        assert_eq!(issue_filter("ENG-7"), json!({"team": {"key": {"eqIgnoreCase": "ENG"}}, "number": {"eq": 7}}));
    }

    #[test]
    fn wraps() {
        assert_eq!(wrap("a bb ccc", 4), "a bb\nccc");
    }

    #[test]
    fn new_ids_are_v4() {
        let id = new_id();
        assert!(is_uuid(&id) && &id[14..15] == "4" && "89ab".contains(&id[19..20]));
    }
}
