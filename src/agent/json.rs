//! A very small JSON reader and writer.
//!
//! Hand-rolled for the same reason the PNG codec, the DEFLATE compressor and
//! the base64 encoder in this crate are (see `AGENTS.md`): this project takes
//! no new crate dependencies, and the emulator core has to keep building for
//! `wasm32` and for the FFI static library.
//!
//! It is deliberately minimal — enough to write a state snapshot and to read a
//! driver's reply — but the *reading* half is treated as parsing hostile input,
//! because the reply comes from a subprocess this crate did not write:
//!
//! * a hard input-size cap ([`MAX_INPUT`]),
//! * a hard nesting-depth cap ([`MAX_DEPTH`]), so a pathological
//!   `[[[[[…]]]]]` cannot blow the stack,
//! * no panics on any input: every failure is a [`ParseError`].

use std::fmt::Write as _;

/// Longest reply accepted from a driver. A model that returns a megabyte of
/// prose is malfunctioning, and buffering it unbounded is how a "cheap local
/// executor" becomes the thing that stalls.
pub const MAX_INPUT: usize = 256 * 1024;
/// Deepest nesting accepted. Real replies are two or three deep.
pub const MAX_DEPTH: usize = 24;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Json {
    /// Field of an object, or `None` for anything else.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A non-negative integer, clamped. Accepts a float that happens to be
    /// whole, because models write `3.0`.
    pub fn as_u64(&self) -> Option<u64> {
        let n = self.as_f64()?;
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        Some(n.round() as u64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }

    /// Convenience: a string field.
    pub fn str_field(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| v.as_str())
    }

    /// Convenience: a non-negative integer field.
    pub fn u64_field(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|v| v.as_u64())
    }
}

/// Parse one JSON value. The whole input must be that value (trailing
/// whitespace is allowed, trailing junk is not).
pub fn parse(input: &str) -> Result<Json, ParseError> {
    if input.len() > MAX_INPUT {
        return Err(ParseError(format!(
            "reply is {} bytes, over the {MAX_INPUT}-byte limit",
            input.len()
        )));
    }
    let bytes: Vec<char> = input.chars().collect();
    let mut p = Parser { s: &bytes, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(ParseError(format!(
            "unexpected trailing characters at offset {}",
            p.i
        )));
    }
    Ok(v)
}

/// Find and parse the first complete JSON object in `input`, ignoring anything
/// before or after it.
///
/// Models wrap JSON in prose and in ``` fences however firmly they are told not
/// to. Refusing such a reply outright turns a usable answer into a wasted call,
/// so the host is lenient *here* — and strict about everything inside the
/// object, where a wrong value would actually mislead.
pub fn parse_embedded(input: &str) -> Result<Json, ParseError> {
    if let Ok(v) = parse(input.trim()) {
        return Ok(v);
    }
    let chars: Vec<char> = input.chars().collect();
    if chars.len() > MAX_INPUT {
        return Err(ParseError("reply is too large".to_string()));
    }
    // Scan for a balanced object, honouring strings and escapes so a `}` inside
    // a quoted string cannot end it early.
    let mut start = None;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        let s: String = chars[start.unwrap_or(0)..=i].iter().collect();
                        if let Ok(v) = parse(&s) {
                            return Ok(v);
                        }
                        start = None;
                    }
                }
            }
            _ => {}
        }
    }
    Err(ParseError("no JSON object found in the reply".to_string()))
}

struct Parser<'a> {
    s: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> Result<(), ParseError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(ParseError(format!("expected '{c}' at offset {}", self.i)))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth > MAX_DEPTH {
            return Err(ParseError(format!("nested deeper than {MAX_DEPTH}")));
        }
        match self.peek() {
            None => Err(ParseError("unexpected end of input".to_string())),
            Some('{') => self.object(depth),
            Some('[') => self.array(depth),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') => self.literal("true", Json::Bool(true)),
            Some('f') => self.literal("false", Json::Bool(false)),
            Some('n') => self.literal("null", Json::Null),
            Some(_) => self.number(),
        }
    }

    fn literal(&mut self, word: &str, out: Json) -> Result<Json, ParseError> {
        for c in word.chars() {
            self.eat(c)?;
        }
        Ok(out)
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.eat('{')?;
        let mut fields = Vec::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(':')?;
            self.ws();
            let v = self.value(depth + 1)?;
            fields.push((k, v));
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => {
                    return Err(ParseError(format!(
                        "expected ',' or '}}' at offset {}",
                        self.i
                    )))
                }
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.eat('[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => {
                    return Err(ParseError(format!(
                        "expected ',' or ']' at offset {}",
                        self.i
                    )))
                }
            }
        }
    }

    fn string(&mut self) -> Result<String, ParseError> {
        self.eat('"')?;
        let mut out = String::new();
        loop {
            let c = self
                .peek()
                .ok_or_else(|| ParseError("unterminated string".to_string()))?;
            self.i += 1;
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = self
                        .peek()
                        .ok_or_else(|| ParseError("dangling escape".to_string()))?;
                    self.i += 1;
                    match e {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let hi = self.hex4()?;
                            // Surrogate pair, the one place a naive decoder
                            // produces mojibake instead of an error.
                            let ch = if (0xD800..0xDC00).contains(&hi) {
                                if self.peek() == Some('\\') {
                                    self.i += 1;
                                    self.eat('u')?;
                                    let lo = self.hex4()?;
                                    if !(0xDC00..0xE000).contains(&lo) {
                                        return Err(ParseError(
                                            "bad surrogate pair".to_string(),
                                        ));
                                    }
                                    let c =
                                        0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                    char::from_u32(c)
                                } else {
                                    None
                                }
                            } else {
                                char::from_u32(hi)
                            };
                            out.push(ch.unwrap_or('\u{fffd}'));
                        }
                        other => {
                            return Err(ParseError(format!("unknown escape '\\{other}'")))
                        }
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(ParseError("control character in string".to_string()))
                }
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self
                .peek()
                .ok_or_else(|| ParseError("short \\u escape".to_string()))?;
            let d = c
                .to_digit(16)
                .ok_or_else(|| ParseError("bad hex in \\u escape".to_string()))?;
            v = v * 16 + d;
            self.i += 1;
        }
        Ok(v)
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.i;
        if self.peek() == Some('-') || self.peek() == Some('+') {
            self.i += 1;
        }
        while let Some(c) = self.peek() {
            // A sign is part of the number only immediately after an exponent
            // marker; anywhere else it belongs to whatever comes next.
            let exponent_sign = (c == '-' || c == '+')
                && matches!(self.s.get(self.i - 1), Some('e') | Some('E'));
            if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || exponent_sign {
                self.i += 1;
            } else {
                break;
            }
        }
        let text: String = self.s[start..self.i].iter().collect();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| ParseError(format!("bad number '{text}'")))
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Append `s` as a quoted JSON string.
pub fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A minimal object/array builder that keeps the comma bookkeeping in one
/// place, so a hand-written encoder cannot emit `{"a":1,}`.
pub struct Writer {
    pub out: String,
    /// One flag per open container: has anything been written into it yet?
    stack: Vec<bool>,
}

impl Default for Writer {
    fn default() -> Self {
        Writer::new()
    }
}

impl Writer {
    pub fn new() -> Writer {
        Writer {
            out: String::with_capacity(1024),
            stack: Vec::new(),
        }
    }

    fn sep(&mut self) {
        if let Some(any) = self.stack.last_mut() {
            if *any {
                self.out.push(',');
            }
            *any = true;
        }
    }

    fn key(&mut self, k: &str) {
        self.sep();
        write_str(&mut self.out, k);
        self.out.push(':');
    }

    pub fn begin_obj(&mut self) {
        self.sep();
        self.out.push('{');
        self.stack.push(false);
    }

    pub fn end_obj(&mut self) {
        self.stack.pop();
        self.out.push('}');
    }

    pub fn begin_arr(&mut self) {
        self.sep();
        self.out.push('[');
        self.stack.push(false);
    }

    pub fn end_arr(&mut self) {
        self.stack.pop();
        self.out.push(']');
    }

    pub fn obj_field(&mut self, k: &str) {
        self.key(k);
        self.out.push('{');
        self.stack.push(false);
    }

    pub fn arr_field(&mut self, k: &str) {
        self.key(k);
        self.out.push('[');
        self.stack.push(false);
    }

    pub fn str_field(&mut self, k: &str, v: &str) {
        self.key(k);
        write_str(&mut self.out, v);
    }

    pub fn num_field(&mut self, k: &str, v: f64) {
        self.key(k);
        if v.is_finite() {
            if v == v.trunc() && v.abs() < 1e15 {
                let _ = write!(self.out, "{}", v as i64);
            } else {
                let _ = write!(self.out, "{v}");
            }
        } else {
            self.out.push_str("null");
        }
    }

    pub fn bool_field(&mut self, k: &str, v: bool) {
        self.key(k);
        self.out.push_str(if v { "true" } else { "false" });
    }

    pub fn null_field(&mut self, k: &str) {
        self.key(k);
        self.out.push_str("null");
    }

    /// A bare value inside an array.
    pub fn str_item(&mut self, v: &str) {
        self.sep();
        write_str(&mut self.out, v);
    }

    pub fn num_item(&mut self, v: f64) {
        self.sep();
        if v == v.trunc() && v.abs() < 1e15 {
            let _ = write!(self.out, "{}", v as i64);
        } else {
            let _ = write!(self.out, "{v}");
        }
    }

    pub fn finish(self) -> String {
        debug_assert!(self.stack.is_empty(), "unbalanced JSON writer");
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_shapes_a_driver_reply_uses() {
        let mut w = Writer::new();
        w.begin_obj();
        w.str_field("commentary", "Time to \"go\"\nsomewhere");
        w.num_field("n", 3.0);
        w.bool_field("done", true);
        w.arr_field("plan");
        w.begin_obj();
        w.str_field("do", "press");
        w.str_field("button", "a");
        w.end_obj();
        w.end_arr();
        w.end_obj();
        let s = w.finish();

        let v = parse(&s).expect("round trip");
        assert_eq!(v.str_field("commentary"), Some("Time to \"go\"\nsomewhere"));
        assert_eq!(v.u64_field("n"), Some(3));
        assert_eq!(v.get("done").and_then(|d| d.as_bool()), Some(true));
        let plan = v.get("plan").and_then(|p| p.as_array()).unwrap();
        assert_eq!(plan[0].str_field("do"), Some("press"));
    }

    #[test]
    fn a_reply_wrapped_in_prose_and_fences_is_still_read() {
        // This is what models actually return, and refusing it wastes the call.
        let reply = "Sure! Here's my plan:\n```json\n{\"say\": \"hi\", \
                     \"plan\": []}\n```\nHope that helps.";
        let v = parse_embedded(reply).expect("embedded object");
        assert_eq!(v.str_field("say"), Some("hi"));
    }

    #[test]
    fn a_brace_inside_a_string_does_not_end_the_object() {
        let reply = "noise {\"say\": \"a } brace\", \"n\": 1} tail";
        let v = parse_embedded(reply).unwrap();
        assert_eq!(v.str_field("say"), Some("a } brace"));
        assert_eq!(v.u64_field("n"), Some(1));
    }

    #[test]
    fn hostile_input_errors_instead_of_panicking() {
        // Deep nesting must not blow the stack.
        let deep = format!("{}{}", "[".repeat(500), "]".repeat(500));
        assert!(parse(&deep).is_err());
        // Oversized input is refused before it is walked.
        let big = format!("\"{}\"", "x".repeat(MAX_INPUT + 10));
        assert!(parse(&big).is_err());
        for bad in [
            "",
            "{",
            "}",
            "[1,]",
            "{\"a\"}",
            "\"\\q\"",
            "\"unterminated",
            "tru",
            "01x",
            "{\"a\":}",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn escapes_and_surrogate_pairs_decode() {
        assert_eq!(
            parse(r#""a\nb\t\u0041\ud83d\ude00""#).unwrap(),
            Json::Str("a\nb\tA\u{1f600}".to_string())
        );
    }

    #[test]
    fn numbers_that_models_write_are_accepted() {
        assert_eq!(parse("3.0").unwrap().as_u64(), Some(3));
        assert_eq!(parse("-1").unwrap().as_u64(), None);
        assert_eq!(parse("1e3").unwrap().as_u64(), Some(1000));
    }
}
