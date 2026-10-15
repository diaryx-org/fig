//! The JSON the protocol needs, read as JavaScript's `JSON.parse` reads it
//! and written so `JSON.parse` reads it back.
//!
//! Strings are UTF-8 bytes, as `TextEncoder` would encode the JavaScript
//! string: a lone surrogate escape (`\ud800`) becomes U+FFFD. Numbers are
//! `f64`, as JavaScript's are (`1e999` is infinity). An object keeps the
//! order JavaScript gives its own keys (`Object.entries`): a repeated key
//! keeps its first place and its last value, and array-index keys
//! (`"0"`, `"17"`) come first, ascending.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Arr(Vec<Json>),
    Obj(Vec<(Vec<u8>, Json)>),
}

impl Json {
    /// The member `key` of an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key.as_bytes()).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&[u8]> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Whether `key` is an array index: a canonical decimal below 2³² − 1,
/// which a JavaScript object orders ahead of its other keys.
pub fn is_array_index(key: &[u8]) -> bool {
    if key.is_empty() || key.len() > 10 || !key.iter().all(u8::is_ascii_digit) {
        return false;
    }
    if key.len() > 1 && key[0] == b'0' {
        return false;
    }
    let n: u64 = std::str::from_utf8(key).ok().and_then(|s| s.parse().ok()).unwrap_or(u64::MAX);
    n < 0xFFFF_FFFF
}

/// Order an object's members as JavaScript does.
fn js_order(members: Vec<(Vec<u8>, Json)>) -> Vec<(Vec<u8>, Json)> {
    let mut out: Vec<(Vec<u8>, Json)> = Vec::with_capacity(members.len());
    for (k, v) in members {
        if let Some(slot) = out.iter_mut().find(|(x, _)| *x == k) {
            slot.1 = v;
        } else {
            out.push((k, v));
        }
    }
    let (mut index, rest): (Vec<_>, Vec<_>) = out.into_iter().partition(|(k, _)| is_array_index(k));
    index.sort_by_key(|(k, _)| std::str::from_utf8(k).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0));
    index.extend(rest);
    index
}

pub fn parse(input: &[u8]) -> Result<Json, String> {
    let mut p = Parser { s: input, i: 0 };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("trailing bytes at {}", p.i));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at {}", b as char, self.i))
        }
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(format!("unexpected token at {}", self.i))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 10_000 {
            return Err("nesting too deep".into());
        }
        match self.peek() {
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    self.ws();
                    items.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(format!("expected `,` or `]` at {}", self.i)),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(members));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    self.eat(b':')?;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    members.push((k, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(js_order(members)));
                        }
                        _ => return Err(format!("expected `,` or `}}` at {}", self.i)),
                    }
                }
            }
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                text.parse::<f64>().map(Json::Num).map_err(|_| format!("bad number `{text}`"))
            }
            _ => Err(format!("unexpected token at {}", self.i)),
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.s.len() {
            return Err("short \\u escape".into());
        }
        let t = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|e| e.to_string())?;
        let v = u32::from_str_radix(t, 16).map_err(|_| format!("bad \\u escape `{t}`"))?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<Vec<u8>, String> {
        self.eat(b'"')?;
        let mut out = Vec::new();
        loop {
            let Some(b) = self.peek() else { return Err("unterminated string".into()) };
            self.i += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("unterminated escape".into()) };
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let u = self.hex4()?;
                            let c = if (0xD800..0xDC00).contains(&u) {
                                // A high surrogate: a pair with the next escape, else lone.
                                if self.s[self.i..].starts_with(b"\\u") {
                                    let save = self.i;
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if (0xDC00..0xE000).contains(&lo) {
                                        char::from_u32(0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00))
                                    } else {
                                        self.i = save;
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                char::from_u32(u)
                            };
                            let c = c.unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(format!("bad escape at {}", self.i)),
                    }
                }
                _ => out.push(b),
            }
        }
    }
}

// ── writing ─────────────────────────────────────────────────────────────────

/// `bytes` as a JSON string, decoded as `TextDecoder` does (a malformed
/// sequence is U+FFFD).
pub fn write_str(out: &mut Vec<u8>, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{2028}' | '\u{2029}' => out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes()),
            c if (c as u32) < 0x20 => out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes()),
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

pub fn write(out: &mut Vec<u8>, v: &Json) {
    match v {
        Json::Null => out.extend_from_slice(b"null"),
        Json::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Json::Num(n) => {
            if n.is_finite() {
                out.extend_from_slice(js_number_string(*n).as_bytes());
            } else {
                out.extend_from_slice(b"null");
            }
        }
        Json::Str(s) => write_str(out, s),
        Json::Arr(items) => {
            out.push(b'[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write(out, x);
            }
            out.push(b']');
        }
        Json::Obj(members) => {
            out.push(b'{');
            for (i, (k, x)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_str(out, k);
                out.push(b':');
                write(out, x);
            }
            out.push(b'}');
        }
    }
}

/// JavaScript's `Number.prototype.toString()` for a finite `f`: the
/// shortest round-trip digits, positional between 1e-7 and 1e21, else
/// exponential (`1e+21`, `1.5e-7`).
pub fn js_number_string(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let e: i64 = exp.parse().unwrap_or(0);
    let k = digits.len() as i64;
    let n = e + 1;
    let mut s = String::new();
    if f < 0.0 {
        s.push('-');
    }
    if k <= n && n <= 21 {
        s.push_str(&digits);
        s.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        s.push_str(&digits[..n as usize]);
        s.push('.');
        s.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        s.push_str("0.");
        s.extend(std::iter::repeat_n('0', (-n) as usize));
        s.push_str(&digits);
    } else {
        s.push_str(&digits[..1]);
        if k > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if n - 1 >= 0 { '+' } else { '-' });
        s.push_str(&(n - 1).abs().to_string());
    }
    s
}

/// The exact decimal of an integer-valued `f` (JavaScript's `BigInt(f)`).
pub fn integer_decimal(f: f64) -> String {
    let neg = f < 0.0;
    let a = f.abs();
    let digits = if a < 3.4e38 {
        (a as u128).to_string()
    } else {
        // m · 2^e, in base-10⁹ limbs.
        let bits = a.to_bits();
        let exp = ((bits >> 52) & 0x7FF) as i64 - 1075;
        let m = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
        let mut limbs: Vec<u64> = vec![m % 1_000_000_000, (m / 1_000_000_000) % 1_000_000_000, m / 1_000_000_000_000_000_000];
        for _ in 0..exp {
            let mut carry = 0u64;
            for l in limbs.iter_mut() {
                let v = *l * 2 + carry;
                *l = v % 1_000_000_000;
                carry = v / 1_000_000_000;
            }
            if carry > 0 {
                limbs.push(carry);
            }
        }
        while limbs.len() > 1 && *limbs.last().unwrap_or(&1) == 0 {
            limbs.pop();
        }
        let mut s = limbs.last().copied().unwrap_or(0).to_string();
        for l in limbs.iter().rev().skip(1) {
            s.push_str(&format!("{l:09}"));
        }
        s
    };
    if neg && digits != "0" { format!("-{digits}") } else { digits }
}

/// Standard base64, padded or not, whitespace ignored.
pub fn base64_decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        assert_eq!("2.5", js_number_string(2.5));
        assert_eq!("-7", js_number_string(-7.0));
        assert_eq!("1e+21", js_number_string(1e21));
        assert_eq!("1.5e-7", js_number_string(1.5e-7));
        assert_eq!("0.000001", js_number_string(1e-6));
        assert_eq!("123456789012345680000", js_number_string(123456789012345680000.0));
        assert_eq!("0.1", js_number_string(0.1));
        assert_eq!("1000000000000000000000", integer_decimal(1e21));
        assert_eq!("-9223372036854775808", integer_decimal(-9223372036854775808.0));
    }

    #[test]
    fn objects_keep_javascript_key_order() {
        let v = parse(br#"{"b":1,"1":2,"a":3,"b":4,"0":5}"#).unwrap();
        let Json::Obj(m) = v else { panic!() };
        let keys: Vec<&[u8]> = m.iter().map(|(k, _)| &k[..]).collect();
        assert_eq!(vec![&b"0"[..], b"1", b"b", b"a"], keys);
        assert_eq!(Json::Num(4.0), m[2].1);
    }

    #[test]
    fn a_lone_surrogate_is_a_replacement_character() {
        assert_eq!(Json::Str("\u{FFFD}x".as_bytes().to_vec()), parse(br#""\ud800x""#).unwrap());
        assert_eq!(Json::Str("\u{1F600}".as_bytes().to_vec()), parse(b"\"\\ud83d\\ude00\"").unwrap());
    }
}
