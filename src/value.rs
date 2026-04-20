use std::fmt;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::error::{Error, Result};
use crate::options::TjsonOptions;
use crate::parse::ParseOptions;
use crate::render::Renderer;

/// A single key-value entry in a TJSON object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: TjsonValue,
}

/// A parsed TJSON value. Mirrors the JSON type system with the same six variants.
///
/// Numbers are stored as strings to preserve exact representation. Objects are stored as
/// an ordered `Vec` of key-value pairs, which allows duplicate keys at the data structure
/// level (though JSON and TJSON parsers typically deduplicate them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TjsonValue {
    /// JSON `null`.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// JSON number.
    Number(serde_json::Number),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<TjsonValue>),
    /// JSON object, as an ordered list of key-value pairs.
    Object(Vec<Entry>),
}

impl From<JsonValue> for TjsonValue {
    fn from(value: JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(value),
            JsonValue::Number(value) => Self::Number(value),
            JsonValue::String(value) => Self::String(value),
            JsonValue::Array(values) => {
                Self::Array(values.into_iter().map(Self::from).collect())
            }
            JsonValue::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| Entry { key, value: Self::from(value) })
                    .collect(),
            ),
        }
    }
}

impl TjsonValue {
    pub(crate) fn parse_with(input: &str, options: ParseOptions) -> Result<Self> {
        crate::parse::Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
    }

    /// Render this value as a TJSON string using the given options.
    pub fn to_tjson_with(&self, options: TjsonOptions) -> Result<String> {
        Renderer::render(self, &options)
    }

    /// Convert this value to a `serde_json::Value`. If the value contains duplicate object keys,
    /// only the last value for each key is kept (serde_json maps deduplicate on insert).
    ///
    /// ```
    /// use tjson::TjsonValue;
    ///
    /// let json: serde_json::Value = serde_json::json!({"name": "Alice"});
    /// let tjson = TjsonValue::from(json.clone());
    /// assert_eq!(tjson.to_json().unwrap(), json);
    /// ```
    pub fn to_json(&self) -> Result<JsonValue> {
        Ok(match self {
            Self::Null => JsonValue::Null,
            Self::Bool(value) => JsonValue::Bool(*value),
            Self::Number(value) => JsonValue::Number(value.clone()),
            Self::String(value) => JsonValue::String(value.clone()),
            Self::Array(values) => JsonValue::Array(
                values
                    .iter()
                    .map(TjsonValue::to_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Object(entries) => {
                let mut map = JsonMap::new();
                for Entry { key, value } in entries {
                    map.insert(key.clone(), value.to_json()?);
                }
                JsonValue::Object(map)
            }
        })
    }
}

impl serde::Serialize for TjsonValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => n.serialize(serializer),
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

impl fmt::Display for TjsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = Renderer::render(self, &TjsonOptions::default()).map_err(|_| fmt::Error)?;
        f.write_str(&s)
    }
}

/// ```
/// let v: tjson::TjsonValue = "  name: Alice".parse().unwrap();
/// assert!(matches!(v, tjson::TjsonValue::Object(_)));
/// ```
impl std::str::FromStr for TjsonValue {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse_with(s, ParseOptions::default())
    }
}
