//! JSON as linearctl has always printed it (the Go implementation's
//! encoding/json): object keys sorted, `<`, `>`, `&`, U+2028 and U+2029
//! escaped, numbers as float64 in the shortest form, and the same messages
//! for input that does not parse. Agents compare output byte for byte across
//! versions, so this is part of the interface.

use serde_json::{Map, Value};

/// Writes v as one JSON document followed by a newline: indented by two
/// spaces when `indent`, compact otherwise. Object keys are sorted and
/// numbers written as Go writes a float64, except in the subtrees of the
/// top-level keys named in `raw`, which are passed through as they came:
/// keys in their order, numbers as written (Go's json.RawMessage).
pub fn encode(v: &Value, indent: bool, raw: &[&str]) -> String {
    let mut out = String::new();
    let mut e = Encoder { out: &mut out, indent };
    match v {
        Value::Object(m) if !raw.is_empty() => e.object(m, 0, false, raw),
        _ => e.value(v, 0, false),
    }
    out.push('\n');
    out
}

/// One line of compact JSON, as Go's json.Marshal writes it (no newline).
pub fn marshal(v: &Value) -> String {
    let mut s = encode(v, false, &[]);
    s.pop();
    s
}

struct Encoder<'a> {
    out: &'a mut String,
    indent: bool,
}

impl Encoder<'_> {
    fn newline(&mut self, depth: usize) {
        if self.indent {
            self.out.push('\n');
            for _ in 0..depth {
                self.out.push_str("  ");
            }
        }
    }

    fn value(&mut self, v: &Value, depth: usize, raw: bool) {
        match v {
            Value::Null => self.out.push_str("null"),
            Value::Bool(b) => self.out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) if raw => self.out.push_str(&n.to_string()),
            Value::Number(n) => self.out.push_str(&number(n)),
            Value::String(s) => string(self.out, s),
            Value::Array(a) => {
                if a.is_empty() {
                    self.out.push_str("[]");
                    return;
                }
                self.out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        self.out.push(',');
                    }
                    self.newline(depth + 1);
                    self.value(x, depth + 1, raw);
                }
                self.newline(depth);
                self.out.push(']');
            }
            Value::Object(m) => self.object(m, depth, raw, &[]),
        }
    }

    fn object(&mut self, m: &Map<String, Value>, depth: usize, raw: bool, raw_keys: &[&str]) {
        if m.is_empty() {
            self.out.push_str("{}");
            return;
        }
        let mut entries: Vec<(&String, &Value)> = m.iter().collect();
        if !raw {
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        }
        self.out.push('{');
        for (i, (k, x)) in entries.into_iter().enumerate() {
            if i > 0 {
                self.out.push(',');
            }
            self.newline(depth + 1);
            string(self.out, k);
            self.out.push(':');
            if self.indent {
                self.out.push(' ');
            }
            self.value(x, depth + 1, raw || raw_keys.contains(&k.as_str()));
        }
        self.newline(depth);
        self.out.push('}');
    }
}

/// A string as Go quotes it in JSON.
pub fn string(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' | '\u{0}'..='\u{1f}' => {
                let n = c as u32;
                out.push_str("\\u");
                for shift in [12, 8, 4, 0] {
                    out.push(HEX[((n >> shift) & 0xf) as usize] as char);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A number as Go prints a float64 in JSON: every JSON number decodes to
/// one there, so a large integer loses precision as it did before.
pub fn number(n: &serde_json::Number) -> String {
    if n.to_string().starts_with('-') && n.as_f64() == Some(0.0) {
        return "-0".into();
    }
    let f = match (n.as_i64(), n.as_u64()) {
        (Some(i), _) if i.unsigned_abs() < (1 << 53) => return i.to_string(),
        (_, Some(u)) if u < (1 << 53) => return u.to_string(),
        _ => n.as_f64().unwrap_or(0.0),
    };
    float(f)
}

/// A float64 as encoding/json writes it.
pub fn float(f: f64) -> String {
    let abs = f.abs();
    if abs == 0.0 || (1e-6..1e21).contains(&abs) {
        return format!("{f}");
    }
    exponent(&format!("{f:e}"))
}

/// A float64 as fmt.Sprint writes it (%v): plain up to 1e21, else with an
/// exponent; used where Go formatted a decoded number as text.
pub fn sprint_float(f: f64) -> String {
    let abs = f.abs();
    if abs == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let exp = abs.log10().floor() as i32;
    if (-4..21).contains(&exp) {
        return format!("{f}");
    }
    let e = exponent(&format!("{f:e}"));
    // %v keeps two exponent digits: 1e-05, 1e+21.
    match e.find('e') {
        Some(i) => {
            let (m, rest) = e.split_at(i + 2);
            if rest.len() < 2 { format!("{m}0{rest}") } else { e.clone() }
        }
        None => e,
    }
}

/// Rust's `1e21` / `1.5e-7` as Go's `1e+21` / `1.5e-7`.
fn exponent(s: &str) -> String {
    match s.split_once('e') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
        _ => s.to_string(),
    }
}

// --- decoding -------------------------------------------------------------

/// Parses data as Go's json.Unmarshal would, with its messages for input
/// that is not JSON. Objects keep their order.
pub fn parse(data: &[u8]) -> Result<Value, String> {
    check(data)?;
    serde_json::from_slice(data).map_err(|e| e.to_string())
}

/// What Go's decoder calls a value's kind in a type error.
pub fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Go's message for a value of the wrong kind: at the top (field "") or in
/// a field of a struct or map.
pub fn type_error(v: &Value, field: &str, go_type: &str) -> String {
    if field.is_empty() {
        format!("json: cannot unmarshal {} into Go value of type {go_type}", kind(v))
    } else {
        format!("json: cannot unmarshal {} into Go struct field .{field} of type {go_type}", kind(v))
    }
}

/// Go's syntax check (encoding/json's scanner): the first error it reports.
fn check(data: &[u8]) -> Result<(), String> {
    let mut s = Scanner { b: data, i: 0 };
    s.ws();
    s.value()?;
    s.ws();
    if s.i < s.b.len() {
        return Err(s.invalid("after top-level value"));
    }
    Ok(())
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

const EOF: &str = "unexpected end of JSON input";

impl Scanner<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Result<u8, String> {
        self.b.get(self.i).copied().ok_or_else(|| EOF.to_string())
    }

    fn invalid(&self, context: &str) -> String {
        let c = self.b[self.i];
        let q = match c {
            b'\'' => "'\\''".to_string(),
            b'"' => "'\"'".to_string(),
            _ => {
                let quoted = quote_go(&char::from(c).to_string());
                format!("'{}'", &quoted[1..quoted.len() - 1])
            }
        };
        format!("invalid character {q} {context}")
    }

    fn value(&mut self) -> Result<(), String> {
        match self.peek()? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => self.string(),
            b'-' | b'0'..=b'9' => self.number(),
            b't' => self.literal(b"true"),
            b'f' => self.literal(b"false"),
            b'n' => self.literal(b"null"),
            _ => Err(self.invalid("looking for beginning of value")),
        }
    }

    fn object(&mut self) -> Result<(), String> {
        self.i += 1;
        self.ws();
        if self.peek()? == b'}' {
            self.i += 1;
            return Ok(());
        }
        loop {
            if self.peek()? != b'"' {
                return Err(self.invalid("looking for beginning of object key string"));
            }
            self.string()?;
            self.ws();
            if self.peek()? != b':' {
                return Err(self.invalid("after object key"));
            }
            self.i += 1;
            self.ws();
            self.value()?;
            self.ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b'}' => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.invalid("after object key:value pair")),
            }
        }
    }

    fn array(&mut self) -> Result<(), String> {
        self.i += 1;
        self.ws();
        if self.peek()? == b']' {
            self.i += 1;
            return Ok(());
        }
        loop {
            self.value()?;
            self.ws();
            match self.peek()? {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b']' => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.invalid("after array element")),
            }
        }
    }

    fn string(&mut self) -> Result<(), String> {
        self.i += 1;
        loop {
            match self.peek()? {
                b'"' => {
                    self.i += 1;
                    return Ok(());
                }
                b'\\' => {
                    self.i += 1;
                    match self.peek()? {
                        b'b' | b'f' | b'n' | b'r' | b't' | b'\\' | b'/' | b'"' => self.i += 1,
                        b'u' => {
                            self.i += 1;
                            for _ in 0..4 {
                                if !self.peek()?.is_ascii_hexdigit() {
                                    return Err(self.invalid("in \\u hexadecimal character escape"));
                                }
                                self.i += 1;
                            }
                        }
                        _ => return Err(self.invalid("in string escape code")),
                    }
                }
                c if c < 0x20 => return Err(self.invalid("in string literal")),
                _ => self.i += 1,
            }
        }
    }

    fn digits(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
            self.i += 1;
        }
    }

    fn number(&mut self) -> Result<(), String> {
        if self.b[self.i] == b'-' {
            self.i += 1;
            if !self.peek()?.is_ascii_digit() {
                return Err(self.invalid("in numeric literal"));
            }
        }
        if self.b[self.i] == b'0' {
            self.i += 1;
        } else {
            self.digits();
        }
        if self.i < self.b.len() && self.b[self.i] == b'.' {
            self.i += 1;
            if !self.peek()?.is_ascii_digit() {
                return Err(self.invalid("after decimal point in numeric literal"));
            }
            self.digits();
        }
        if self.i < self.b.len() && matches!(self.b[self.i], b'e' | b'E') {
            self.i += 1;
            if matches!(self.peek()?, b'+' | b'-') {
                self.i += 1;
            }
            if !self.peek()?.is_ascii_digit() {
                return Err(self.invalid("in exponent of numeric literal"));
            }
            self.digits();
        }
        Ok(())
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), String> {
        self.i += 1;
        for &want in &word[1..] {
            if self.peek()? != want {
                let name = String::from_utf8_lossy(word);
                return Err(self.invalid(&format!("in literal {name} (expecting '{}')", want as char)));
            }
            self.i += 1;
        }
        Ok(())
    }
}

/// Go's strconv.Quote: a double-quoted string with Go escapes.
pub fn quote_go(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            c if is_go_printable(c) => out.push(c),
            c if (c as u32) < 0x80 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
}

/// Go's strconv.IsPrint, near enough: not a control or format character,
/// nor a separator other than the ASCII space.
fn is_go_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(c as u32, 0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x180e | 0x200b..=0x200f
        | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f | 0xfeff | 0xfff9..=0xfffb | 0xe000..=0xf8ff
        | 0xfff0..=0xfff8 | 0xfffe | 0xffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encodes_as_go_does() {
        let v = json!({"b": 1, "a": "<x> & \u{2028}\u{1b}\"\n", "c": [], "d": {}, "e": [1.5, 1e21, 1e-7, 2.0, -0.0, 12345678901234567890u64]});
        assert_eq!(
            marshal(&v),
            r#"{"a":"\u003cx\u003e \u0026 \u2028\u001b\"\n","b":1,"c":[],"d":{},"e":[1.5,1e+21,1e-7,2,-0,12345678901234567000]}"#
        );
        assert_eq!(encode(&json!({"a": [1, {}], "b": {"c": null}}), true, &[]),
            "{\n  \"a\": [\n    1,\n    {}\n  ],\n  \"b\": {\n    \"c\": null\n  }\n}\n");
    }

    #[test]
    fn passes_raw_subtrees_through() {
        let v: Value = serde_json::from_str(r#"{"z": {"b": 1.0, "a": 2}, "user": {"name": "n", "id": 1.0}}"#).unwrap();
        assert_eq!(encode(&v, false, &["user"]), "{\"user\":{\"name\":\"n\",\"id\":1.0},\"z\":{\"a\":2,\"b\":1}}\n");
    }

    #[test]
    fn sprint_as_go() {
        assert_eq!(sprint_float(42.0), "42");
        assert_eq!(sprint_float(0.5), "0.5");
        assert_eq!(sprint_float(1e21), "1e+21");
        assert_eq!(sprint_float(1e-5), "1e-05");
    }

    #[test]
    fn syntax_errors_as_go() {
        for (input, want) in [
            ("", "unexpected end of JSON input"),
            ("nope", "invalid character 'o' in literal null (expecting 'u')"),
            ("{", "unexpected end of JSON input"),
            ("{1}", "invalid character '1' looking for beginning of object key string"),
            ("{\"a\" 1}", "invalid character '1' after object key"),
            ("{\"a\":1 2}", "invalid character '2' after object key:value pair"),
            ("[1 2]", "invalid character '2' after array element"),
            ("[1,]", "invalid character ']' looking for beginning of value"),
            ("01", "invalid character '1' after top-level value"),
            ("-x", "invalid character 'x' in numeric literal"),
            ("1.x", "invalid character 'x' after decimal point in numeric literal"),
            ("1ex", "invalid character 'x' in exponent of numeric literal"),
            ("\"\\x\"", "invalid character 'x' in string escape code"),
            ("\"\\u12x4\"", "invalid character 'x' in \\u hexadecimal character escape"),
            ("\"a\nb\"", "invalid character '\\n' in string literal"),
            ("'", "invalid character '\\'' looking for beginning of value"),
            ("{} x", "invalid character 'x' after top-level value"),
        ] {
            assert_eq!(check(input.as_bytes()).unwrap_err(), want, "{input:?}");
        }
        assert!(check(b" {\"a\": [1, -0.5e+3, true, false, null, \"\\u00e9\"]} ").is_ok());
    }
}
