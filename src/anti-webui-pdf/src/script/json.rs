//! What goes between Rust and the scripts: JSON, as text. It is all the engine has to pass, so
//! another engine only has to pass text too.

use std::fmt::Write;

/// How deep what is read can go, as nothing that is sent goes deeper than a few levels.
const DEPTH: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn object(entries: impl IntoIterator<Item = (&'static str, Json)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v))
                .collect(),
        )
    }

    pub fn str(text: impl Into<String>) -> Self {
        Self::String(text.into())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_usize(&self) -> Option<usize> {
        self.as_f64()
            .filter(|n| *n >= 0.0 && n.fract() == 0.0)
            .map(|n| n as usize)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Number(n) if n.is_finite() => {
                let _ = write!(out, "{n}");
            }
            Self::Number(_) => out.push_str("null"),
            Self::String(s) => write_string(s, out),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(entries) => {
                out.push('{');
                for (i, (key, value)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut reader = Reader {
            bytes: text.as_bytes(),
            at: 0,
        };
        let value = reader.value(DEPTH)?;
        reader.space();
        if reader.at != reader.bytes.len() {
            return Err(format!("something after the JSON at {}", reader.at));
        }
        Ok(value)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Self::String(s.to_owned())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<usize> for Json {
    fn from(n: usize) -> Self {
        Self::Number(n as f64)
    }
}

impl From<f64> for Json {
    fn from(n: f64) -> Self {
        Self::Number(n)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(o: Option<T>) -> Self {
        o.map_or(Self::Null, Into::into)
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // These end a line in JavaScript, so they are written out as well.
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn expect(&mut self, word: &str) -> Result<(), String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(format!("expected {word} at {}", self.at))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth == 0 {
            return Err("JSON that goes too deep".to_owned());
        }
        self.space();
        match self.bytes.get(self.at) {
            Some(b'n') => self.expect("null").map(|()| Json::Null),
            Some(b't') => self.expect("true").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect("false").map(|()| Json::Bool(false)),
            Some(b'"') => self.string().map(Json::String),
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth - 1)?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return Err(format!("expected , or ] at {}", self.at)),
                    }
                }
            }
            Some(b'{') => {
                self.at += 1;
                let mut entries = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.space();
                    self.expect(":")?;
                    entries.push((key, self.value(depth - 1)?));
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Object(entries));
                        }
                        _ => return Err(format!("expected , or }} at {}", self.at)),
                    }
                }
            }
            Some(_) => self.number(),
            None => Err("JSON that ends too soon".to_owned()),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map(Json::Number)
            .ok_or_else(|| format!("not a number at {start}"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.at..self.at + 4)
            .and_then(|d| std::str::from_utf8(d).ok())
            .and_then(|d| u32::from_str_radix(d, 16).ok())
            .ok_or_else(|| format!("a broken \\u at {}", self.at))?;
        self.at += 4;
        Ok(digits)
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect("\"")?;
        let mut out = String::new();
        loop {
            let start = self.at;
            while self
                .bytes
                .get(self.at)
                .is_some_and(|b| *b != b'"' && *b != b'\\')
            {
                self.at += 1;
            }
            out.push_str(
                std::str::from_utf8(&self.bytes[start..self.at]).map_err(|e| e.to_string())?,
            );
            match self.bytes.get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.at += 1;
                    let escaped = *self.bytes.get(self.at).ok_or("a string that ends")?;
                    self.at += 1;
                    match escaped {
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&first)
                                && self.bytes[self.at..].starts_with(b"\\u")
                            {
                                self.at += 2;
                                let second = self.hex4()?;
                                0x10000 + ((first - 0xd800) << 10) + (second.wrapping_sub(0xdc00))
                            } else {
                                first
                            };
                            out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
                        }
                        other => out.push(other as char),
                    }
                }
                _ => return Err("a string that doesn't end".to_owned()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_reads_back() {
        let value = Json::object([
            ("a", Json::Number(1.5)),
            ("b", Json::str("日本\n\"x\"\u{1}")),
            (
                "c",
                Json::Array(vec![Json::Null, Json::Bool(true), Json::Number(-3.0)]),
            ),
            ("d", Json::Object(Vec::new())),
        ]);
        assert_eq!(Json::parse(&value.text()), Ok(value));
    }

    #[test]
    fn what_javascript_writes_reads() {
        let value = Json::parse(r#" {"t":"x","s":"😀 é","n":[1e3, -0.5]} "#).unwrap();
        assert_eq!(value.get("s").and_then(Json::as_str), Some("😀 é"));
        assert_eq!(
            value
                .get("n")
                .and_then(Json::as_array)
                .map(|n| n[0].as_f64()),
            Some(Some(1000.0))
        );
        assert!(Json::parse("[1,").is_err());
        assert!(Json::parse(&"[".repeat(100)).is_err());
    }
}
