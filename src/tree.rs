//! The internal seam between the parser and the tree types it can grow.
//!
//! `Tree` is implemented by [`Value`] (drops every fact — monomorphization compiles the
//! plain path down to exactly the pre-trait code) and, later, by `Document` (keeps
//! presentation facts) and `SpannedValue` (keeps source spans for deserializer
//! diagnostics). The trait is sealed by `pub(crate)` visibility and must never become
//! public API: reshaping it is always a crate-internal affair.
//!
//! Design doctrine (see local/annotated-tree-plan.md): facts passed through here are
//! *descriptive observations* of the source — which string form appeared, whether an
//! array was written as a table, where a token sits in the input. What a consumer does
//! with them (preserve, normalize, ignore) is that consumer's policy, never decided here.

use crate::number::Number;
use crate::value::{Entry, Value};

/// A half-open byte range into the original parser input.
///
/// Byte offsets, not character positions: O(1) to produce and slice, and the currency
/// spoken by tooling substrates. Line/column for human display is derived on demand from
/// the parser's line table. `u32` halves the footprint; document sizes beyond 4 GiB are
/// rejected before parsing begins rather than silently mis-spanned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: u32,
    pub(crate) len: u32,
}

impl Span {
    pub(crate) fn new(start: usize, len: usize) -> Self {
        // Callers guarantee start/len come from an input already bounded by
        // Parser::parse_document's document size check, so these cannot truncate.
        Self { start: start as u32, len: len as u32 }
    }
}

/// Which concrete multiline glyph a string was written with.
///
/// This is the *observed flavor* (a fact), distinct from
/// [`MultilineStyle`](crate::MultilineStyle) (a rendering strategy with fallback
/// rules). The renderer maps an honored flavor onto its emission machinery, where the
/// usual safety fallbacks still apply.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultilineFlavor {
    /// One backtick: content at n+2 indent.
    Single,
    /// Two backticks: pipe-guarded content lines.
    Double,
    /// Three backticks: content at column 0.
    Triple,
}

/// How a string value was written in the source. Bare vs quoted is a user choice,
/// so it is recorded, not normalized away.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringForm {
    /// Space-prefixed bare string (` value`). Folding is geometry: a folded bare
    /// string records as `Bare`.
    Bare,
    /// JSON string (`"value"`). Folded JSON strings record as `Quoted`.
    Quoted,
    /// Backtick multiline string of the given flavor.
    Multiline(MultilineFlavor),
}

/// How an object key was written in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyForm {
    /// Bare key (`key:`).
    Bare,
    /// JSON string key (`"key":`).
    Quoted,
}

/// A comment line as captured by the parser, before placement classification: the raw
/// byte column of its `//` within the physical line, and the comment text from `//` to
/// end of line, exactly as written.
#[derive(Clone, Debug)]
pub(crate) struct RawComment {
    pub(crate) col: usize,
    pub(crate) text: String,
}

/// Facts observed while parsing a non-string scalar (null, bool, number).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScalarFacts {
    pub(crate) span: Span,
}

/// Facts observed while parsing a string value.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StringFacts {
    pub(crate) form: StringForm,
    pub(crate) span: Span,
}

/// Facts observed while parsing an array or object.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ContainerFacts {
    pub(crate) span: Span,
    /// `true` when the array was written in table syntax.
    pub(crate) table: bool,
}

/// Facts observed while parsing an object entry (the key side).
#[derive(Clone, Copy, Debug)]
pub(crate) struct EntryFacts {
    pub(crate) key_form: KeyForm,
    pub(crate) key_span: Span,
}

/// A borrowed view of one tree node, letting generic code branch on the node's kind
/// without knowing the concrete tree type. This is the inspect-side currency shared by
/// the deserializer and (later) the renderer.
pub(crate) enum NodeRef<'a, T: Tree> {
    Null,
    Bool(bool),
    Number(&'a Number),
    String(&'a str),
    Array(&'a [T]),
    Object(&'a [T::Entry]),
}

/// A tree the parser can grow and generic walkers can inspect. Build methods are
/// associated functions: the type itself is the builder, so `Parser<Value>` needs no
/// runtime state to decide what to construct.
///
/// Implementations that don't care about a fact drop the argument; the optimizer then
/// deletes the fact plumbing from that monomorphization entirely. The same applies on
/// the inspect side: the default `span()` accessors return `None` and compile away for
/// trees that keep no spans.
pub(crate) trait Tree: Sized {
    /// The object-entry type paired with this tree (`Entry` for `Value`).
    type Entry;

    /// `true` when this tree stores comments. Comment capture in the parser is gated on
    /// this constant, so trees that don't care compile the buffering away entirely.
    const KEEPS_COMMENTS: bool = false;

    // ---- Build half: called by the parser ----

    fn new_null(facts: ScalarFacts) -> Self;
    fn new_bool(value: bool, facts: ScalarFacts) -> Self;
    fn new_number(value: Number, facts: ScalarFacts) -> Self;
    fn new_string(value: String, facts: StringFacts) -> Self;
    fn new_array(items: Vec<Self>, facts: ContainerFacts) -> Self;
    fn new_object(entries: Vec<Self::Entry>, facts: ContainerFacts) -> Self;
    fn new_entry(key: String, value: Self, facts: EntryFacts) -> Self::Entry;

    /// Build from a parsed MINIMAL JSON fragment (a single-line escape hatch in the
    /// grammar). Implementations choose how source facts apply to the fragment's
    /// interior — e.g. an annotated tree may mark every interior string as `Quoted`,
    /// since that is how JSON spells strings.
    fn from_minimal_json(value: serde_json::Value, facts: ContainerFacts) -> Self;

    /// Attach comment lines that preceded this node in the source. `node_level` is the
    /// node's logical indent, used to classify placement (Left iff the comment sits at
    /// column 0 AND the node is deeper than 0; AtLevel otherwise).
    fn attach_comments_before(_node: &mut Self, _comments: Vec<RawComment>, _node_level: usize) {}

    /// Attach comment lines that preceded this object entry in the source.
    fn attach_entry_comments(
        _entry: &mut Self::Entry,
        _comments: Vec<RawComment>,
        _entry_level: usize,
    ) {
    }

    /// Attach comment lines that trailed the document, after the last value line.
    fn attach_trailing_comments(_root: &mut Self, _comments: Vec<RawComment>) {}

    // ---- Inspect half: called by generic walkers ----

    fn node(&self) -> NodeRef<'_, Self>;
    fn entry_key(entry: &Self::Entry) -> &str;
    fn entry_value(entry: &Self::Entry) -> &Self;

    /// Source span of this node, for trees that keep one.
    fn span(&self) -> Option<Span> {
        None
    }

    /// Source span of an entry's key, for trees that keep one.
    fn entry_key_span(_entry: &Self::Entry) -> Option<Span> {
        None
    }

    // ---- Presentation facts: defaults say "no opinion"; only Document overrides. ----

    /// How this string was written, for trees that record it.
    fn string_form(&self) -> Option<StringForm> {
        None
    }

    /// Whether this array was written as a table, for trees that record it.
    fn table_opinion(&self) -> Option<bool> {
        None
    }

    /// Comments preceding this node, for trees that keep them.
    fn comments_before(&self) -> &[crate::document::Comment] {
        &[]
    }

    /// Comments trailing the document, kept on the root node of trees that record them.
    fn trailing_comments(&self) -> &[crate::document::Comment] {
        &[]
    }

    /// How an entry's key was written, for trees that record it.
    fn entry_key_form(_entry: &Self::Entry) -> Option<KeyForm> {
        None
    }

    /// Comments preceding an entry, for trees that keep them.
    fn entry_comments(_entry: &Self::Entry) -> &[crate::document::Comment] {
        &[]
    }

    /// `true` when this node is a string. The parser needs this one inspection to
    /// enforce the string-only rule for two-space array packing.
    fn is_string(&self) -> bool {
        matches!(self.node(), NodeRef::String(_))
    }
}

impl Tree for Value {
    type Entry = Entry;

    fn new_null(_facts: ScalarFacts) -> Self {
        Value::Null
    }

    fn new_bool(value: bool, _facts: ScalarFacts) -> Self {
        Value::Bool(value)
    }

    fn new_number(value: Number, _facts: ScalarFacts) -> Self {
        Value::Number(value)
    }

    fn new_string(value: String, _facts: StringFacts) -> Self {
        Value::String(value)
    }

    fn new_array(items: Vec<Self>, _facts: ContainerFacts) -> Self {
        Value::Array(items)
    }

    fn new_object(entries: Vec<Self::Entry>, _facts: ContainerFacts) -> Self {
        Value::Object(entries)
    }

    fn new_entry(key: String, value: Self, _facts: EntryFacts) -> Self::Entry {
        Entry { key, value }
    }

    fn from_minimal_json(value: serde_json::Value, _facts: ContainerFacts) -> Self {
        Value::from_serde_json(value)
    }

    fn node(&self) -> NodeRef<'_, Self> {
        match self {
            Value::Null => NodeRef::Null,
            Value::Bool(b) => NodeRef::Bool(*b),
            Value::Number(n) => NodeRef::Number(n),
            Value::String(s) => NodeRef::String(s),
            Value::Array(items) => NodeRef::Array(items),
            Value::Object(entries) => NodeRef::Object(entries),
        }
    }

    fn entry_key(entry: &Self::Entry) -> &str {
        &entry.key
    }

    fn entry_value(entry: &Self::Entry) -> &Self {
        &entry.value
    }
}
