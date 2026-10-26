//! A parser for GraphQL executable documents: what `api query` checks (is
//! it a query?) and what demo mode answers. Errors read as gqlparser's did:
//! "input:LINE:COL: Expected Name, found <EOF>".

use serde_json::{Map, Value};

#[derive(Debug)]
pub struct Document {
    pub operations: Vec<Operation>,
}

#[derive(Debug)]
pub struct Operation {
    /// query, mutation or subscription.
    pub kind: String,
    pub selections: Vec<Selection>,
}

#[derive(Debug)]
pub enum Selection {
    Field(Field),
    Inline { on: Option<String>, selections: Vec<Selection> },
    Spread,
}

#[derive(Debug)]
pub struct Field {
    pub alias: Option<String>,
    pub name: String,
    pub args: Vec<(String, Lit)>,
    pub directives: Vec<Directive>,
    pub selections: Vec<Selection>,
}

impl Field {
    pub fn key(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug)]
pub struct Directive {
    pub name: String,
    pub args: Vec<(String, Lit)>,
}

/// A value as written in the document.
#[derive(Debug)]
pub enum Lit {
    Var(String),
    Int(String),
    Float(String),
    Str(String),
    Bool(bool),
    Null,
    Enum(String),
    List(Vec<Lit>),
    Object(Vec<(String, Lit)>),
}

impl Lit {
    /// The value, with variables from vars; an absent variable is null.
    pub fn value(&self, vars: &Value) -> Value {
        match self {
            Lit::Var(n) => vars.get(n).cloned().unwrap_or(Value::Null),
            Lit::Int(s) | Lit::Float(s) => serde_json::from_str(s).unwrap_or(Value::Null),
            Lit::Str(s) | Lit::Enum(s) => Value::String(s.clone()),
            Lit::Bool(b) => Value::Bool(*b),
            Lit::Null => Value::Null,
            Lit::List(l) => Value::Array(l.iter().map(|x| x.value(vars)).collect()),
            Lit::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), x.value(vars))).collect::<Map<_, _>>()),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Tok {
    Punct(&'static str),
    Name(String),
    Int(String),
    Float(String),
    Str(String),
    BlockStr(String),
    Eof,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Punct(p) => (*p).to_string(),
            Tok::Name(v) => format!("Name {}", crate::json::quote_go(v)),
            Tok::Int(v) => format!("Int {}", crate::json::quote_go(v)),
            Tok::Float(v) => format!("Float {}", crate::json::quote_go(v)),
            Tok::Str(v) => format!("String {}", crate::json::quote_go(v)),
            Tok::BlockStr(v) => format!("BlockString {}", crate::json::quote_go(v)),
            Tok::Eof => "<EOF>".into(),
        }
    }
}

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
    line_start: usize,
}

type Res<T> = Result<T, String>;

impl Lexer<'_> {
    fn err(&self, at: usize, line: usize, line_start: usize, msg: &str) -> String {
        format!("input:{line}:{}: {msg}", at - line_start + 1)
    }

    /// The next token and where it starts (offset, line, line start).
    fn next(&mut self) -> Res<(Tok, usize, usize, usize)> {
        loop {
            match self.s.get(self.i) {
                Some(b'\n') => {
                    self.i += 1;
                    self.line += 1;
                    self.line_start = self.i;
                }
                Some(b'\r') => {
                    self.i += 1;
                    if self.s.get(self.i) == Some(&b'\n') {
                        self.i += 1;
                    }
                    self.line += 1;
                    self.line_start = self.i;
                }
                Some(b' ' | b'\t' | b',') => self.i += 1,
                Some(0xef) if self.s[self.i..].starts_with(&[0xef, 0xbb, 0xbf]) => self.i += 3,
                Some(b'#') => {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' && self.s[self.i] != b'\r' {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
        let (start, line, ls) = (self.i, self.line, self.line_start);
        let Some(&c) = self.s.get(self.i) else { return Ok((Tok::Eof, start, line, ls)) };
        let punct = match c {
            b'!' => Some("!"),
            b'$' => Some("$"),
            b'&' => Some("&"),
            b'(' => Some("("),
            b')' => Some(")"),
            b':' => Some(":"),
            b'=' => Some("="),
            b'@' => Some("@"),
            b'[' => Some("["),
            b']' => Some("]"),
            b'{' => Some("{"),
            b'|' => Some("|"),
            b'}' => Some("}"),
            _ => None,
        };
        if let Some(p) = punct {
            self.i += 1;
            return Ok((Tok::Punct(p), start, line, ls));
        }
        if c == b'.' {
            if self.s[self.i..].starts_with(b"...") {
                self.i += 3;
                return Ok((Tok::Punct("..."), start, line, ls));
            }
            return Err(self.err(start, line, ls, "Unexpected character \".\""));
        }
        if c == b'_' || c.is_ascii_alphabetic() {
            while self.i < self.s.len() && (self.s[self.i] == b'_' || self.s[self.i].is_ascii_alphanumeric()) {
                self.i += 1;
            }
            let v = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
            return Ok((Tok::Name(v), start, line, ls));
        }
        if c == b'-' || c.is_ascii_digit() {
            return self.number(start, line, ls);
        }
        if c == b'"' {
            if self.s[self.i..].starts_with(b"\"\"\"") {
                return self.block_string(start, line, ls);
            }
            return self.string(start, line, ls);
        }
        let ch = String::from_utf8_lossy(&self.s[start..(start + 1).min(self.s.len())]).into_owned();
        Err(self.err(start, line, ls, &format!("Unexpected character {}", crate::json::quote_go(&ch))))
    }

    fn digits(&mut self) -> usize {
        let from = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        self.i - from
    }

    fn number(&mut self, start: usize, line: usize, ls: usize) -> Res<(Tok, usize, usize, usize)> {
        let bad = |l: &Self| l.err(l.i, line, ls, "Invalid number");
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        if self.s.get(self.i) == Some(&b'0') {
            self.i += 1;
            if self.s.get(self.i).is_some_and(u8::is_ascii_digit) {
                return Err(bad(self));
            }
        } else if self.digits() == 0 {
            return Err(bad(self));
        }
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') {
            float = true;
            self.i += 1;
            if self.digits() == 0 {
                return Err(bad(self));
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            float = true;
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(bad(self));
            }
        }
        let v = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
        Ok((if float { Tok::Float(v) } else { Tok::Int(v) }, start, line, ls))
    }

    fn string(&mut self, start: usize, line: usize, ls: usize) -> Res<(Tok, usize, usize, usize)> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return Err(self.err(self.i, line, ls, "Unterminated string.")) };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok((Tok::Str(out), start, line, ls));
                }
                b'\n' | b'\r' => return Err(self.err(self.i, line, ls, "Unterminated string.")),
                b'\\' => {
                    let esc = self.s.get(self.i + 1).copied();
                    self.i += 2;
                    match esc {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'/') => out.push('/'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'u') => {
                            let hex = self.s.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok());
                            let Some(c) = hex.and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) else {
                                return Err(self.err(self.i - 2, line, ls, "Invalid character escape sequence"));
                            };
                            out.push(c);
                            self.i += 4;
                        }
                        _ => return Err(self.err(self.i - 2, line, ls, "Invalid character escape sequence")),
                    }
                }
                _ => {
                    let rest = std::str::from_utf8(&self.s[self.i..]).unwrap_or("");
                    let ch = rest.chars().next().unwrap_or('\u{fffd}');
                    out.push(ch);
                    self.i += ch.len_utf8().max(1);
                }
            }
        }
    }

    fn block_string(&mut self, start: usize, line: usize, ls: usize) -> Res<(Tok, usize, usize, usize)> {
        self.i += 3;
        let from = self.i;
        loop {
            if self.i >= self.s.len() {
                return Err(self.err(self.i, line, ls, "Unterminated string."));
            }
            if self.s[self.i..].starts_with(b"\\\"\"\"") {
                self.i += 4;
                continue;
            }
            if self.s[self.i..].starts_with(b"\"\"\"") {
                let raw = String::from_utf8_lossy(&self.s[from..self.i]).replace("\\\"\"\"", "\"\"\"");
                self.i += 3;
                return Ok((Tok::BlockStr(raw), start, line, ls));
            }
            if self.s[self.i] == b'\n' {
                self.line += 1;
                self.line_start = self.i + 1;
            }
            self.i += 1;
        }
    }
}

struct Parser<'a> {
    lex: Lexer<'a>,
    tok: Tok,
    at: (usize, usize, usize),
}

/// Parses an executable document: operations and fragments.
pub fn parse(src: &str) -> Res<Document> {
    let mut lex = Lexer { s: src.as_bytes(), i: 0, line: 1, line_start: 0 };
    let (tok, a, b, c) = lex.next()?;
    let mut p = Parser { lex, tok, at: (a, b, c) };
    let mut operations = vec![];
    if p.tok == Tok::Eof {
        return Err(p.unexpected());
    }
    while p.tok != Tok::Eof {
        match p.tok.clone() {
            Tok::Punct("{") => operations.push(Operation { kind: "query".into(), selections: p.selection_set()? }),
            Tok::Name(n) if n == "query" || n == "mutation" || n == "subscription" => {
                p.advance()?;
                if let Tok::Name(_) = p.tok {
                    p.advance()?;
                }
                if p.tok == Tok::Punct("(") {
                    p.variable_definitions()?;
                }
                p.directives()?;
                operations.push(Operation { kind: n, selections: p.selection_set()? });
            }
            Tok::Name(n) if n == "fragment" => {
                p.advance()?;
                p.name()?;
                p.expect_keyword("on")?;
                p.name()?;
                p.directives()?;
                p.selection_set()?;
            }
            _ => return Err(p.unexpected()),
        }
    }
    Ok(Document { operations })
}

impl Parser<'_> {
    fn advance(&mut self) -> Res<()> {
        let (t, a, b, c) = self.lex.next()?;
        self.tok = t;
        self.at = (a, b, c);
        Ok(())
    }

    fn error(&self, msg: &str) -> String {
        let (at, line, ls) = self.at;
        self.lex.err(at, line, ls, msg)
    }

    fn unexpected(&self) -> String {
        self.error(&format!("Unexpected {}", self.tok.describe()))
    }

    fn expect(&mut self, p: &'static str) -> Res<()> {
        if self.tok != Tok::Punct(p) {
            return Err(self.error(&format!("Expected {p}, found {}", self.tok.describe())));
        }
        self.advance()
    }

    fn name(&mut self) -> Res<String> {
        match self.tok.clone() {
            Tok::Name(n) => {
                self.advance()?;
                Ok(n)
            }
            t => Err(self.error(&format!("Expected Name, found {}", t.describe()))),
        }
    }

    fn expect_keyword(&mut self, k: &str) -> Res<()> {
        match &self.tok {
            Tok::Name(n) if n == k => self.advance(),
            t => Err(self.error(&format!("Expected \"{k}\", found {}", t.describe()))),
        }
    }

    fn variable_definitions(&mut self) -> Res<()> {
        self.expect("(")?;
        while self.tok != Tok::Punct(")") {
            self.expect("$")?;
            self.name()?;
            self.expect(":")?;
            self.type_ref()?;
            if self.tok == Tok::Punct("=") {
                self.advance()?;
                self.value(true)?;
            }
            self.directives()?;
        }
        self.advance()
    }

    fn type_ref(&mut self) -> Res<()> {
        if self.tok == Tok::Punct("[") {
            self.advance()?;
            self.type_ref()?;
            self.expect("]")?;
        } else {
            self.name()?;
        }
        if self.tok == Tok::Punct("!") {
            self.advance()?;
        }
        Ok(())
    }

    fn directives(&mut self) -> Res<Vec<Directive>> {
        let mut out = vec![];
        while self.tok == Tok::Punct("@") {
            self.advance()?;
            let name = self.name()?;
            let args = if self.tok == Tok::Punct("(") { self.arguments()? } else { vec![] };
            out.push(Directive { name, args });
        }
        Ok(out)
    }

    fn arguments(&mut self) -> Res<Vec<(String, Lit)>> {
        self.expect("(")?;
        let mut out = vec![];
        loop {
            let name = self.name()?;
            self.expect(":")?;
            out.push((name, self.value(false)?));
            if self.tok == Tok::Punct(")") {
                break;
            }
        }
        self.advance()?;
        Ok(out)
    }

    fn value(&mut self, constant: bool) -> Res<Lit> {
        let v = match self.tok.clone() {
            Tok::Punct("$") if !constant => {
                self.advance()?;
                return Ok(Lit::Var(self.name()?));
            }
            Tok::Punct("[") => {
                self.advance()?;
                let mut l = vec![];
                while self.tok != Tok::Punct("]") {
                    l.push(self.value(constant)?);
                }
                self.advance()?;
                return Ok(Lit::List(l));
            }
            Tok::Punct("{") => {
                self.advance()?;
                let mut o = vec![];
                while self.tok != Tok::Punct("}") {
                    let k = self.name()?;
                    self.expect(":")?;
                    o.push((k, self.value(constant)?));
                }
                self.advance()?;
                return Ok(Lit::Object(o));
            }
            Tok::Int(s) => Lit::Int(s),
            Tok::Float(s) => Lit::Float(s),
            Tok::Str(s) | Tok::BlockStr(s) => Lit::Str(s),
            Tok::Name(n) => match n.as_str() {
                "true" => Lit::Bool(true),
                "false" => Lit::Bool(false),
                "null" => Lit::Null,
                _ => Lit::Enum(n),
            },
            _ => return Err(self.unexpected()),
        };
        self.advance()?;
        Ok(v)
    }

    fn selection_set(&mut self) -> Res<Vec<Selection>> {
        self.expect("{")?;
        let mut out = vec![];
        loop {
            out.push(self.selection()?);
            if self.tok == Tok::Punct("}") {
                break;
            }
        }
        self.advance()?;
        Ok(out)
    }

    fn selection(&mut self) -> Res<Selection> {
        if self.tok == Tok::Punct("...") {
            self.advance()?;
            let on = match &self.tok {
                Tok::Name(n) if n == "on" => {
                    self.advance()?;
                    Some(self.name()?)
                }
                Tok::Name(_) => {
                    self.name()?;
                    self.directives()?;
                    return Ok(Selection::Spread);
                }
                _ => None,
            };
            self.directives()?;
            let selections = self.selection_set()?;
            return Ok(Selection::Inline { on, selections });
        }
        let mut name = self.name()?;
        let mut alias = None;
        if self.tok == Tok::Punct(":") {
            self.advance()?;
            alias = Some(std::mem::replace(&mut name, self.name()?));
        }
        let args = if self.tok == Tok::Punct("(") { self.arguments()? } else { vec![] };
        let directives = self.directives()?;
        let selections = if self.tok == Tok::Punct("{") { self.selection_set()? } else { vec![] };
        Ok(Selection::Field(Field { alias, name, args, directives, selections }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_linearctl_sends() {
        for (name, doc) in crate::queries::documents() {
            let d = parse(doc).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.operations.len(), 1, "{name}");
        }
        let d = parse("{ a: viewer { id } }").unwrap();
        assert_eq!(d.operations[0].kind, "query");
    }

    #[test]
    fn errors_as_gqlparser() {
        assert_eq!(parse("query {").unwrap_err(), "input:1:8: Expected Name, found <EOF>");
        assert_eq!(parse("").unwrap_err(), "input:1:1: Unexpected <EOF>");
        assert_eq!(parse("mutation { x }").unwrap().operations[0].kind, "mutation");
        assert!(parse("query { a(x: ) }").is_err());
    }

    #[test]
    fn values_with_variables() {
        let d = parse("query Q($n: Int!) { issues(first: $n, filter: { a: [1, \"x\", true, null, E] }) { id } }").unwrap();
        let Selection::Field(f) = &d.operations[0].selections[0] else { panic!() };
        let vars = serde_json::json!({"n": 3});
        assert_eq!(f.args[0].1.value(&vars), serde_json::json!(3));
        assert_eq!(f.args[1].1.value(&vars), serde_json::json!({"a": [1, "x", true, null, "E"]}));
    }
}
