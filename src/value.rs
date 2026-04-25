use std::fmt;

use crate::error::{Error, Result};
use crate::number::Number;
use crate::options::RenderOptions;
use crate::parse::{MultilineLocalEol, ParseOptions};
use crate::render::Renderer;
use crate::util::{
    is_allowed_bare_string, /*is_comma_like,*/ is_forbidden_literal_tjson_char, is_pipe_like,
    is_reserved_word, parse_bare_key_prefix,
};

/// Single-pass string classifier. Carries the original `&str` plus all renderer-relevant
/// boolean flags so callers can classify once and branch without re-scanning.
#[derive(Clone, Copy)]
pub(crate) struct StrMeta<'a> {
    pub(crate) s: &'a str,
    /// `true` when the string contains at least one `\n` (covers both LF and CRLF lines).
    pub(crate) has_eol: bool,
    /// `Some(Lf)` / `Some(CrLf)` when newlines are uniform; `None` when mixed or absent.
    pub(crate) eol_type: Option<MultilineLocalEol>,
    /// `true` when the string contains any char that is forbidden in literal TJSON strings.
    pub(crate) has_forbidden_literal: bool,
    /// `true` when `is_allowed_bare_string` would return `true`.
    pub(crate) is_bare_eligible: bool,
    /// `true` when the string matches a reserved word ("true", "false", "null", "[]", "{}", "\"\"").
    pub(crate) is_reserved_word: bool,
    /// `true` when the string contains at least one pipe-like character.
    pub(crate) has_pipe_like: bool,
    // /// `true` when the string contains at least one comma-like character.
    //pub(crate) has_comma_like: bool,
}

impl<'a> StrMeta<'a> {
    pub(crate) fn new(s: &'a str) -> Self {
        let has_eol = s.as_bytes().contains(&b'\n');
        let eol_type = if has_eol { detect_multiline_local_eol(s) } else { Some(MultilineLocalEol::default()) };
        let has_forbidden_literal = s.chars().any(is_forbidden_literal_tjson_char);
        let is_bare_eligible = is_allowed_bare_string(s);
        let is_reserved_word = is_reserved_word(s);
        let has_pipe_like = s.chars().any(is_pipe_like);
        // let has_comma_like = s.chars().any(is_comma_like);
        StrMeta { s, has_eol, eol_type, has_forbidden_literal, is_bare_eligible, is_reserved_word, has_pipe_like/*, has_comma_like*/ }
    }
}

/// A `&str` guaranteed to satisfy the TJSON bare-string rules (rendereable without quoting).
#[allow(dead_code)]
pub(crate) struct BareString<'a>(StrMeta<'a>);

#[allow(dead_code)]
impl<'a> BareString<'a> {
    pub(crate) fn new(s: &'a str) -> Option<Self> {
        let meta = StrMeta::new(s);
        if meta.is_bare_eligible { Some(BareString(meta)) } else { None }
    }

    pub(crate) fn meta(&self) -> &StrMeta<'a> { &self.0 }
}

impl<'a> std::ops::Deref for BareString<'a> {
    type Target = str;
    fn deref(&self) -> &str { self.0.s }
}

/// A `BareString` that is also safe in table cells: not a reserved word, no pipe-like chars.
///
/// `TableBareString` is a strict subtype of `BareString` — it can always be used anywhere
/// a `BareString` is accepted via `Deref`.
#[allow(dead_code)]
pub(crate) struct TableBareString<'a>(BareString<'a>);

#[allow(dead_code)]
impl<'a> TableBareString<'a> {
    pub(crate) fn new(s: &'a str) -> Option<Self> {
        let meta = StrMeta::new(s);
        if meta.is_bare_eligible && !meta.is_reserved_word && !meta.has_pipe_like {
            Some(TableBareString(BareString(meta)))
        } else {
            None
        }
    }
}

impl<'a> std::ops::Deref for TableBareString<'a> {
    type Target = BareString<'a>;
    fn deref(&self) -> &BareString<'a> { &self.0 }
}

/// A `&str` that can be rendered as a TJSON multiline string: contains newlines (uniform
/// LF or uniform CRLF) and no forbidden literal characters.
#[allow(dead_code)]
pub(crate) struct MultilineString<'a>(StrMeta<'a>);

#[allow(dead_code)]
impl<'a> MultilineString<'a> {
    pub(crate) fn new(s: &'a str) -> Option<Self> {
        let meta = StrMeta::new(s);
        if meta.has_eol && meta.eol_type.is_some() && !meta.has_forbidden_literal {
            Some(MultilineString(meta))
        } else {
            None
        }
    }

    pub(crate) fn eol(&self) -> MultilineLocalEol { self.0.eol_type.unwrap() }
}

impl<'a> std::ops::Deref for MultilineString<'a> {
    type Target = str;
    fn deref(&self) -> &str { self.0.s }
}

/// A `&str` guaranteed to satisfy the TJSON bare-key rules.
#[allow(dead_code)]
pub(crate) struct BareKey<'a>(&'a str);

#[allow(dead_code)]
impl<'a> BareKey<'a> {
    pub(crate) fn new(s: &'a str) -> Option<Self> {
        if parse_bare_key_prefix(s).is_some_and(|end| end == s.len()) {
            Some(BareKey(s))
        } else {
            None
        }
    }
}

impl<'a> std::ops::Deref for BareKey<'a> {
    type Target = str;
    fn deref(&self) -> &str { self.0 }
}

/// A single key-value entry in a TJSON object.
///
/// Used instead of a tuple so that code handling object entries can use named fields
/// rather than `.0` / `.1`. Objects are represented as `Vec<Entry>` to preserve
/// insertion order and allow duplicate keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: Value,
}

/// A parsed TJSON value. Mirrors the JSON type system with the same six variants.
///
/// Numbers are stored as [`Number`] values, which preserve the exact string representation.
/// Objects are stored as an ordered `Vec` of key-value pairs, which allows duplicate keys
/// at the data structure level (though JSON and TJSON parsers typically deduplicate them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// JSON `null`.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// JSON number.
    Number(Number),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<Value>),
    /// JSON object, as an ordered list of key-value pairs.
    Object(Vec<Entry>),
}

#[cfg(feature = "serde_json")]
impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        Self::from_serde_json(value)
    }
}

#[cfg(feature = "serde_json")]
impl From<Value> for serde_json::Value {
    fn from(value: Value) -> Self {
        value.to_serde_json()
    }
}

impl Value {
    /// Convert from a `serde_json::Value`. Used internally regardless of the `serde_json`
    /// feature, since serde_json is a hard dependency.
    pub(crate) fn from_serde_json(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(v) => Self::Bool(v),
            serde_json::Value::Number(n) => Self::Number(Number(n.to_string())),
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Array(values) => {
                Self::Array(values.into_iter().map(Self::from_serde_json).collect())
            }
            serde_json::Value::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| Entry { key, value: Self::from_serde_json(value) })
                    .collect(),
            ),
        }
    }

    pub(crate) fn to_serde_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(v) => serde_json::Value::Bool(*v),
            Self::Number(n) => serde_json::Value::Number(n.to_serde_json_number()),
            Self::String(s) => serde_json::Value::String(s.clone()),
            Self::Array(values) => {
                serde_json::Value::Array(values.iter().map(Value::to_serde_json).collect())
            }
            Self::Object(entries) => {
                let mut map = serde_json::Map::new();
                for Entry { key, value } in entries {
                    map.insert(key.clone(), value.to_serde_json());
                }
                serde_json::Value::Object(map)
            }
        }
    }

    pub(crate) fn parse_with(input: &str, options: ParseOptions) -> Result<Self> {
        crate::parse::Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
    }

    /// Render this value as a TJSON string using the given options.
    ///
    /// ```
    /// use tjson::{Value, RenderOptions};
    ///
    /// let v: Value = "  name: Alice  age:30".parse().unwrap();
    /// let s = v.to_tjson_with(RenderOptions::canonical());
    /// assert_eq!(s, "  name: Alice\n  age:30");
    /// ```
    pub fn to_tjson_with(&self, options: RenderOptions) -> String {
        Renderer::render(self, &options)
    }

    /// Serialize this value to a JSON string.
    ///
    /// ```
    /// use tjson::Value;
    ///
    /// let v: Value = "  name: Alice".parse().unwrap();
    /// assert_eq!(v.to_json(), r#"{"name":"Alice"}"#);
    /// ```
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_json(self, &mut out);
        out
    }
}

fn write_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            // serde_json handles all JSON string escaping correctly.
            out.push_str(&serde_json::to_string(s).expect("string serialization is infallible"))
        }
        Value::Array(values) => {
            out.push('[');
            for (i, v) in values.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_json(v, out);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            out.push('{');
            for (i, Entry { key, value }) in entries.iter().enumerate() {
                if i > 0 { out.push(','); }
                out.push_str(&serde_json::to_string(key).expect("string serialization is infallible"));
                out.push(':');
                write_json(value, out);
            }
            out.push('}');
        }
    }
}

impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => serde::Serialize::serialize(n, serializer),
            Self::String(s) => serializer.serialize_str(s),
            Self::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for v in values {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Self::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for Entry { key, value } in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_tjson_with(RenderOptions::default()))
    }
}

/// ```
/// let v: tjson::Value = "  name: Alice".parse().unwrap();
/// assert!(matches!(v, tjson::Value::Object(_)));
/// ```
impl std::str::FromStr for Value {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse_with(s, ParseOptions::default())
    }
}

pub(crate) fn detect_multiline_local_eol(value: &str) -> Option<MultilineLocalEol> {
    let bytes = value.as_bytes();
    let mut index = 0usize;
    let mut saw_lf = false;
    let mut saw_crlf = false;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    saw_crlf = true;
                    index += 2;
                } else {
                    return None;
                }
            }
            b'\n' => {
                saw_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    match (saw_lf, saw_crlf) {
        (false, false) => None,
        (true, false) => Some(MultilineLocalEol::Lf),
        (false, true) => Some(MultilineLocalEol::CrLf),
        (true, true) => None,
    }
}
