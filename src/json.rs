//! A small JSON reader and writer for the preset file.
//!
//! The file is user-editable, so parsing is strict and bounded: nesting is limited,
//! trailing text is rejected, and strings may not contain raw control characters.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    /// Members keep their file order.
    Object(Vec<(String, Json)>),
}

const MAX_DEPTH: usize = 16;

impl Json {
    pub fn parse(text: &str) -> Option<Json> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut parser = Parser {
            text,
            bytes: text.as_bytes(),
            pos: 0,
        };
        parser.skip_whitespace();
        let value = parser.value(0)?;
        parser.skip_whitespace();
        (parser.pos == parser.bytes.len()).then_some(value)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(number) => Some(*number),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Serializes with two-space indentation.
    pub fn write(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out, 0);
        out.push('\n');
        out
    }

    fn write_into(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, depth: usize| out.push_str(&"  ".repeat(depth));
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
            Json::Number(number) if number.is_finite() => {
                if number.fract() == 0.0 && number.abs() < 1e15 {
                    out.push_str(&format!("{}", *number as i64));
                } else {
                    out.push_str(&format!("{number}"));
                }
            }
            Json::Number(_) => out.push_str("null"),
            Json::String(text) => write_string(out, text),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Array(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, depth + 1);
                    item.write_into(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push(']');
            }
            Json::Object(members) if members.is_empty() => out.push_str("{}"),
            Json::Object(members) => {
                out.push_str("{\n");
                for (i, (key, value)) in members.iter().enumerate() {
                    pad(out, depth + 1);
                    write_string(out, key);
                    out.push_str(": ");
                    value.write_into(out, depth + 1);
                    out.push_str(if i + 1 < members.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.bytes.get(self.pos) == Some(&byte)).then(|| self.pos += 1)
    }

    fn literal(&mut self, word: &str, value: Json) -> Option<Json> {
        self.text[self.pos..].starts_with(word).then(|| {
            self.pos += word.len();
            value
        })
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        match *self.bytes.get(self.pos)? {
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'"' => self.string().map(Json::String),
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                self.skip_whitespace();
                if self.eat(b']').is_some() {
                    return Some(Json::Array(items));
                }
                loop {
                    self.skip_whitespace();
                    items.push(self.value(depth + 1)?);
                    self.skip_whitespace();
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b']')?;
                    return Some(Json::Array(items));
                }
            }
            b'{' => {
                self.pos += 1;
                let mut members = Vec::new();
                self.skip_whitespace();
                if self.eat(b'}').is_some() {
                    return Some(Json::Object(members));
                }
                loop {
                    self.skip_whitespace();
                    let key = self.string()?;
                    self.skip_whitespace();
                    self.eat(b':')?;
                    self.skip_whitespace();
                    members.push((key, self.value(depth + 1)?));
                    self.skip_whitespace();
                    if self.eat(b',').is_some() {
                        continue;
                    }
                    self.eat(b'}')?;
                    return Some(Json::Object(members));
                }
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.pos;
        while matches!(
            self.bytes.get(self.pos),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        let number: f64 = self.text[start..self.pos].parse().ok()?;
        number.is_finite().then_some(Json::Number(number))
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.text.get(self.pos..self.pos + 4)?;
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.pos += 4;
        u32::from_str_radix(digits, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let start = self.pos;
            // Delimiters are ASCII, so these slices always fall on character boundaries.
            while let Some(&byte) = self.bytes.get(self.pos) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(&self.text[start..self.pos]);
            match *self.bytes.get(self.pos)? {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let escape = *self.bytes.get(self.pos)?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&first) {
                                // A high surrogate must be followed by a low one.
                                self.eat(b'\\')?;
                                self.eat(b'u')?;
                                let second = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&second) {
                                    return None;
                                }
                                0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                            } else {
                                first
                            };
                            out.push(char::from_u32(code)?);
                        }
                        _ => return None,
                    }
                }
                // A raw control character inside a string.
                _ => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_nested_values_and_escapes() {
        let text =
            r#"{"a": [1, 2.5, -3e2, true, null], "s": "q\"\\\n\u00e9\ud83d\ude00", "o": {}}"#;
        let value = Json::parse(text).unwrap();
        assert_eq!(
            value.get("s").unwrap().as_str(),
            Some("q\"\\\n\u{e9}\u{1f600}")
        );
        assert_eq!(Json::parse(&value.write()), Some(value));
    }

    #[test]
    fn rejects_malformed_and_hostile_input() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "[1] x",
            "\"raw\ttab\"",
            "\"\\ud800\"",
            "\"\\q\"",
            "[1e999]",
        ] {
            assert_eq!(Json::parse(bad), None, "{bad}");
        }
        let deep = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert_eq!(Json::parse(&deep), None);
    }
}
