//! serde `Deserializer` over any [`Tree`], written once and instantiated twice:
//! against [`SpannedValue`](crate::spanned::SpannedValue) for [`crate::from_str`]
//! (errors carry real TJSON line/column with a caret excerpt) and against
//! [`Value`](crate::value::Value) for [`crate::from_value`] (no source text exists, so
//! errors carry the field path only — never fabricated coordinates).
//!
//! # Number protocol
//!
//! This deserializer speaks serde_json's `arbitrary_precision` wire protocol: a number
//! that cannot be delivered exactly through a primitive visit arrives as a single-entry
//! map `{ "$serde_json::private::Number": "<digits>" }`. `serde_json::Number` and
//! [`crate::Number`] recognize that shape and capture the digits losslessly — exactness
//! is a format invariant ("1.00" is not "1"). `deserialize_any` uses the ladder
//! u64 → i64 → u128 → i128 → f64-only-when-the-digit-string-round-trips → token map,
//! mirroring serde_json's value deserializer. This is deliberately more accepting than
//! the old text trampoline (untagged enums now match plain floats); the differential
//! tests pin the divergence.

use serde::de::{
    self, Deserializer, IntoDeserializer, Visitor,
    value::{BorrowedStrDeserializer, StringDeserializer},
};

use crate::error::{DeserializeError, Location};
use crate::number::Number;
use crate::spanned;
use crate::tree::{NodeRef, Span, Tree};

/// serde_json's private number token. Impersonated byte-for-byte on purpose: it is how
/// `serde_json::Number` (and therefore [`crate::Number`]) recognizes exact digits coming
/// through the serde data model.
const NUMBER_TOKEN: &str = "$serde_json::private::Number";

type Result<T> = std::result::Result<T, DeserializeError>;

pub(crate) struct TreeDeserializer<'de, T: Tree> {
    node: &'de T,
    /// Original source text, present when the tree was parsed from it. Used to resolve
    /// spans into locations at error time; absent for programmatically built trees.
    source: Option<&'de str>,
}

impl<'de, T: Tree> TreeDeserializer<'de, T> {
    pub(crate) fn new(node: &'de T, source: Option<&'de str>) -> Self {
        Self { node, source }
    }

    fn location_of(&self, span: Option<Span>) -> Option<Location> {
        locate_span(self.source, span)
    }

    /// Stamp this node's location onto an error that doesn't have one yet. Applied in
    /// every entry point so the deepest node that saw the error names the position.
    fn stamp(&self, err: DeserializeError) -> DeserializeError {
        match self.location_of(self.node.span()) {
            Some(location) => err.locate(location),
            None => err,
        }
    }

    fn stamp_result<V>(&self, result: Result<V>) -> Result<V> {
        result.map_err(|err| self.stamp(err))
    }

    /// The `Unexpected` description for type mismatches, matching serde_json's wording
    /// so diagnostics stay familiar.
    fn unexpected(&self) -> de::Unexpected<'_> {
        match self.node.node() {
            NodeRef::Null => de::Unexpected::Unit,
            NodeRef::Bool(b) => de::Unexpected::Bool(b),
            NodeRef::Number(n) => {
                if let Some(u) = n.as_u64() {
                    de::Unexpected::Unsigned(u)
                } else if let Some(i) = n.as_i64() {
                    de::Unexpected::Signed(i)
                } else {
                    de::Unexpected::Other("number")
                }
            }
            NodeRef::String(s) => de::Unexpected::Str(s),
            NodeRef::Array(_) => de::Unexpected::Seq,
            NodeRef::Object(_) => de::Unexpected::Map,
        }
    }

    fn invalid_type(&self, exp: &dyn de::Expected) -> DeserializeError {
        self.stamp(de::Error::invalid_type(self.unexpected(), exp))
    }
}

/// Delivers a number as serde_json's `arbitrary_precision` token map.
struct NumberTokenAccess {
    digits: Option<String>,
}

impl<'de> de::MapAccess<'de> for NumberTokenAccess {
    type Error = DeserializeError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.digits.is_none() {
            return Ok(None);
        }
        seed.deserialize(NumberTokenKey).map(Some)
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let digits = self.digits.take().expect("next_value_seed follows Some key");
        let string_deserializer: StringDeserializer<DeserializeError> = digits.into_deserializer();
        seed.deserialize(string_deserializer)
    }
}

/// Deserializer for the token map's single key.
struct NumberTokenKey;

impl<'de> Deserializer<'de> for NumberTokenKey {
    type Error = DeserializeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_borrowed_str(NUMBER_TOKEN)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
        byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map
        struct enum identifier ignored_any
    }
}

/// A hint for errors that bubble out of serde's self-describing machinery (untagged
/// enum matching, `deserialize_any` consumers) over a number that only exists in exact
/// form. Those errors are minted by serde's Content buffering, which cannot see why the
/// number arrived as a token map, so the generic "did not match any variant" hides the
/// real cause. The access wrappers append this when the failing value is such a number
/// and the error carries no location — the signature of a Content-land failure (our own
/// typed methods stamp locations at creation).
fn exactness_hint<T: Tree>(node: &T) -> Option<(String, Option<Span>)> {
    // The offending number may be nested (an object variant fails to match because
    // one of ITS fields is an exact-only number), so search the subtree for the
    // first exact-only number rather than checking the top node alone. Its span (when
    // the tree carries one) relocates the error from the container to the offender.
    let offender = first_exact_only_number(node)?;
    let NodeRef::Number(n) = offender.node() else {
        unreachable!("first_exact_only_number only returns number nodes");
    };
    let digits = n.as_str();
    // The loss is LEXICAL, not necessarily numeric: 2.50 is numerically exact in an
    // f64, but reads back as 2.5 — the written form (trailing zeros, exponent
    // spelling, extreme precision) is what cannot survive the trip. Show the
    // readback spelling when there is one so the user sees exactly what would change.
    let readback = digits
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .and_then(serde_json::Number::from_f64)
        .map(|n| n.to_string());
    // Name the culprit precisely. By far the most common case is a trailing zero in
    // the fraction (2.50, 1.00): numerically representable, lexically not — the zero
    // can signal precision and an f64 cannot keep it. Everything else (30-digit
    // decimals, out-of-range exponents) is genuine precision overflow.
    let cause = if let Some(zeros) = fraction_trailing_zeros(digits) {
        let read_back = readback
            .map(|r| format!(" (it would read back as {r})"))
            .unwrap_or_default();
        if zeros > 1 {
            format!(
                "the trailing zeros in {digits} are part of the number as written — \
                 they can signal precision — but an f64 cannot keep them{read_back}"
            )
        } else {
            format!(
                "the trailing zero in {digits} is part of the number as written — it \
                 can signal precision — but an f64 cannot keep it{read_back}"
            )
        }
    } else {
        let read_back = readback
            .map(|r| format!(" — the nearest f64 reads back as {r} —"))
            .unwrap_or_default();
        format!("the number {digits} exceeds what an f64 can hold exactly{read_back}")
    };
    let note = format!(
        "{cause}, and untagged enum matching cannot know whether you would accept \
         that, so it only sees the exact form, which f64 variants cannot hold. A \
         tjson::Number field preserves {digits} as written; a plain f64 field \
         accepts the approximation"
    );
    Some((note, offender.span()))
}

fn first_exact_only_number<T: Tree>(node: &T) -> Option<&T> {
    match node.node() {
        NodeRef::Number(n) => {
            let digits = n.as_str();
            let fits_integer = digits.parse::<u64>().is_ok()
                || digits.parse::<i64>().is_ok()
                || digits.parse::<u128>().is_ok()
                || digits.parse::<i128>().is_ok();
            let rides_f64 = digits.parse::<f64>().is_ok_and(|f| f64_round_trips(digits, f));
            if fits_integer || rides_f64 { None } else { Some(node) }
        }
        NodeRef::Array(items) => items.iter().find_map(first_exact_only_number),
        NodeRef::Object(entries) => entries
            .iter()
            .find_map(|entry| first_exact_only_number(T::entry_value(entry))),
        _ => None,
    }
}

/// The count of trailing zeros in the number's fractional part (before any exponent),
/// when there are any — the "2.50" / "1.00" case, where the loss through f64 is purely
/// the written zeros.
fn fraction_trailing_zeros(digits: &str) -> Option<usize> {
    let mantissa = digits.split(['e', 'E']).next().unwrap_or(digits);
    if !mantissa.contains('.') {
        return None;
    }
    let zeros = mantissa.len() - mantissa.trim_end_matches('0').len();
    if zeros > 0 { Some(zeros) } else { None }
}

/// Resolve a span against the original source, when both exist.
fn locate_span(source: Option<&str>, span: Option<Span>) -> Option<Location> {
    let source = source?;
    let span = span?;
    let (line, column, source_line) = spanned::locate(source, span);
    Some(Location { line, column, source_line: Some(source_line) })
}

/// `true` when `digits` is exactly recoverable from the f64 it parses to, i.e. visiting
/// f64 loses nothing — not the value alone, the *spelling*: self-describing consumers
/// regenerate digits through `serde_json::Number`, so that is the formatter the check
/// must agree with ("1e100" formats back as "1e+100" and must NOT take this path).
/// `1.00` and `100000000000000000001` fail and ride the exact token map instead.
fn f64_round_trips(digits: &str, value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    serde_json::Number::from_f64(value).is_some_and(|n| n.to_string() == digits)
}

macro_rules! deserialize_parsed_number {
    ($method:ident, $ty:ty, $visit:ident) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            match self.node.node() {
                NodeRef::Number(n) => match n.as_str().parse::<$ty>() {
                    Ok(value) => self.stamp_result(visitor.$visit(value)),
                    Err(_) => Err(self.invalid_type(&visitor)),
                },
                _ => Err(self.invalid_type(&visitor)),
            }
        }
    };
}

impl<'de, T: Tree> Deserializer<'de> for TreeDeserializer<'de, T> {
    type Error = DeserializeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Null => self.stamp_result(visitor.visit_unit()),
            NodeRef::Bool(b) => self.stamp_result(visitor.visit_bool(b)),
            NodeRef::Number(n) => {
                let digits = n.as_str();
                let result = if let Ok(u) = digits.parse::<u64>() {
                    visitor.visit_u64(u)
                } else if let Ok(i) = digits.parse::<i64>() {
                    visitor.visit_i64(i)
                } else if let Ok(u) = digits.parse::<u128>() {
                    visitor.visit_u128(u)
                } else if let Ok(i) = digits.parse::<i128>() {
                    visitor.visit_i128(i)
                } else if digits.parse::<f64>().is_ok_and(|f| f64_round_trips(digits, f)) {
                    visitor.visit_f64(digits.parse::<f64>().expect("checked just above"))
                } else {
                    visitor.visit_map(NumberTokenAccess { digits: Some(digits.to_owned()) })
                };
                self.stamp_result(result)
            }
            NodeRef::String(s) => self.stamp_result(visitor.visit_borrowed_str(s)),
            NodeRef::Array(items) => {
                let source = self.source;
                self.stamp_result(visitor.visit_seq(SeqAccess::new(items, source)))
            }
            NodeRef::Object(entries) => {
                let source = self.source;
                self.stamp_result(visitor.visit_map(MapAccess::<T>::new(entries, source)))
            }
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Bool(b) => self.stamp_result(visitor.visit_bool(b)),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    deserialize_parsed_number!(deserialize_i8, i8, visit_i8);
    deserialize_parsed_number!(deserialize_i16, i16, visit_i16);
    deserialize_parsed_number!(deserialize_i32, i32, visit_i32);
    deserialize_parsed_number!(deserialize_i64, i64, visit_i64);
    deserialize_parsed_number!(deserialize_i128, i128, visit_i128);
    deserialize_parsed_number!(deserialize_u8, u8, visit_u8);
    deserialize_parsed_number!(deserialize_u16, u16, visit_u16);
    deserialize_parsed_number!(deserialize_u32, u32, visit_u32);
    deserialize_parsed_number!(deserialize_u64, u64, visit_u64);
    deserialize_parsed_number!(deserialize_u128, u128, visit_u128);
    deserialize_parsed_number!(deserialize_f32, f32, visit_f32);
    deserialize_parsed_number!(deserialize_f64, f64, visit_f64);

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::String(s) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => self.stamp_result(visitor.visit_char(c)),
                    _ => Err(self.invalid_type(&visitor)),
                }
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::String(s) => self.stamp_result(visitor.visit_borrowed_str(s)),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::String(s) => self.stamp_result(visitor.visit_borrowed_str(s)),
            NodeRef::Array(items) => {
                let source = self.source;
                self.stamp_result(visitor.visit_seq(SeqAccess::new(items, source)))
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Null => self.stamp_result(visitor.visit_none()),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Null => self.stamp_result(visitor.visit_unit()),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Array(items) => {
                let source = self.source;
                self.stamp_result(visitor.visit_seq(SeqAccess::new(items, source)))
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.node.node() {
            NodeRef::Object(entries) => {
                let source = self.source;
                self.stamp_result(visitor.visit_map(MapAccess::<T>::new(entries, source)))
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.node.node() {
            // A bare string is a unit variant.
            NodeRef::String(_) => {
                let node = self.node;
                let source = self.source;
                self.stamp_result(visitor.visit_enum(EnumAccess {
                    variant: node,
                    content: None,
                    source,
                }))
            }
            // A single-entry object is `variant: content`, matching serde_json.
            NodeRef::Object(entries) => {
                if entries.len() != 1 {
                    return Err(self.stamp(de::Error::invalid_length(
                        entries.len(),
                        &"map with a single key naming the enum variant",
                    )));
                }
                let entry = &entries[0];
                let source = self.source;
                self.stamp_result(visitor.visit_enum(VariantByKeyAccess::<T> { entry, source }))
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.stamp_result(visitor.visit_unit())
    }
}

struct SeqAccess<'de, T: Tree> {
    items: std::slice::Iter<'de, T>,
    index: usize,
    source: Option<&'de str>,
}

impl<'de, T: Tree> SeqAccess<'de, T> {
    fn new(items: &'de [T], source: Option<&'de str>) -> Self {
        Self { items: items.iter(), index: 0, source }
    }
}

impl<'de, T: Tree + 'de> de::SeqAccess<'de> for SeqAccess<'de, T> {
    type Error = DeserializeError;

    fn next_element_seed<S: de::DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>> {
        let Some(item) = self.items.next() else {
            return Ok(None);
        };
        let index = self.index;
        self.index += 1;
        seed.deserialize(TreeDeserializer::new(item, self.source))
            .map(Some)
            .map_err(|err| {
                let err = match exactness_hint(item) {
                    Some((note, span)) if !err.is_located() => {
                        let err = err.with_note(&note);
                        match locate_span(self.source, span) {
                            Some(location) => err.locate(location),
                            None => err,
                        }
                    }
                    _ => err,
                };
                err.nest(&format!("[{index}]"))
            })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

struct MapAccess<'de, T: Tree> {
    entries: std::slice::Iter<'de, T::Entry>,
    current: Option<&'de T::Entry>,
    source: Option<&'de str>,
}

impl<'de, T: Tree> MapAccess<'de, T> {
    fn new(entries: &'de [T::Entry], source: Option<&'de str>) -> Self {
        Self { entries: entries.iter(), current: None, source }
    }

    fn locate_key(&self, entry: &'de T::Entry) -> Option<Location> {
        let source = self.source?;
        let span = T::entry_key_span(entry)?;
        let (line, column, source_line) = spanned::locate(source, span);
        Some(Location { line, column, source_line: Some(source_line) })
    }
}

impl<'de, T: Tree + 'de> de::MapAccess<'de> for MapAccess<'de, T>
where
    T::Entry: 'de,
{
    type Error = DeserializeError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some(entry) = self.entries.next() else {
            self.current = None;
            return Ok(None);
        };
        self.current = Some(entry);
        let key = T::entry_key(entry);
        // Errors here are about the key itself (e.g. deny_unknown_fields), so they get
        // the key's location, not the value's.
        seed.deserialize(BorrowedStrDeserializer::new(key)).map(Some).map_err(
            |err: DeserializeError| match self.locate_key(entry) {
                Some(location) => err.locate(location),
                None => err,
            },
        )
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let entry = self.current.take().expect("next_value_seed follows next_key_seed");
        let key = T::entry_key(entry);
        let value = T::entry_value(entry);
        seed.deserialize(TreeDeserializer::new(value, self.source))
            .map_err(|err| {
                let err = match exactness_hint(value) {
                    Some((note, span)) if !err.is_located() => {
                        let err = err.with_note(&note);
                        match locate_span(self.source, span) {
                            Some(location) => err.locate(location),
                            None => err,
                        }
                    }
                    _ => err,
                };
                err.nest(key)
            })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

/// Enum access for a unit variant spelled as a bare string.
struct EnumAccess<'de, T: Tree> {
    variant: &'de T,
    content: Option<&'de T>,
    source: Option<&'de str>,
}

impl<'de, T: Tree + 'de> de::EnumAccess<'de> for EnumAccess<'de, T> {
    type Error = DeserializeError;
    type Variant = VariantAccess<'de, T>;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let variant = seed.deserialize(TreeDeserializer::new(self.variant, self.source))?;
        Ok((variant, VariantAccess { content: self.content, source: self.source }))
    }
}

/// Enum access for `variant: content` spelled as a single-entry object.
struct VariantByKeyAccess<'de, T: Tree> {
    entry: &'de T::Entry,
    source: Option<&'de str>,
}

impl<'de, T: Tree + 'de> de::EnumAccess<'de> for VariantByKeyAccess<'de, T>
where
    T::Entry: 'de,
{
    type Error = DeserializeError;
    type Variant = VariantAccess<'de, T>;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant)> {
        let key = T::entry_key(self.entry);
        let variant = seed.deserialize(BorrowedStrDeserializer::new(key))?;
        Ok((
            variant,
            VariantAccess { content: Some(T::entry_value(self.entry)), source: self.source },
        ))
    }
}

struct VariantAccess<'de, T: Tree> {
    content: Option<&'de T>,
    source: Option<&'de str>,
}

impl<'de, T: Tree + 'de> de::VariantAccess<'de> for VariantAccess<'de, T> {
    type Error = DeserializeError;

    fn unit_variant(self) -> Result<()> {
        match self.content {
            None => Ok(()),
            Some(content) => {
                TreeDeserializer::new(content, self.source).deserialize_unit(UnitOnly)
            }
        }
    }

    fn newtype_variant_seed<S: de::DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value> {
        match self.content {
            Some(content) => seed.deserialize(TreeDeserializer::new(content, self.source)),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"newtype variant",
            )),
        }
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value> {
        match self.content {
            Some(content) => TreeDeserializer::new(content, self.source).deserialize_seq(visitor),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"tuple variant",
            )),
        }
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        match self.content {
            Some(content) => TreeDeserializer::new(content, self.source).deserialize_map(visitor),
            None => Err(de::Error::invalid_type(
                de::Unexpected::UnitVariant,
                &"struct variant",
            )),
        }
    }
}

/// Minimal visitor accepting only unit, for unit-variant content checks.
struct UnitOnly;

impl<'de> Visitor<'de> for UnitOnly {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("unit")
    }

    fn visit_unit<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
}

/// Deserialize `T` from a parsed tree, resolving error locations against `source` when
/// the tree carries spans.
pub(crate) fn deserialize_from_tree<'de, D, T>(
    tree: &'de D,
    source: Option<&'de str>,
) -> std::result::Result<T, DeserializeError>
where
    D: Tree,
    T: serde::Deserialize<'de>,
{
    T::deserialize(TreeDeserializer::new(tree, source))
}

impl Number {
    /// The exact-digits token map used by `deserialize_any`, shared with `Value`'s
    /// `Deserialize` impl so a `Value` round-trips its numbers losslessly through any
    /// deserializer speaking the serde_json protocol.
    pub(crate) fn is_number_token_key(key: &str) -> bool {
        key == NUMBER_TOKEN
    }
}
