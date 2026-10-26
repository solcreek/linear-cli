//! The command line: commands and flags as data, parsed, documented in
//! --help and listed by describe from the same definitions.
//!
//! linearctl was built on cobra and pflag; agents and scripts depend on how
//! it reads its arguments and on what its help and errors say, so this
//! follows their rules exactly: `--flag value` and `--flag=value`, flags
//! anywhere among the arguments, `--` ending them, cobra's way of finding
//! the subcommand, and its help layout.

use crate::fail::R;
use crate::json::quote_go;
use crate::run::Run;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Bool,
    Str,
    /// Repeatable: each use adds a value (pflag's stringArray).
    Strs,
    /// Base-10 digits only (pflag would read 010 as octal and take 0x3).
    Whole,
}

#[derive(Clone, Debug)]
pub struct Flag {
    pub name: &'static str,
    pub short: Option<char>,
    pub kind: Kind,
    /// The default as text, as pflag shows it: "false", "", "[]", "30".
    pub default: String,
    /// The help line; a `QUOTED` word names the value.
    pub usage: String,
    /// The value of `--flag` given alone (bools: "true").
    pub no_opt: Option<String>,
    pub hidden: bool,
}

impl Flag {
    pub fn bool(name: &'static str, usage: &str) -> Flag {
        Flag::new(name, Kind::Bool, "false", usage)
    }
    pub fn str(name: &'static str, usage: &str) -> Flag {
        Flag::new(name, Kind::Str, "", usage)
    }
    pub fn strs(name: &'static str, usage: &str) -> Flag {
        Flag::new(name, Kind::Strs, "[]", usage)
    }
    pub fn whole(name: &'static str, default: u64, usage: &str) -> Flag {
        Flag::new(name, Kind::Whole, &default.to_string(), usage)
    }
    pub fn new(name: &'static str, kind: Kind, default: &str, usage: &str) -> Flag {
        Flag {
            name,
            short: None,
            kind,
            default: default.into(),
            usage: usage.into(),
            no_opt: (kind == Kind::Bool).then(|| "true".into()),
            hidden: false,
        }
    }
    pub fn default_to(mut self, d: &str) -> Flag {
        self.default = d.into();
        self
    }
    pub fn no_opt(mut self, v: &str) -> Flag {
        self.no_opt = Some(v.into());
        self
    }
    pub fn hidden(mut self) -> Flag {
        self.hidden = true;
        self
    }
    fn type_name(&self) -> &'static str {
        match self.kind {
            Kind::Bool => "bool",
            Kind::Str => "string",
            Kind::Strs => "stringArray",
            Kind::Whole => "uint",
        }
    }
}

pub type Handler = fn(&mut Run, &Ctx) -> R;
pub type ArgCheck = fn(&Ctx) -> Result<(), String>;

#[derive(Clone, Copy)]
pub enum Args {
    /// cobra's Args == nil: anything, but a root with subcommands refuses
    /// an unknown one.
    Legacy,
    None,
    Exact(usize),
    AtLeast(usize),
    Custom(ArgCheck),
}

#[derive(Clone)]
pub struct Cmd {
    /// "show ISSUE": the name, then the positionals ([X] is optional).
    pub use_line: String,
    pub aliases: Vec<&'static str>,
    pub short: String,
    pub long: String,
    pub example: String,
    pub flags: Vec<Flag>,
    /// Flags every command below inherits (the root's).
    pub persistent: Vec<Flag>,
    pub subs: Vec<Cmd>,
    pub args: Args,
    /// None: not runnable, so it shows its help.
    pub run: Option<Handler>,
    pub mutates: bool,
    /// It reads a secret on stdin.
    pub stdin: bool,
    pub hidden: bool,
    pub flags_in_use_line: bool,
    pub parse_flags: bool,
    /// cobra's help command: not listed by describe.
    pub help_command: bool,
}

impl Cmd {
    pub fn new(use_line: &str, short: &str) -> Cmd {
        Cmd {
            use_line: use_line.into(),
            aliases: vec![],
            short: short.into(),
            long: String::new(),
            example: String::new(),
            flags: vec![],
            persistent: vec![],
            subs: vec![],
            args: Args::None,
            run: None,
            mutates: false,
            stdin: false,
            hidden: false,
            flags_in_use_line: true,
            parse_flags: true,
            help_command: false,
        }
    }

    /// A command that only groups others: run alone, it shows its help.
    pub fn group(name: &str, short: &str, subs: Vec<Cmd>) -> Cmd {
        let mut c = Cmd::new(name, short);
        c.run = Some(|run, ctx| {
            run.help(ctx);
            Ok(())
        });
        c.subs = subs;
        c
    }

    pub fn name(&self) -> &str {
        self.use_line.split(' ').next().unwrap_or("")
    }

    pub fn long(mut self, s: &str) -> Cmd {
        self.long = s.into();
        self
    }
    pub fn example(mut self, s: &str) -> Cmd {
        self.example = s.into();
        self
    }
    pub fn args(mut self, a: Args) -> Cmd {
        self.args = a;
        self
    }
    pub fn run(mut self, h: Handler) -> Cmd {
        self.run = Some(h);
        self
    }
    pub fn flag(mut self, f: Flag) -> Cmd {
        self.flags.push(f);
        self
    }
    pub fn alias(mut self, a: &'static str) -> Cmd {
        self.aliases.push(a);
        self
    }

    /// Gives a command --dry-run and --yes; it refuses to run without
    /// exactly one of them.
    pub fn mutating(mut self) -> Cmd {
        self.flags.push(Flag::bool("dry-run", "show what would change; change nothing"));
        self.flags.push(Flag::bool("yes", "apply the change"));
        self.mutates = true;
        self
    }

    fn runnable(&self) -> bool {
        self.run.is_some()
    }

    fn find_sub(&self, name: &str) -> Option<usize> {
        self.subs.iter().position(|s| s.name() == name || s.aliases.contains(&name))
    }

    /// cobra's IsAvailableCommand: listed in help and describe.
    pub fn available(&self) -> bool {
        !self.hidden && !self.help_command && (self.runnable() || self.subs.iter().any(Cmd::available))
    }
}

/// The help flag cobra gives every command.
fn help_flag(name: &str) -> Flag {
    let mut f = Flag::bool("help", &format!("help for {name}"));
    f.short = Some('h');
    f
}

/// A command found in the tree, with the flags it reads.
pub struct Found<'a> {
    pub chain: Vec<&'a Cmd>,
    /// The name it was called by: its own, or an alias.
    pub called_as: String,
}

impl<'a> Found<'a> {
    pub fn cmd(&self) -> &'a Cmd {
        self.chain[self.chain.len() - 1]
    }

    pub fn path(&self) -> String {
        self.chain.iter().map(|c| c.name()).collect::<Vec<_>>().join(" ")
    }

    fn is_root(&self) -> bool {
        self.chain.len() == 1
    }

    /// Its own flags (the root's persistent ones count as its own), help
    /// included, sorted by name.
    pub fn local_flags(&self) -> Vec<Flag> {
        let c = self.cmd();
        let mut flags: Vec<Flag> = c.flags.clone();
        flags.extend(c.persistent.iter().cloned());
        if !flags.iter().any(|f| f.name == "help") {
            flags.push(help_flag(c.name()));
        }
        flags.sort_by(|a, b| a.name.cmp(b.name));
        flags
    }

    /// The flags it inherits from the commands above it, sorted.
    pub fn inherited_flags(&self) -> Vec<Flag> {
        let mut flags: Vec<Flag> = vec![];
        for c in &self.chain[..self.chain.len() - 1] {
            for f in &c.persistent {
                if !flags.iter().any(|g| g.name == f.name) {
                    flags.push(f.clone());
                }
            }
        }
        let local = self.local_flags();
        flags.retain(|f| !local.iter().any(|l| l.name == f.name));
        flags.sort_by(|a, b| a.name.cmp(b.name));
        flags
    }

    fn all_flags(&self) -> Vec<Flag> {
        let mut f = self.local_flags();
        f.extend(self.inherited_flags());
        f
    }
}

/// The flags and positionals of one invocation.
pub struct Ctx<'a> {
    pub found: Found<'a>,
    pub args: Vec<String>,
    values: HashMap<&'static str, Vec<String>>,
    changed: HashSet<&'static str>,
    kinds: HashMap<&'static str, Kind>,
}

impl<'a> Ctx<'a> {
    pub fn cmd(&self) -> &'a Cmd {
        self.found.cmd()
    }

    /// "linearctl issue list"
    pub fn path(&self) -> String {
        self.found.path()
    }

    pub fn changed(&self, name: &str) -> bool {
        self.changed.contains(name)
    }

    pub fn str(&self, name: &str) -> String {
        self.values.get(name).and_then(|v| v.last()).cloned().unwrap_or_default()
    }

    pub fn strs(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }

    pub fn bool(&self, name: &str) -> bool {
        self.str(name) == "true"
    }

    pub fn whole(&self, name: &str) -> u64 {
        self.str(name).parse().unwrap_or(0)
    }

    pub fn has_flag(&self, name: &str) -> bool {
        self.kinds.contains_key(name)
    }

    pub fn kind(&self, name: &str) -> Option<Kind> {
        self.kinds.get(name).copied()
    }
}

/// What parsing the command line came to.
pub enum Outcome<'a> {
    Run(Ctx<'a>),
    Help(Ctx<'a>),
    Version,
    /// A usage error, with whatever flags were read before it (--json may
    /// be among them).
    Usage(String, Option<Ctx<'a>>),
}

/// Finds the command args name and reads its flags, as cobra's ExecuteC.
pub fn parse<'a>(root: &'a Cmd, args: &[String]) -> Outcome<'a> {
    let (found, rest) = find(root, args);
    let cmd = found.cmd();
    if let Args::Legacy = cmd.args {
        // cobra's legacyArgs: a root with subcommands refuses an unknown one.
        let stripped = strip_flags(&rest, &found.all_flags());
        if !cmd.subs.is_empty() && found.is_root() && !stripped.is_empty() {
            return Outcome::Usage(format!("unknown command {} for {}", quote_go(&stripped[0]), quote_go(&found.path())), None);
        }
    }
    let flags = found.all_flags();
    let mut ctx = Ctx {
        found,
        args: vec![],
        values: HashMap::new(),
        changed: HashSet::new(),
        kinds: flags.iter().map(|f| (f.name, f.kind)).collect(),
    };
    for f in &flags {
        if f.kind == Kind::Strs {
            ctx.values.insert(f.name, vec![]);
        } else {
            ctx.values.insert(f.name, vec![f.default.clone()]);
        }
    }
    if !cmd.parse_flags {
        ctx.args = rest;
        return Outcome::Run(ctx);
    }
    if let Err(e) = parse_flags(&mut ctx, &flags, &rest) {
        return Outcome::Usage(e, Some(ctx));
    }
    if ctx.bool("help") {
        return Outcome::Help(ctx);
    }
    if ctx.found.is_root() && ctx.bool("version") {
        return Outcome::Version;
    }
    if !cmd.runnable() {
        return Outcome::Help(ctx);
    }
    let check = match cmd.args {
        Args::Legacy => Ok(()),
        Args::None if !ctx.args.is_empty() => {
            Err(format!("unknown command {} for {}", quote_go(&ctx.args[0]), quote_go(&ctx.path())))
        }
        Args::None => Ok(()),
        Args::Exact(n) if ctx.args.len() != n => Err(format!("accepts {n} arg(s), received {}", ctx.args.len())),
        Args::Exact(_) => Ok(()),
        Args::AtLeast(n) if ctx.args.len() < n => {
            Err(format!("requires at least {n} arg(s), only received {}", ctx.args.len()))
        }
        Args::AtLeast(_) => Ok(()),
        Args::Custom(f) => f(&ctx),
    };
    match check {
        Ok(()) => Outcome::Run(ctx),
        Err(e) => Outcome::Usage(e, Some(ctx)),
    }
}

/// cobra's Find: walks the subcommand names among args, skipping flags and
/// the values of flags that take one.
fn find<'a>(root: &'a Cmd, args: &[String]) -> (Found<'a>, Vec<String>) {
    let mut found = root_found(root);
    let mut args = args.to_vec();
    loop {
        let flags = found.all_flags();
        let names = strip_flags(&args, &flags);
        let Some(next) = names.first() else { break };
        let Some(i) = found.cmd().find_sub(next) else { break };
        args = minus_first(&args, next, &flags);
        found.called_as = next.clone();
        found.chain.push(&found.cmd().subs[i]);
    }
    (found, args)
}

fn takes_no_value(name: &str, flags: &[Flag]) -> bool {
    flags.iter().any(|f| f.name == name && f.no_opt.is_some())
}

fn short_takes_no_value(name: &str, flags: &[Flag]) -> bool {
    let Some(c) = name.chars().next() else { return false };
    flags.iter().any(|f| f.short == Some(c) && f.no_opt.is_some())
}

/// cobra's stripFlags: the arguments that are not flags or their values.
fn strip_flags(args: &[String], flags: &[Flag]) -> Vec<String> {
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        i += 1;
        if s == "--" {
            break;
        }
        let long = s.starts_with("--") && !s.contains('=') && !takes_no_value(&s[2..], flags);
        let short = s.starts_with('-') && !s.contains('=') && s.len() == 2 && !short_takes_no_value(&s[1..], flags);
        if long || short {
            if args.len() - i <= 1 {
                break;
            }
            i += 1;
            continue;
        }
        if !s.is_empty() && !s.starts_with('-') {
            out.push(s.clone());
        }
    }
    out
}

/// cobra's argsMinusFirstX: args without the first x that is not a flag's
/// value.
fn minus_first(args: &[String], x: &str, flags: &[Flag]) -> Vec<String> {
    let mut pos = 0;
    while pos < args.len() {
        let s = &args[pos];
        if s == "--" {
            break;
        }
        let long = s.starts_with("--") && !s.contains('=') && !takes_no_value(&s[2..], flags);
        let short = s.starts_with('-') && !s.contains('=') && s.len() == 2 && !short_takes_no_value(&s[1..], flags);
        if long || short {
            pos += 2;
            continue;
        }
        if !s.starts_with('-') && s == x {
            let mut out = args.to_vec();
            out.remove(pos);
            return out;
        }
        pos += 1;
    }
    args.to_vec()
}

/// pflag's Parse, flags interspersed with arguments.
fn parse_flags(ctx: &mut Ctx, flags: &[Flag], args: &[String]) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        i += 1;
        if s.len() < 2 || !s.starts_with('-') {
            ctx.args.push(s.clone());
            continue;
        }
        if s == "--" {
            ctx.args.extend(args[i..].iter().cloned());
            break;
        }
        if let Some(name) = s.strip_prefix("--") {
            if name.starts_with('-') || name.starts_with('=') {
                return Err(format!("bad flag syntax: {s}"));
            }
            let (name, value) = match name.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (name, None),
            };
            let Some(flag) = flags.iter().find(|f| f.name == name) else {
                return Err(format!("unknown flag: --{name}"));
            };
            let value = match (value, &flag.no_opt) {
                (Some(v), _) => v,
                (None, Some(d)) => d.clone(),
                (None, None) if i < args.len() => {
                    i += 1;
                    args[i - 1].clone()
                }
                (None, None) => return Err(format!("flag needs an argument: {s}")),
            };
            set(ctx, flag, &value)?;
            continue;
        }
        // -abc: shorthands, -f=value, -fvalue or -f value.
        let mut shorts = &s[1..];
        while !shorts.is_empty() {
            let c = shorts.chars().next().unwrap();
            let Some(flag) = flags.iter().find(|f| f.short == Some(c)) else {
                return Err(format!("unknown shorthand flag: {} in -{shorts}", quote_char(c)));
            };
            let rest = &shorts[c.len_utf8()..];
            let value = if rest.len() > 1 && rest.starts_with('=') {
                let v = rest[1..].to_string();
                shorts = "";
                v
            } else if let Some(d) = &flag.no_opt {
                shorts = rest;
                d.clone()
            } else if !rest.is_empty() {
                let v = rest.to_string();
                shorts = "";
                v
            } else if i < args.len() {
                i += 1;
                shorts = "";
                args[i - 1].clone()
            } else {
                return Err(format!("flag needs an argument: {} in -{shorts}", quote_char(c)));
            };
            set(ctx, flag, &value)?;
        }
    }
    Ok(())
}

/// Go's %q of a single character: 'v'.
fn quote_char(c: char) -> String {
    match c {
        '\'' => "'\\''".into(),
        '"' => "'\"'".into(),
        _ => {
            let q = quote_go(&c.to_string());
            format!("'{}'", &q[1..q.len() - 1])
        }
    }
}

fn set(ctx: &mut Ctx, flag: &Flag, value: &str) -> Result<(), String> {
    let invalid = |e: String| {
        let name = match flag.short {
            Some(c) => format!("-{c}, --{}", flag.name),
            None => format!("--{}", flag.name),
        };
        format!("invalid argument {} for {} flag: {e}", quote_go(value), quote_go(&name))
    };
    let stored = match flag.kind {
        Kind::Bool => match value {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => "true".to_string(),
            "0" | "f" | "F" | "FALSE" | "false" | "False" => "false".to_string(),
            _ => return Err(invalid(format!("strconv.ParseBool: parsing {}: invalid syntax", quote_go(value)))),
        },
        Kind::Whole => {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid(format!("takes a whole number, got {}", quote_go(value))));
            }
            match value.parse::<i64>() {
                Ok(n) => n.to_string(),
                Err(_) => return Err(invalid(format!("{} is too large", quote_go(value)))),
            }
        }
        Kind::Str | Kind::Strs => value.to_string(),
    };
    let first = ctx.changed.insert(flag.name);
    let slot = ctx.values.entry(flag.name).or_default();
    if flag.kind == Kind::Strs {
        if first {
            slot.clear();
        }
        // pflag reads --flag "" on an array as no value at all.
        if (!stored.is_empty() || !first || !slot.is_empty())
            && !stored.is_empty() {
                slot.push(stored);
            }
    } else {
        *slot = vec![stored];
    }
    Ok(())
}

// --- help -------------------------------------------------------------------

pub const EXIT_HELP: &str = "Exit codes: 0 ok, 1 failed, 2 usage, 3 partial, 4 busy, 5 auth (no API key, or\n\
Linear rejected it), 6 rate-limited (retry after retryAt), 7 conflict (it changed since\n\
you read it; read it again)";

/// cobra's help for found: its long text, usage, flags, and the exit codes.
pub fn help(found: &Found) -> String {
    let c = found.cmd();
    let mut s = String::new();
    let text = if c.long.is_empty() { &c.short } else { &c.long };
    let text = text.trim_end();
    if !text.is_empty() {
        s.push_str(text);
        s.push_str("\n\n");
    }
    if c.runnable() || !c.subs.is_empty() {
        s.push_str(&usage_text(found));
    }
    s.push('\n');
    s.push_str(EXIT_HELP);
    s.push_str("\nRun 'linearctl describe' for every command as JSON.\n");
    s
}

/// cobra's UseLine: "linearctl issue show ISSUE [flags]".
pub fn use_line(found: &Found) -> String {
    let c = found.cmd();
    let parent: Vec<&str> = found.chain[..found.chain.len() - 1].iter().map(|c| c.name()).collect();
    let mut line = if parent.is_empty() { c.use_line.clone() } else { format!("{} {}", parent.join(" "), c.use_line) };
    if c.flags_in_use_line && !line.contains("[flags]") {
        line.push_str(" [flags]");
    }
    line
}

fn usage_text(found: &Found) -> String {
    let c = found.cmd();
    let path = found.path();
    let subs: Vec<&Cmd> = sorted_subs(c);
    let has_available = subs.iter().any(|s| s.available());
    let mut s = String::from("Usage:");
    if c.runnable() {
        s.push_str("\n  ");
        s.push_str(&use_line(found));
    }
    if has_available {
        s.push_str(&format!("\n  {path} [command]"));
    }
    if !c.aliases.is_empty() {
        s.push_str("\n\nAliases:\n  ");
        let mut names = vec![c.name()];
        names.extend(c.aliases.iter().copied());
        s.push_str(&names.join(", "));
    }
    if !c.example.is_empty() {
        s.push_str("\n\nExamples:\n");
        s.push_str(&c.example);
    }
    if has_available {
        s.push_str("\n\nAvailable Commands:");
        let pad = subs.iter().map(|s| s.name().len()).max().unwrap_or(0).max(11);
        for sub in &subs {
            if sub.available() || sub.name() == "help" {
                s.push_str(&format!("\n  {:<pad$} {}", sub.name(), sub.short));
            }
        }
    }
    let local: Vec<Flag> = found.local_flags().into_iter().filter(|f| !f.hidden).collect();
    if !local.is_empty() {
        s.push_str("\n\nFlags:\n");
        s.push_str(flag_usages(&local).trim_end());
    }
    let inherited: Vec<Flag> = found.inherited_flags().into_iter().filter(|f| !f.hidden).collect();
    if !inherited.is_empty() {
        s.push_str("\n\nGlobal Flags:\n");
        s.push_str(flag_usages(&inherited).trim_end());
    }
    if has_available {
        s.push_str(&format!("\n\nUse \"{path} [command] --help\" for more information about a command."));
    }
    s.push('\n');
    s
}

fn sorted_subs(c: &Cmd) -> Vec<&Cmd> {
    let mut subs: Vec<&Cmd> = c.subs.iter().collect();
    subs.sort_by(|a, b| a.name().cmp(b.name()));
    subs
}

/// pflag's UnquoteUsage: the `NAME` in a usage line, or the type's name.
fn unquote_usage(f: &Flag) -> (String, String) {
    if let Some(i) = f.usage.find('`')
        && let Some(j) = f.usage[i + 1..].find('`') {
            let name = &f.usage[i + 1..i + 1 + j];
            let usage = format!("{}{}{}", &f.usage[..i], name, &f.usage[i + 2 + j..]);
            return (name.to_string(), usage);
        }
    let name = match f.kind {
        Kind::Bool => "",
        k => Flag::new("", k, "", "").type_name(),
    };
    (name.to_string(), f.usage.clone())
}

/// pflag's defaultIsZeroValue.
fn default_is_zero(f: &Flag) -> bool {
    match f.kind {
        Kind::Bool => f.default == "false" || f.default.is_empty(),
        Kind::Str => f.default.is_empty(),
        Kind::Strs => f.default == "[]",
        Kind::Whole => matches!(f.default.as_str(), "false" | "<nil>" | "" | "0"),
    }
}

/// pflag's FlagUsages: one aligned line per flag.
fn flag_usages(flags: &[Flag]) -> String {
    let mut lines = vec![];
    let mut max = 0;
    for f in flags {
        let mut line = match f.short {
            Some(c) => format!("  -{c}, --{}", f.name),
            None => format!("      --{}", f.name),
        };
        let (name, usage) = unquote_usage(f);
        if !name.is_empty() {
            line.push(' ');
            line.push_str(&name);
        }
        if let Some(d) = &f.no_opt {
            match f.kind {
                Kind::Str => line.push_str(&format!("[=\"{d}\"]")),
                Kind::Bool if d != "true" => line.push_str(&format!("[={d}]")),
                Kind::Bool => {}
                _ => line.push_str(&format!("[={d}]")),
            }
        }
        // pflag measures the line with the \0 it later replaces by spacing.
        max = max.max(line.len() + 1);
        let mut tail = usage;
        if !default_is_zero(f) {
            if f.kind == Kind::Str {
                tail.push_str(&format!(" (default {})", quote_go(&f.default)));
            } else {
                tail.push_str(&format!(" (default {})", f.default));
            }
        }
        lines.push((line, tail));
    }
    let mut out = String::new();
    for (head, tail) in lines {
        let spacing = " ".repeat(max - head.len());
        let tail = tail.replace('\n', &format!("\n{}", " ".repeat(max + 2)));
        out.push_str(&format!("{head} {spacing} {tail}\n"));
    }
    out
}

// --- describe -----------------------------------------------------------------

/// The same definitions as --help, as data.
pub fn describe(found: &Found, version: &str) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    let c = found.cmd();
    let flag = |f: &Flag| {
        let kind = match f.kind {
            Kind::Bool => "boolean",
            Kind::Whole => "integer",
            _ => "string",
        };
        let default = if f.kind != Kind::Bool && !f.default.is_empty() { json!(f.default) } else { Value::Null };
        json!({"name": f.name, "short": f.short.map(String::from), "type": kind, "default": default,
            "description": f.usage.replace('`', "")})
    };
    let mut flags: Vec<Value> = found.local_flags().iter().filter(|f| !f.hidden).map(flag).collect();
    flags.extend(found.inherited_flags().iter().filter(|f| !f.hidden).map(flag));
    let subs: Vec<Value> = sorted_subs(c)
        .into_iter()
        .filter(|s| s.available() && s.name() != "completion")
        .map(|s| {
            let mut chain = found.chain.clone();
            chain.push(s);
            describe(&Found { chain, called_as: s.name().into() }, version)
        })
        .collect();
    let mut d = Map::new();
    d.insert("name".into(), json!(c.name()));
    d.insert("summary".into(), json!(c.short));
    d.insert("flags".into(), Value::Array(flags));
    d.insert("commands".into(), Value::Array(subs));
    if found.is_root() {
        d.insert("version".into(), json!(version));
        d.insert("exitCodes".into(), json!({"ok": 0, "failed": 1, "usage": 2, "partial": 3, "busy": 4, "auth": 5,
            "rateLimited": 6, "conflict": 7}));
        return Value::Object(d);
    }
    let args: Vec<Value> = c
        .use_line
        .split_whitespace()
        .skip(1)
        .map(|w| json!({"name": w.trim_matches(|c| c == '[' || c == ']'), "required": !w.starts_with('['), "default": null}))
        .collect();
    let examples: Vec<Value> = c.example.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| json!(l)).collect();
    d.insert("usage".into(), json!(use_line(found)));
    d.insert("aliases".into(), json!(c.aliases));
    d.insert("mutates".into(), json!(c.mutates));
    if c.stdin {
        d.insert("stdin".into(), json!(true));
    }
    d.insert("args".into(), Value::Array(args));
    d.insert("examples".into(), Value::Array(examples));
    Value::Object(d)
}

/// Every command, for describe and completion: the root found alone.
pub fn root_found(root: &Cmd) -> Found<'_> {
    Found { chain: vec![root], called_as: root.name().into() }
}

/// The command at path (names, not aliases), if there is one.
pub fn found_at<'a>(root: &'a Cmd, args: &[String]) -> Found<'a> {
    find(root, args).0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn strip_flags_as_cobra() {
        let flags = vec![Flag::bool("json", ""), Flag::str("team", "")];
        assert_eq!(strip_flags(&s(&["--team", "ENG", "issue", "list"]), &flags), s(&["issue", "list"]));
        assert_eq!(strip_flags(&s(&["--json", "issue"]), &flags), s(&["issue"]));
        // An unknown flag takes the next word as its value.
        assert_eq!(strip_flags(&s(&["--zz", "issue", "list"]), &flags), s(&["list"]));
        assert_eq!(strip_flags(&s(&["--team", "ENG"]), &flags), Vec::<String>::new());
        assert_eq!(strip_flags(&s(&["a", "--", "b"]), &flags), s(&["a"]));
    }
}
