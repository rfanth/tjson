//! [`Document`] — a TJSON tree that additionally carries what [`Value`] deliberately
//! discards: comments and presentation facts (string/key forms, table-ness).
//!
//! `Value` is the data model: six JSON-shaped variants, nothing else, fully open for
//! matching. `Document` is *data as written*: facts observed by the parser ride on the
//! nodes themselves (no path indexing — facts move with their nodes under mutation),
//! and generators can attach comments and presentation choices while building output.
//!
//! Doctrine: recording is mechanism, normalizing is policy. Everything here is a
//! descriptive observation of the source; whether the renderer honors or normalizes a
//! fact is decided by render-time policy, never at parse time. Facts are *choices*
//! (was a table, was quoted), never geometry (column widths, indent depths) — geometry
//! always belongs to the renderer.
//!
//! Nodes are semi-opaque by design: accessor methods rather than a public matchable
//! enum, because the annotation vocabulary will grow and opening a shape up later is
//! possible while closing one never is.

use std::str::FromStr;

use crate::error::Error;
use crate::number::Number;
use crate::tree::{
    ContainerFacts, EntryFacts, KeyForm, NodeRef, RawComment, ScalarFacts, StringFacts,
    StringForm, Tree,
};
use crate::value::{Entry, Value};

/// Where a comment sits horizontally when re-emitted.
///
/// Classified from the source: `Left` iff the comment was at column 0 *and* the thing
/// it comments is deeper than column 0; otherwise `AtLevel`. A `Left` comment stays at
/// the margin; an `AtLevel` comment follows its subject's indent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Placement {
    /// Pinned to column 0 regardless of the subject's depth.
    Left,
    /// Indented to the level of the thing it comments.
    #[default]
    AtLevel,
}

/// One full-line comment, attached to the node or entry that followed it in the source
/// (or to the document trailer when nothing followed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    text: String,
    placement: Placement,
}

impl Comment {
    /// Create an `AtLevel` comment. `text` may be given with or without the leading
    /// `//`; it is stored (and re-emitted) with one.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let text = if text.starts_with("//") { text } else { format!("// {text}") };
        Self { text, placement: Placement::AtLevel }
    }

    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// The comment text as written, including the leading `//`.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    fn classify(raw: RawComment, subject_level: usize) -> Self {
        // Left iff at the margin under an indented subject. A comment at its subject's
        // level — including level 0, where the two readings coincide — is AtLevel, the
        // interpretation that follows the subject if the tree is later re-nested.
        let placement = if raw.col == 0 && subject_level > 0 {
            Placement::Left
        } else {
            Placement::AtLevel
        };
        Self { text: raw.text, placement }
    }

    fn classify_all(raw: Vec<RawComment>, subject_level: usize) -> Vec<Self> {
        raw.into_iter().map(|c| Self::classify(c, subject_level)).collect()
    }
}

/// A parsed or constructed TJSON document: a [`Value`]-shaped tree whose nodes carry
/// comments and presentation facts. Obtain one with [`str::parse`] or
/// [`Document::from_value`]; project the plain data out with [`Document::to_value`].
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    root: DocNode,
}

impl Document {
    pub fn root(&self) -> &DocNode {
        &self.root
    }

    pub fn root_mut(&mut self) -> &mut DocNode {
        &mut self.root
    }

    /// Comments that trailed the document after the last value line.
    pub fn trailing_comments(&self) -> &[Comment] {
        &self.root.trailing_comments
    }

    pub fn push_trailing_comment(&mut self, comment: Comment) {
        self.root.trailing_comments.push(comment);
    }

    /// Project the plain data out, discarding comments and presentation facts.
    pub fn to_value(&self) -> Value {
        self.root.to_value()
    }

    /// Lift a plain [`Value`] into a `Document` with no comments and no recorded
    /// presentation facts (the renderer's normal policies apply everywhere).
    pub fn from_value(value: &Value) -> Self {
        Self { root: DocNode::lift(value) }
    }

    /// Render as TJSON. Comments are emitted and recorded presentation facts honored
    /// according to the policy knobs on `options` (`honor_string_forms`,
    /// `honor_key_forms`, `honor_tables`, `render_comments` — all on by default);
    /// everything else about the layout is recomputed by the renderer.
    pub fn to_tjson_with(&self, options: crate::RenderOptions) -> String {
        crate::render::Renderer::render(&self.root, &options)
    }
}

impl std::fmt::Display for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_tjson_with(crate::RenderOptions::default()))
    }
}

impl FromStr for Document {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let root = crate::parse::Parser::<DocNode>::parse_document(s, 0).map_err(Error::Parse)?;
        Ok(Self { root })
    }
}

/// One node of a [`Document`]: a value plus the comments and presentation facts the
/// parser observed for it (or a generator attached to it).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct DocNode {
    kind: DocKind,
    comments_before: Vec<Comment>,
    /// For strings: how the string was written. `None` means "no opinion" — the
    /// renderer's global policy decides.
    string_form: Option<StringForm>,
    /// For arrays: `Some(true)` was written as a table, `Some(false)` explicitly not.
    table: Option<bool>,
    /// Populated only on a document's root node; exposed through [`Document`].
    trailing_comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Default)]
enum DocKind {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<DocNode>),
    Object(Vec<DocEntry>),
}

impl DocNode {
    fn from_kind(kind: DocKind) -> Self {
        Self { kind, ..Self::default() }
    }

    pub fn null() -> Self {
        Self::from_kind(DocKind::Null)
    }

    pub fn bool(value: bool) -> Self {
        Self::from_kind(DocKind::Bool(value))
    }

    pub fn number(value: Number) -> Self {
        Self::from_kind(DocKind::Number(value))
    }

    pub fn string(value: impl Into<String>) -> Self {
        Self::from_kind(DocKind::String(value.into()))
    }

    pub fn array(items: Vec<DocNode>) -> Self {
        Self::from_kind(DocKind::Array(items))
    }

    pub fn object(entries: Vec<DocEntry>) -> Self {
        Self::from_kind(DocKind::Object(entries))
    }

    // ---- Kind accessors (semi-opaque: no public matchable enum) ----

    pub fn is_null(&self) -> bool {
        matches!(self.kind, DocKind::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.kind {
            DocKind::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<&Number> {
        match &self.kind {
            DocKind::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            DocKind::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn items(&self) -> Option<&[DocNode]> {
        match &self.kind {
            DocKind::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn items_mut(&mut self) -> Option<&mut Vec<DocNode>> {
        match &mut self.kind {
            DocKind::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn entries(&self) -> Option<&[DocEntry]> {
        match &self.kind {
            DocKind::Object(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn entries_mut(&mut self) -> Option<&mut Vec<DocEntry>> {
        match &mut self.kind {
            DocKind::Object(entries) => Some(entries),
            _ => None,
        }
    }

    // ---- Facts ----

    pub fn comments_before(&self) -> &[Comment] {
        &self.comments_before
    }

    pub fn push_comment_before(&mut self, comment: Comment) {
        self.comments_before.push(comment);
    }

    /// How this string was written, when this node is a string the parser saw (or a
    /// generator expressed an opinion about).
    pub fn string_form(&self) -> Option<StringForm> {
        self.string_form
    }

    pub fn set_string_form(&mut self, form: Option<StringForm>) {
        self.string_form = form;
    }

    /// Whether this array was written as a table (`Some(true)`), explicitly not
    /// (`Some(false)`), or carries no opinion (`None` — renderer heuristics decide).
    pub fn table(&self) -> Option<bool> {
        self.table
    }

    pub fn set_table(&mut self, table: Option<bool>) {
        self.table = table;
    }

    // ---- Projections ----

    pub fn to_value(&self) -> Value {
        match &self.kind {
            DocKind::Null => Value::Null,
            DocKind::Bool(b) => Value::Bool(*b),
            DocKind::Number(n) => Value::Number(n.clone()),
            DocKind::String(s) => Value::String(s.clone()),
            DocKind::Array(items) => Value::Array(items.iter().map(DocNode::to_value).collect()),
            DocKind::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|entry| Entry { key: entry.key.clone(), value: entry.value.to_value() })
                    .collect(),
            ),
        }
    }

    fn lift(value: &Value) -> Self {
        let kind = match value {
            Value::Null => DocKind::Null,
            Value::Bool(b) => DocKind::Bool(*b),
            Value::Number(n) => DocKind::Number(n.clone()),
            Value::String(s) => DocKind::String(s.clone()),
            Value::Array(items) => DocKind::Array(items.iter().map(DocNode::lift).collect()),
            Value::Object(entries) => DocKind::Object(
                entries
                    .iter()
                    .map(|entry| DocEntry::new(entry.key.clone(), DocNode::lift(&entry.value)))
                    .collect(),
            ),
        };
        Self::from_kind(kind)
    }
}

/// One key–value entry of an object [`DocNode`], carrying the key's presentation facts
/// and any comments that preceded the entry.
#[derive(Clone, Debug, PartialEq)]
pub struct DocEntry {
    key: String,
    key_form: Option<KeyForm>,
    comments_before: Vec<Comment>,
    value: DocNode,
}

impl DocEntry {
    pub fn new(key: impl Into<String>, value: DocNode) -> Self {
        Self { key: key.into(), key_form: None, comments_before: Vec::new(), value }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// How the key was written, when the parser saw it (or a generator chose).
    pub fn key_form(&self) -> Option<KeyForm> {
        self.key_form
    }

    pub fn set_key_form(&mut self, form: Option<KeyForm>) {
        self.key_form = form;
    }

    pub fn comments_before(&self) -> &[Comment] {
        &self.comments_before
    }

    pub fn push_comment_before(&mut self, comment: Comment) {
        self.comments_before.push(comment);
    }

    pub fn value(&self) -> &DocNode {
        &self.value
    }

    pub fn value_mut(&mut self) -> &mut DocNode {
        &mut self.value
    }
}

impl Tree for DocNode {
    type Entry = DocEntry;

    const KEEPS_COMMENTS: bool = true;

    fn new_null(_facts: ScalarFacts) -> Self {
        Self::null()
    }

    fn new_bool(value: bool, _facts: ScalarFacts) -> Self {
        Self::bool(value)
    }

    fn new_number(value: Number, _facts: ScalarFacts) -> Self {
        Self::number(value)
    }

    fn new_string(value: String, facts: StringFacts) -> Self {
        let mut node = Self::string(value);
        node.string_form = Some(facts.form);
        node
    }

    fn new_array(items: Vec<Self>, facts: ContainerFacts) -> Self {
        let mut node = Self::array(items);
        node.table = Some(facts.table);
        node
    }

    fn new_object(entries: Vec<Self::Entry>, _facts: ContainerFacts) -> Self {
        Self::object(entries)
    }

    fn new_entry(key: String, value: Self, facts: EntryFacts) -> Self::Entry {
        let mut entry = DocEntry::new(key, value);
        entry.key_form = Some(facts.key_form);
        entry
    }

    fn from_minimal_json(value: serde_json::Value, _facts: ContainerFacts) -> Self {
        // JSON spells strings and keys quoted, so the fragment's interior records
        // Quoted throughout — that is what the source physically says.
        fn convert(value: serde_json::Value) -> DocNode {
            match value {
                serde_json::Value::Null => DocNode::null(),
                serde_json::Value::Bool(b) => DocNode::bool(b),
                serde_json::Value::Number(n) => DocNode::number(Number(n.to_string())),
                serde_json::Value::String(s) => {
                    let mut node = DocNode::string(s);
                    node.string_form = Some(StringForm::Quoted);
                    node
                }
                serde_json::Value::Array(items) => {
                    let mut node = DocNode::array(items.into_iter().map(convert).collect());
                    node.table = Some(false);
                    node
                }
                serde_json::Value::Object(map) => DocNode::object(
                    map.into_iter()
                        .map(|(key, value)| {
                            let mut entry = DocEntry::new(key, convert(value));
                            entry.key_form = Some(KeyForm::Quoted);
                            entry
                        })
                        .collect(),
                ),
            }
        }
        convert(value)
    }

    fn attach_comments_before(node: &mut Self, comments: Vec<RawComment>, node_level: usize) {
        node.comments_before.extend(Comment::classify_all(comments, node_level));
    }

    fn attach_entry_comments(entry: &mut Self::Entry, comments: Vec<RawComment>, entry_level: usize) {
        entry.comments_before.extend(Comment::classify_all(comments, entry_level));
    }

    fn attach_trailing_comments(root: &mut Self, comments: Vec<RawComment>) {
        // Trailer comments have no subject; their own column is the only signal, and
        // re-emission at the root level makes Left/AtLevel coincide anyway.
        root.trailing_comments.extend(Comment::classify_all(comments, 0));
    }

    fn node(&self) -> NodeRef<'_, Self> {
        match &self.kind {
            DocKind::Null => NodeRef::Null,
            DocKind::Bool(b) => NodeRef::Bool(*b),
            DocKind::Number(n) => NodeRef::Number(n),
            DocKind::String(s) => NodeRef::String(s),
            DocKind::Array(items) => NodeRef::Array(items),
            DocKind::Object(entries) => NodeRef::Object(entries),
        }
    }

    fn entry_key(entry: &Self::Entry) -> &str {
        &entry.key
    }

    fn entry_value(entry: &Self::Entry) -> &Self {
        &entry.value
    }

    fn string_form(&self) -> Option<StringForm> {
        self.string_form
    }

    fn table_opinion(&self) -> Option<bool> {
        self.table
    }

    fn comments_before(&self) -> &[Comment] {
        &self.comments_before
    }

    fn trailing_comments(&self) -> &[Comment] {
        &self.trailing_comments
    }

    fn entry_key_form(entry: &Self::Entry) -> Option<KeyForm> {
        entry.key_form
    }

    fn entry_comments(entry: &Self::Entry) -> &[Comment] {
        &entry.comments_before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(input: &str) -> Document {
        input.parse().expect("test document must parse")
    }

    fn entry<'a>(node: &'a DocNode, key: &str) -> &'a DocEntry {
        node.entries()
            .expect("expected object")
            .iter()
            .find(|e| e.key() == key)
            .unwrap_or_else(|| panic!("no entry {key}"))
    }

    #[test]
    fn records_string_and_key_forms() {
        let d = doc("  bare: hello\n  quoted:\"world\"\n  \"qkey\":1");
        let root = d.root();
        assert_eq!(entry(root, "bare").value().string_form(), Some(StringForm::Bare));
        assert_eq!(entry(root, "bare").key_form(), Some(KeyForm::Bare));
        assert_eq!(entry(root, "quoted").value().string_form(), Some(StringForm::Quoted));
        assert_eq!(entry(root, "qkey").key_form(), Some(KeyForm::Quoted));
    }

    #[test]
    fn records_multiline_flavor() {
        use crate::tree::MultilineFlavor;
        let d = doc("  note: ``\n| first\n| second\n   ``");
        let form = entry(d.root(), "note").value().string_form();
        assert_eq!(form, Some(StringForm::Multiline(MultilineFlavor::Double)));
    }

    #[test]
    fn records_tables_and_plain_arrays() {
        let d = doc("  |a  |b  |\n  |1  |2  |");
        assert_eq!(d.root().table(), Some(true));

        let d = doc("  nums:  1, 2, 3");
        assert_eq!(entry(d.root(), "nums").value().table(), Some(false));
    }

    #[test]
    fn attaches_entry_comments_with_placement() {
        let d = doc("  a:1\n  // about b\n  b:2\n// left about c\n  c:3");
        let root = d.root();
        assert!(entry(root, "a").comments_before().is_empty());

        let b_comments = entry(root, "b").comments_before();
        assert_eq!(b_comments.len(), 1);
        assert_eq!(b_comments[0].text(), "// about b");
        assert_eq!(b_comments[0].placement(), Placement::AtLevel);

        let c_comments = entry(root, "c").comments_before();
        assert_eq!(c_comments[0].text(), "// left about c");
        assert_eq!(c_comments[0].placement(), Placement::Left);
    }

    #[test]
    fn attaches_comments_to_array_elements_and_table_rows() {
        let d = doc("  data:\n     one\n// note\n     two");
        let items = entry(d.root(), "data").value().items().expect("array");
        assert!(items[0].comments_before().is_empty());
        assert_eq!(items[1].comments_before()[0].text(), "// note");

        let d = doc("  |a  |\n  |1  |\n  // row note\n  |2  |");
        let rows = d.root().items().expect("table rows");
        assert_eq!(rows[1].comments_before()[0].text(), "// row note");
    }

    #[test]
    fn attaches_root_and_trailing_comments() {
        let d = doc("// header\n  a:1\n// trailer");
        // A document-leading comment attaches to the outermost node that follows it —
        // the root object — not to that object's first entry.
        let root_comments = d.root().comments_before();
        assert_eq!(root_comments.len(), 1);
        assert_eq!(root_comments[0].text(), "// header");
        // The root sits at level 0, where margin and level coincide: AtLevel by rule.
        assert_eq!(root_comments[0].placement(), Placement::AtLevel);
        assert!(entry(d.root(), "a").comments_before().is_empty());

        assert_eq!(d.trailing_comments().len(), 1);
        assert_eq!(d.trailing_comments()[0].text(), "// trailer");
    }

    #[test]
    fn dedent_comment_attaches_to_next_sibling() {
        // A comment before a dedent belongs to whatever comes next, wherever that is.
        let d = doc("  a:\n    x:1\n// about b\n  b:2");
        let b_comments = entry(d.root(), "b").comments_before();
        assert_eq!(b_comments.len(), 1);
        assert_eq!(b_comments[0].text(), "// about b");
    }

    #[test]
    fn minimal_json_interior_records_quoted_forms() {
        let d = doc("  [{\"a\":\"x\"}]");
        let outer = d.root().items().expect("outer array");
        let fragment = outer[0].items().expect("fragment array");
        let inner = entry(&fragment[0], "a");
        assert_eq!(inner.key_form(), Some(KeyForm::Quoted));
        assert_eq!(inner.value().string_form(), Some(StringForm::Quoted));
    }

    #[test]
    fn projection_round_trips_data() {
        let input = "// header\n  a:1.00\n  b: text\n  c:\n    [ 1, 2";
        let d = doc(input);
        let direct: Value = input.parse().expect("parses as Value");
        assert_eq!(d.to_value(), direct, "projection must equal the plain parse");

        let lifted = Document::from_value(&direct);
        assert_eq!(lifted.to_value(), direct);
        assert!(lifted.trailing_comments().is_empty());
    }

    // ---- Render round-trips: the formatter contract ----
    //
    // Byte identity is NOT the goal (geometry normalizes); preservation of comments
    // and recorded forms is. The property is: parse → render → reparse gives the same
    // data, the same comments, and the same forms.

    fn render_default(d: &Document) -> String {
        d.to_tjson_with(crate::RenderOptions::default())
    }

    #[test]
    fn round_trip_preserves_comments_and_data() {
        let input = concat!(
            "// header\n",
            "  a:1\n",
            "  // about b\n",
            "  b: two\n",
            "// left about c\n",
            "  c:3\n",
            "// trailer",
        );
        let d = doc(input);
        let rendered = render_default(&d);
        let reparsed = doc(&rendered);

        assert_eq!(reparsed.to_value(), d.to_value(), "data survives: {rendered}");
        assert_eq!(
            reparsed.root().comments_before(),
            d.root().comments_before(),
            "header survives: {rendered}"
        );
        assert_eq!(
            entry(reparsed.root(), "b").comments_before(),
            entry(d.root(), "b").comments_before(),
            "entry comment survives: {rendered}"
        );
        let c = entry(reparsed.root(), "c").comments_before();
        assert_eq!(c[0].placement(), Placement::Left, "Left placement survives: {rendered}");
        assert!(rendered.contains("\n// left about c\n"), "Left comment at margin: {rendered}");
        assert_eq!(reparsed.trailing_comments(), d.trailing_comments(), "trailer: {rendered}");
    }

    #[test]
    fn round_trip_preserves_string_and_key_forms() {
        let input = "  bare: hello\n  quoted:\"world\"\n  \"qkey\":1";
        let rendered = render_default(&doc(input));
        assert!(rendered.contains("bare: hello"), "bare stays bare: {rendered}");
        assert!(rendered.contains("quoted:\"world\""), "quoted stays quoted: {rendered}");
        assert!(rendered.contains("\"qkey\":1"), "quoted key stays quoted: {rendered}");

        // With honoring off, the global policy normalizes the quoted string to bare.
        let normalized = doc(input).to_tjson_with(
            crate::RenderOptions::default().honor_string_forms(false).honor_key_forms(false),
        );
        assert!(normalized.contains("quoted: world"), "normalized to bare: {normalized}");
        assert!(normalized.contains("qkey:1"), "key normalized to bare: {normalized}");
    }

    #[test]
    fn round_trip_preserves_multiline_flavor() {
        use crate::tree::MultilineFlavor;
        // Default style is Bold (``); a recorded Single flavor must survive honoring.
        let input = "  note: `\n    first\n    second\n   `";
        let rendered = render_default(&doc(input));
        let reparsed = doc(&rendered);
        assert_eq!(
            entry(reparsed.root(), "note").value().string_form(),
            Some(StringForm::Multiline(MultilineFlavor::Single)),
            "single-backtick flavor survives: {rendered}"
        );
    }

    #[test]
    fn round_trip_preserves_table_opinion() {
        // Two rows x two columns: below the default heuristics (3x3), so only the
        // recorded was-a-table fact can keep it a table.
        let input = "  |a  |b  |\n  |1  |2  |\n  |3  |4  |";
        let rendered = render_default(&doc(input));
        assert!(rendered.contains('|'), "forced table stays a table: {rendered}");
        let reparsed = doc(&rendered);
        assert_eq!(reparsed.to_value(), doc(input).to_value());

        // An array of objects written vertically must NOT be table-ified, even when
        // the heuristics would fire.
        let vertical = concat!(
            "  rows:\n",
            "  [ { a:1    b:2    c:3\n",
            "    { a:4    b:5    c:6\n",
            "    { a:7    b:8    c:9",
        );
        let rendered = render_default(&doc(vertical));
        assert!(!rendered.contains('|'), "vertical array stays vertical: {rendered}");
    }

    #[test]
    fn packed_array_comment_starts_new_run() {
        let input = "  data:  1, 2,\n// note\n    3, 4";
        let d = doc(input);
        let rendered = render_default(&d);
        let reparsed = doc(&rendered);
        assert_eq!(reparsed.to_value(), d.to_value(), "data survives: {rendered}");
        // The comment sits between two packed runs. It was at the margin in the source
        // (col 0 under elements at level 4 → Left), so it re-emits at the margin.
        assert!(rendered.contains("// note\n"), "comment survives: {rendered}");
        let comment_line = rendered.lines().find(|l| l.contains("// note")).unwrap();
        let after: Vec<&str> = rendered.lines().skip_while(|l| !l.contains("// note")).collect();
        assert!(after.len() >= 2, "packing resumes after the comment: {rendered}");
        assert!(comment_line.starts_with("//"), "Left placement stays at margin: {rendered}");

        // The same comment written at the elements' level classifies AtLevel and
        // re-emits at that level.
        let leveled = doc("  data:  1, 2,\n    // note\n    3, 4");
        let rendered = render_default(&leveled);
        let comment_line = rendered.lines().find(|l| l.contains("// note")).unwrap();
        assert!(comment_line.starts_with("    //"), "AtLevel at element indent: {rendered}");
    }

    #[test]
    fn table_row_comments_render_in_place() {
        let input = "  |a  |b  |\n  |1  |2  |\n  // row note\n  |3  |4  |";
        let d = doc(input);
        let rendered = render_default(&d);
        let reparsed = doc(&rendered);
        assert_eq!(reparsed.to_value(), d.to_value(), "table data survives: {rendered}");
        let lines: Vec<&str> = rendered.lines().collect();
        let note_pos = lines.iter().position(|l| l.contains("// row note")).unwrap();
        assert!(lines[note_pos - 1].contains('|'), "comment between rows: {rendered}");
        assert!(lines[note_pos + 1].contains('|'), "table continues after: {rendered}");
    }

    #[test]
    fn render_comments_false_strips_comments() {
        let input = "// header\n  a:1\n  // about b\n  b:2";
        let stripped = doc(input).to_tjson_with(
            crate::RenderOptions::default().render_comments(false),
        );
        assert!(!stripped.contains("//"), "comments stripped: {stripped}");
    }

    #[test]
    fn canonical_keeps_comments_and_normalizes_forms() {
        let input = "// header\n  quoted:\"world\"";
        let canonical = doc(input).to_tjson_with(crate::RenderOptions::canonical());
        assert!(canonical.contains("// header"), "canonical keeps comments: {canonical}");
        assert!(canonical.contains("quoted: world"), "canonical normalizes forms: {canonical}");
    }

    #[test]
    fn generators_can_attach_comments_and_facts() {
        let mut node = DocNode::object(vec![DocEntry::new("a", DocNode::number(1i64.into()))]);
        let entries = node.entries_mut().expect("object");
        entries[0].push_comment_before(Comment::new("generated by test"));
        assert_eq!(entries[0].comments_before()[0].text(), "// generated by test");
        assert_eq!(entries[0].comments_before()[0].placement(), Placement::AtLevel);
    }
}
