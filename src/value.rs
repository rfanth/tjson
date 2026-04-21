use std::fmt;

use crate::error::{Error, Result};
use crate::number::Number;
use crate::options::RenderOptions;
use crate::parse::ParseOptions;
use crate::render::Renderer;

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
        Renderer::render(self, &options).expect("render is infallible")
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
