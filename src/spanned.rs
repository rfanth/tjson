//! `SpannedValue` — the span-keeping parse target behind `from_str`'s typed
//! deserialization. Mirrors [`Value`]'s shape and additionally records where each node
//! and each object key sits in the original input, so type-mismatch errors can point at
//! real TJSON source the way parse errors do.
//!
//! Spans are parse artifacts, not document facts: this type is internal, never public,
//! and never feeds the renderer. A tree built programmatically has no spans — that case
//! is served by deserializing from `&Value` instead.

use crate::number::Number;
use crate::tree::{
    ContainerFacts, EntryFacts, NodeRef, ScalarFacts, Span, StringFacts, Tree,
};

#[derive(Debug)]
pub(crate) struct SpannedValue {
    pub(crate) span: Span,
    pub(crate) kind: SpannedKind,
}

#[derive(Debug)]
pub(crate) enum SpannedKind {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<SpannedValue>),
    Object(Vec<SpannedEntry>),
}

#[derive(Debug)]
pub(crate) struct SpannedEntry {
    pub(crate) key: String,
    pub(crate) key_span: Span,
    pub(crate) value: SpannedValue,
}

impl Tree for SpannedValue {
    type Entry = SpannedEntry;

    fn new_null(facts: ScalarFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::Null }
    }

    fn new_bool(value: bool, facts: ScalarFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::Bool(value) }
    }

    fn new_number(value: Number, facts: ScalarFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::Number(value) }
    }

    fn new_string(value: String, facts: StringFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::String(value) }
    }

    fn new_array(items: Vec<Self>, facts: ContainerFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::Array(items) }
    }

    fn new_object(entries: Vec<Self::Entry>, facts: ContainerFacts) -> Self {
        Self { span: facts.span, kind: SpannedKind::Object(entries) }
    }

    fn new_entry(key: String, value: Self, facts: EntryFacts) -> Self::Entry {
        // KeyForm is a presentation fact; spans are all this target keeps.
        let _ = facts.key_form;
        SpannedEntry { key, key_span: facts.key_span, value }
    }

    fn from_minimal_json(value: serde_json::Value, facts: ContainerFacts) -> Self {
        // A MINIMAL JSON fragment is a single source token; every node inside it
        // shares the fragment's span. Diagnostics inside the fragment point at the
        // fragment, which is the finest truth the physical source offers.
        from_json_with_span(value, facts.span)
    }

    fn node(&self) -> NodeRef<'_, Self> {
        match &self.kind {
            SpannedKind::Null => NodeRef::Null,
            SpannedKind::Bool(b) => NodeRef::Bool(*b),
            SpannedKind::Number(n) => NodeRef::Number(n),
            SpannedKind::String(s) => NodeRef::String(s),
            SpannedKind::Array(items) => NodeRef::Array(items),
            SpannedKind::Object(entries) => NodeRef::Object(entries),
        }
    }

    fn entry_key(entry: &Self::Entry) -> &str {
        &entry.key
    }

    fn entry_value(entry: &Self::Entry) -> &Self {
        &entry.value
    }

    fn span(&self) -> Option<Span> {
        Some(self.span)
    }

    fn entry_key_span(entry: &Self::Entry) -> Option<Span> {
        Some(entry.key_span)
    }
}

fn from_json_with_span(value: serde_json::Value, span: Span) -> SpannedValue {
    let kind = match value {
        serde_json::Value::Null => SpannedKind::Null,
        serde_json::Value::Bool(b) => SpannedKind::Bool(b),
        serde_json::Value::Number(n) => SpannedKind::Number(Number(n.to_string())),
        serde_json::Value::String(s) => SpannedKind::String(s),
        serde_json::Value::Array(items) => SpannedKind::Array(
            items.into_iter().map(|item| from_json_with_span(item, span)).collect(),
        ),
        serde_json::Value::Object(map) => SpannedKind::Object(
            map.into_iter()
                .map(|(key, value)| SpannedEntry {
                    key,
                    key_span: span,
                    value: from_json_with_span(value, span),
                })
                .collect(),
        ),
    };
    SpannedValue { span, kind }
}

/// Resolve a span back to 1-based line/column plus the source line's text, for
/// ParseError-style display. Columns are counted in characters to match the parser's
/// error convention. O(input) — called only when an error is actually being reported.
pub(crate) fn locate(input: &str, span: Span) -> (usize, usize, String) {
    let target = span.start as usize;
    let mut line_start = 0usize;
    for (line_index, raw) in input.split('\n').enumerate() {
        let content_len = if raw.ends_with('\r') { raw.len() - 1 } else { raw.len() };
        let line_end = line_start + raw.len();
        // The target belongs to this line when it falls before the next line's start
        // (line-ending bytes included), or when this is the final line.
        if target <= line_start + content_len || input.len() <= line_end {
            let offset_in_line = target.saturating_sub(line_start).min(content_len);
            let column = raw[..offset_in_line].chars().count() + 1;
            return (line_index + 1, column, raw[..content_len].to_owned());
        }
        line_start = line_end + 1;
    }
    (1, 1, String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Parser;

    fn parse(input: &str) -> SpannedValue {
        Parser::<SpannedValue>::parse_document(input, 0).expect("test document must parse")
    }

    fn entry<'a>(value: &'a SpannedValue, key: &str) -> &'a SpannedEntry {
        match &value.kind {
            SpannedKind::Object(entries) => entries
                .iter()
                .find(|e| e.key == key)
                .unwrap_or_else(|| panic!("no entry {key}")),
            other => panic!("expected object, got {other:?}"),
        }
    }

    fn located(input: &str, span: Span) -> (usize, usize) {
        let (line, column, _) = locate(input, span);
        (line, column)
    }

    #[test]
    fn spans_point_at_scalar_tokens() {
        let input = "  name: Alice\n  age:30";
        let root = parse(input);

        let name = entry(&root, "name");
        // Key "name" starts at line 1 column 3; value "Alice" at column 9.
        assert_eq!(located(input, name.key_span), (1, 3));
        assert_eq!(located(input, name.value.span), (1, 9));
        assert_eq!(&input[name.value.span.start as usize..][..name.value.span.len as usize], "Alice");

        let age = entry(&root, "age");
        assert_eq!(located(input, age.key_span), (2, 3));
        assert_eq!(located(input, age.value.span), (2, 7));
        assert_eq!(&input[age.value.span.start as usize..][..age.value.span.len as usize], "30");
    }

    #[test]
    fn spans_track_packed_lines() {
        // Two entries on one line: the second entry's key and value spans must point
        // past the first, not at the line start.
        let input = "  a:1    b: two";
        let root = parse(input);
        let b = entry(&root, "b");
        assert_eq!(located(input, b.key_span), (1, 10));
        assert_eq!(located(input, b.value.span), (1, 13));
        assert_eq!(&input[b.value.span.start as usize..][..b.value.span.len as usize], "two");
    }

    #[test]
    fn spans_track_packed_array_elements() {
        let input = "  data:  10, 20, 300";
        let root = parse(input);
        let data = entry(&root, "data");
        let SpannedKind::Array(items) = &data.value.kind else {
            panic!("expected array");
        };
        assert_eq!(items.len(), 3);
        assert_eq!(located(input, items[0].span), (1, 10));
        assert_eq!(located(input, items[2].span), (1, 18));
        assert_eq!(&input[items[2].span.start as usize..][..items[2].span.len as usize], "300");
    }

    #[test]
    fn folded_values_span_their_opening_line() {
        let input = "  note: hello\n  / world\n  after:1";
        let root = parse(input);
        let note = entry(&root, "note");
        let (line, _, _) = locate(input, note.value.span);
        assert_eq!(line, 1, "folded value must be located on its opening line");

        let after = entry(&root, "after");
        assert_eq!(located(input, after.value.span), (3, 9));
    }

    #[test]
    fn multiline_strings_span_their_opener() {
        let input = "  note: ``\n| first\n| second\n   ``";
        let root = parse(input);
        let note = entry(&root, "note");
        let (line, column, _) = locate(input, note.value.span);
        assert_eq!((line, column), (1, 9), "span must be the opener glyph, not the body");
    }

    #[test]
    fn table_rows_span_their_lines() {
        let input = "  |a  |b   |\n  |1  | x  |\n  |2  | y  |";
        let root = parse(input);
        let SpannedKind::Array(rows) = &root.kind else { panic!("expected table array") };
        // The array itself spans the header line; each row spans its own line.
        let (header_line, _, _) = locate(input, root.span);
        assert_eq!(header_line, 1);
        let (row1_line, _, _) = locate(input, rows[0].span);
        let (row2_line, _, _) = locate(input, rows[1].span);
        assert_eq!((row1_line, row2_line), (2, 3));
    }

    #[test]
    fn crlf_input_locates_correctly() {
        let input = "  a:1\r\n  b:22\r\n";
        let root = parse(input);
        let b = entry(&root, "b");
        assert_eq!(located(input, b.value.span), (2, 5));
        assert_eq!(&input[b.value.span.start as usize..][..b.value.span.len as usize], "22");
    }

    #[test]
    fn minimal_json_interior_shares_fragment_span() {
        let input = "  [{\"a\":{\"b\":null},\"c\":3}]";
        let root = parse(input);
        let SpannedKind::Array(outer) = &root.kind else { panic!("expected array") };
        let SpannedKind::Array(fragment_items) = &outer[0].kind else {
            panic!("expected inner minimal-JSON array")
        };
        let inner_obj = &fragment_items[0];
        let (line, column, _) = locate(input, inner_obj.span);
        assert_eq!((line, column), (1, 3), "interior nodes point at the fragment");
    }
}
