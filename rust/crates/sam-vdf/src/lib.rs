//! Parser for Steam's binary KeyValues (VDF) format.
//!
//! This is a port of `SAM.Game/KeyValue.cs` from the C# Steam Achievement Manager.
//! It reads the `UserGameStatsSchema_<appid>.bin` files that Steam caches under
//! `<steam>/appcache/stats/`, which are the only source of achievement and stat
//! *metadata* (display names, descriptions, icons, permission bits). The Steam
//! client interface itself only exposes stat and achievement *values*.
//!
//! Wire format, one node at a time:
//!
//! ```text
//! u8   type tag
//! if tag == End (8): the current container ends here
//! cstr name (NUL-terminated UTF-8)
//! then, depending on tag:
//!   None     (0) -> nested children, terminated by an End tag
//!   String   (1) -> cstr value
//!   Int32    (2) -> i32 little-endian
//!   Float32  (3) -> f32 little-endian
//!   Pointer  (4) -> u32 little-endian
//!   WideString(5)-> unsupported, rejected
//!   Color    (6) -> u32 little-endian
//!   UInt64   (7) -> u64 little-endian
//! ```

use std::fmt;

/// Type tags as they appear on the wire.
mod tag {
    pub const NONE: u8 = 0;
    pub const STRING: u8 = 1;
    pub const INT32: u8 = 2;
    pub const FLOAT32: u8 = 3;
    pub const POINTER: u8 = 4;
    pub const WIDE_STRING: u8 = 5;
    pub const COLOR: u8 = 6;
    pub const UINT64: u8 = 7;
    pub const END: u8 = 8;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A container node; the payload lives in [`KeyValue::children`].
    None,
    String(String),
    Int32(i32),
    Float32(f32),
    Pointer(u32),
    Color(u32),
    UInt64(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyValue {
    pub name: String,
    pub value: Value,
    pub children: Vec<KeyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnexpectedEof,
    UnknownType(u8),
    WideStringUnsupported,
    /// A NUL-terminated string ran off the end of the buffer.
    UnterminatedString,
    InvalidUtf8,
    /// The outermost container was closed but bytes remained.
    TrailingData,
    /// Nesting exceeded [`MAX_DEPTH`]; the file is malformed or hostile.
    DepthLimitExceeded,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => write!(f, "unexpected end of input"),
            Error::UnknownType(t) => write!(f, "unknown value type {t}"),
            Error::WideStringUnsupported => write!(f, "wide strings are unsupported"),
            Error::UnterminatedString => write!(f, "unterminated string"),
            Error::InvalidUtf8 => write!(f, "invalid UTF-8 in string"),
            Error::TrailingData => write!(f, "trailing data after root container"),
            Error::DepthLimitExceeded => write!(f, "nesting depth limit exceeded"),
        }
    }
}

impl std::error::Error for Error {}

/// Schema files nest maybe four or five levels deep in practice. This bound
/// exists purely so a corrupt file cannot drive the recursive parser into a
/// stack overflow.
pub const MAX_DEPTH: usize = 64;

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Result<u8, Error> {
        let b = *self.data.get(self.pos).ok_or(Error::UnexpectedEof)?;
        self.pos += 1;
        Ok(b)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let end = self.pos.checked_add(N).ok_or(Error::UnexpectedEof)?;
        let slice = self.data.get(self.pos..end).ok_or(Error::UnexpectedEof)?;
        self.pos = end;
        Ok(slice.try_into().expect("slice length checked above"))
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.take::<4>()?))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take::<4>()?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take::<8>()?))
    }

    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.take::<4>()?))
    }

    /// NUL-terminated UTF-8. Named `ReadStringUnicode` in the C#, but it is
    /// UTF-8 there too, not UTF-16.
    fn cstr(&mut self) -> Result<String, Error> {
        let start = self.pos;
        let rel = self.data[start..]
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::UnterminatedString)?;
        let bytes = &self.data[start..start + rel];
        self.pos = start + rel + 1;
        // Steam occasionally emits text that is not strictly valid UTF-8 in
        // localized display strings. Salvage it rather than failing the whole
        // schema, which would make the game unusable in the UI.
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// Parse a complete binary KeyValues document.
///
/// The returned root is a synthetic container named `<root>`; a schema file's
/// real content is its single child, keyed by the app ID as a decimal string.
pub fn parse(data: &[u8]) -> Result<KeyValue, Error> {
    let mut cursor = Cursor { data, pos: 0 };
    let children = parse_children(&mut cursor, 0)?;

    // The C# treats leftover bytes as a hard failure. Keep that, but tolerate
    // trailing NUL padding, which some cached files carry.
    if !cursor.at_end() && data[cursor.pos..].iter().any(|&b| b != 0) {
        return Err(Error::TrailingData);
    }

    Ok(KeyValue {
        name: "<root>".to_string(),
        value: Value::None,
        children,
    })
}

fn parse_children(cursor: &mut Cursor<'_>, depth: usize) -> Result<Vec<KeyValue>, Error> {
    if depth > MAX_DEPTH {
        return Err(Error::DepthLimitExceeded);
    }

    let mut children = Vec::new();
    loop {
        let tag = cursor.u8()?;
        if tag == tag::END {
            return Ok(children);
        }

        let name = cursor.cstr()?;
        let (value, nested) = match tag {
            tag::NONE => (Value::None, parse_children(cursor, depth + 1)?),
            tag::STRING => (Value::String(cursor.cstr()?), Vec::new()),
            tag::INT32 => (Value::Int32(cursor.i32()?), Vec::new()),
            tag::FLOAT32 => (Value::Float32(cursor.f32()?), Vec::new()),
            tag::POINTER => (Value::Pointer(cursor.u32()?), Vec::new()),
            tag::COLOR => (Value::Color(cursor.u32()?), Vec::new()),
            tag::UINT64 => (Value::UInt64(cursor.u64()?), Vec::new()),
            tag::WIDE_STRING => return Err(Error::WideStringUnsupported),
            other => return Err(Error::UnknownType(other)),
        };

        children.push(KeyValue {
            name,
            value,
            children: nested,
        });
    }
}

/// A missing node. Lookups return this instead of an `Option` so that chained
/// access such as `kv["display"]["name"]` mirrors the C# and never panics.
static MISSING: KeyValue = KeyValue {
    name: String::new(),
    value: Value::None,
    children: Vec::new(),
};

impl KeyValue {
    /// Case-insensitive child lookup, matching the C# `InvariantCultureIgnoreCase`.
    /// Returns a permanently-empty node when absent so accesses can be chained.
    pub fn get(&self, key: &str) -> &KeyValue {
        self.children
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(key))
            .unwrap_or(&MISSING)
    }

    /// All children whose name matches, case-insensitively. The achievement
    /// block of a schema repeats the key `bits`, so a single-result lookup
    /// would drop achievements.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a KeyValue> + 'a {
        self.children
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(key))
    }

    /// True when this node came from the file rather than from a failed lookup.
    /// A container node is valid; only [`MISSING`] and empty names are not.
    pub fn is_valid(&self) -> bool {
        !std::ptr::eq(self, &MISSING)
    }

    pub fn is_container(&self) -> bool {
        matches!(self.value, Value::None)
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// String coercion. Non-string scalars stringify, matching the C#
    /// `Value.ToString()` behaviour.
    pub fn as_string_or(&self, default: &str) -> String {
        match &self.value {
            Value::String(s) => s.clone(),
            Value::Int32(v) => v.to_string(),
            Value::Float32(v) => v.to_string(),
            Value::Pointer(v) | Value::Color(v) => v.to_string(),
            Value::UInt64(v) => v.to_string(),
            Value::None => default.to_string(),
        }
    }

    pub fn as_i32_or(&self, default: i32) -> i32 {
        match &self.value {
            Value::String(s) => s.trim().parse().unwrap_or(default),
            Value::Int32(v) => *v,
            Value::Float32(v) => *v as i32,
            Value::UInt64(v) => (*v & 0xFFFF_FFFF) as i32,
            _ => default,
        }
    }

    pub fn as_f32_or(&self, default: f32) -> f32 {
        match &self.value {
            Value::String(s) => s.trim().parse().unwrap_or(default),
            Value::Int32(v) => *v as f32,
            Value::Float32(v) => *v,
            Value::UInt64(v) => (*v & 0xFFFF_FFFF) as f32,
            _ => default,
        }
    }

    pub fn as_bool_or(&self, default: bool) -> bool {
        match &self.value {
            Value::String(s) => s.trim().parse::<i32>().map(|v| v != 0).unwrap_or(default),
            Value::Int32(v) => *v != 0,
            Value::Float32(v) => (*v as i32) != 0,
            Value::UInt64(v) => *v != 0,
            _ => default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds `root { "outer" { "name" = "hello", "count" = 42i32 } }`.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.push(tag::NONE);
        b.extend_from_slice(b"outer\0");
        b.push(tag::STRING);
        b.extend_from_slice(b"name\0");
        b.extend_from_slice(b"hello\0");
        b.push(tag::INT32);
        b.extend_from_slice(b"count\0");
        b.extend_from_slice(&42i32.to_le_bytes());
        b.push(tag::END); // closes "outer"
        b.push(tag::END); // closes root
        b
    }

    #[test]
    fn parses_nested_document() {
        let kv = parse(&sample()).expect("should parse");
        let outer = kv.get("outer");
        assert!(outer.is_valid());
        assert_eq!(outer.get("name").as_str(), Some("hello"));
        assert_eq!(outer.get("count").as_i32_or(0), 42);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let kv = parse(&sample()).unwrap();
        assert_eq!(kv.get("OUTER").get("NaMe").as_str(), Some("hello"));
    }

    #[test]
    fn missing_lookups_chain_without_panicking() {
        let kv = parse(&sample()).unwrap();
        let missing = kv.get("nope").get("also-nope").get("still-nope");
        assert!(!missing.is_valid());
        assert_eq!(missing.as_i32_or(7), 7);
        assert_eq!(missing.as_string_or("fallback"), "fallback");
    }

    #[test]
    fn coerces_across_types() {
        let mut b = Vec::new();
        b.push(tag::STRING);
        b.extend_from_slice(b"numeric_string\0");
        b.extend_from_slice(b"123\0");
        b.push(tag::FLOAT32);
        b.extend_from_slice(b"real\0");
        b.extend_from_slice(&2.75f32.to_le_bytes());
        b.push(tag::END);

        let kv = parse(&b).unwrap();
        // A string node still answers integer and boolean queries, which the
        // schema relies on for fields like "min" and "incrementonly".
        assert_eq!(kv.get("numeric_string").as_i32_or(0), 123);
        assert!(kv.get("numeric_string").as_bool_or(false));
        // Floats truncate toward zero when read as an integer.
        assert_eq!(kv.get("real").as_i32_or(0), 2);
        assert_eq!(kv.get("real").as_f32_or(0.0), 2.75);
    }

    #[test]
    fn get_all_returns_every_match() {
        let mut b = Vec::new();
        for _ in 0..3 {
            b.push(tag::NONE);
            b.extend_from_slice(b"bits\0");
            b.push(tag::END);
        }
        b.push(tag::END);

        let kv = parse(&b).unwrap();
        assert_eq!(kv.get_all("bits").count(), 3);
    }

    #[test]
    fn rejects_truncated_input() {
        let mut truncated = sample();
        truncated.truncate(truncated.len() - 4);
        assert!(parse(&truncated).is_err());
    }

    #[test]
    fn rejects_unknown_type_tag() {
        let b = vec![99u8, b'x', 0];
        assert_eq!(parse(&b), Err(Error::UnknownType(99)));
    }

    #[test]
    fn tolerates_trailing_nul_padding() {
        let mut padded = sample();
        padded.extend_from_slice(&[0, 0, 0, 0]);
        assert!(parse(&padded).is_ok());
    }

    #[test]
    fn rejects_trailing_garbage() {
        let mut trailing = sample();
        trailing.extend_from_slice(b"junk");
        assert_eq!(parse(&trailing), Err(Error::TrailingData));
    }
}
