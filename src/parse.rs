use std::marker::PhantomData;

use serde_json::Value as JsonValue;

use crate::number::Number;

use crate::error::ParseError;
use crate::position::{
    ARRAY_STARTER, ByteOffset, Column, DocumentOffset, FileIndent, Glyph, Leading, Line,
    LogicalIndent, Marker, OffColumnMarker, Opener,
};
use crate::tree::{NodeRef,
    ContainerFacts, EntryFacts, KeyForm, MultilineFlavor, RawComment, ScalarFacts, Span,
    BareForm, StringFacts, StringForm, Tree,
};
use crate::options::{
    ByteOrderMark, CommentPlacementError, MissingIndentMarker, MultilineMinimum, ParseOptions,
    TrailingSpaces,
};
use crate::util::*;

/// How a packed array line separates its elements.
///
/// `Undetermined` is not a missing answer, it is the first element's real state:
/// the separator that decides the packing comes *after* it. Naming a packing
/// before one has been seen is guessing, and the guess used to be "comma" --
/// which told the writer of a space packed line, containing no commas at all,
/// that the comma after a bare string becomes part of it.
/// How deeply a document may nest before the parser refuses it.
///
/// Recursive descent turns nesting depth into stack depth, and a document can
/// ask for more of it than the process has: past a few thousand levels the
/// parser died with a segmentation fault, which is not a failure a library may
/// hand its caller.
///
/// 128 because that is serde_json's own recursion limit. MINIMAL JSON inside a
/// document is parsed by serde_json and already stops there, so the two agree
/// rather than disagreeing somewhere past here. A floor, not a ceiling -- a
/// later release can raise it once descending costs less than a stack frame per
/// level.
///
/// Counted, not derived from the indent. Those are different quantities: a
/// ` /<` re-anchors the frame at a value's position and so adds a level of
/// indentation with no container behind it, which makes a document of true
/// depth 89 measure about 131 logical levels.
const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Packing {
    Undetermined,
    Comma,
    Space,
}

/// Where a value sits, for the parts of the reading that depend on it -- which
/// are mostly the diagnostics, because the advice for a badly spelled element is
/// different on each kind of line.
///
/// `ArrayLine` carries its packing rather than standing alone: the two packings
/// have opposite rules about bare strings (one forbids them, the other requires
/// them), so a message that knows only "some array line" has to guess which rule
/// it is explaining.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayLineValueContext {
    ArrayLine(Packing),
    ObjectValue,
    SingleValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Array,
    Object,
}

/// What a multiline string's line breaks *mean* when its body is put back
/// together into data.
///
/// Not the file's line ending. Those are two independent things and keeping them
/// apart is what lets a TJSON file survive a whole-file EOL conversion with its
/// data intact:
///
/// - The **file EOL** is a render option ([`crate::options::Eol`]). It
///   terminates every physical line, multiline body lines included, and it is
///   presentation -- disposable, and safe for `unix2dos` or `dos2unix` to change.
/// - The **LOCAL EOL INDICATOR** is this. It is written as *text* after the
///   opening backticks -- the four characters `\r\n`, or nothing for LF -- and
///   says what to join the body lines with when reconstructing the value.
///
/// The indicator being text rather than bytes is the whole trick. A tool that
/// rewrites every line ending in the file cannot touch it, so a string whose data
/// holds CRLF still reassembles as CRLF after the file has been converted to LF,
/// and vice versa. The data's EOL is a property of the data and travels with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum MultilineLocalEol {
    #[default]
    Lf,
    CrLf,
}

/// What the line after a fold turns out to be. See `classify_fold_next`.
enum FoldNext<'a> {
    /// A `/ ` continuation at the expected indent; carries the text after the
    /// marker.
    Continues(&'a str),
    /// A comment where a continuation could have been. The spec forbids a
    /// comment within a fold, so this is neither a continuation nor a clean
    /// end, and callers that consume a fold should report it as such.
        Comment,
    /// Anything else: the fold is over.
    Ends,
    /// A `/ ` continuation is on this line, but not at the indent that was asked
    /// about.
    ///
    /// Split out from [`Self::Ends`] because the two are opposite facts wearing
    /// one answer. `Ends` means the value finished; this means the value did
    /// *not* finish and the caller was looking in the wrong column. Merged, a
    /// caller asking "is this a folded key" hears "no" and builds an array
    /// element out of an object entry -- no error, a different document. That
    /// bug has been found once already, at the `child_indent` call site, and
    /// was fixed there rather than here.
    ///
    /// Carries nothing. It briefly held the indent it was found at, which no
    /// caller ever read: the one error built from this variant is
    /// [`Parser::stray_fold_marker`], which takes the line number and measures
    /// the column itself. A marker at an odd column has no structural indent to
    /// report anyway -- that line is refused by [`Leading::of`] before any fold
    /// walk reaches it.
    ContinuesElsewhere,
}

/// If `content` looks like an attempted key -- there is a colon on the line --
/// return the first bare key rule its text breaks.
///
/// The split is taken at the real separator, an ASCII colon, not at the first
/// colonlike character -- otherwise `ab\u{02D0}cd:1` would be measured as the
/// key `ab`, which is perfectly valid, and the colonlike that actually caused
/// the rejection would never be named.
fn check_attempted_bare_key(content: &str, forms: &ParseOptions) -> Result<(), BareKeyFault> {
    // No colon means this was never a key attempt, which reports as `Ok` for the
    // same reason a valid key does: the caller only wants a fault when there is one
    // to name, and "nothing to say about this line" is the same answer either way.
    let Some(end) = content.find(':') else {
        return Ok(());
    };
    check_bare_key(&content[..end], forms)
}

impl MultilineLocalEol {
    /// The bytes to join body lines with when rebuilding the value: `0A`, or
    /// `0D 0A`. Used only on the data side, never written to a file.
    ///
    /// [`Self::opener_suffix`] is the same fact spelled as text for a document to
    /// carry. Two representations, one meeting point, so the names say which is
    /// which rather than leaving a reader to compare the bodies.
    fn bytes(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    /// How the LOCAL EOL INDICATOR is spelled after the opening backticks: the
    /// literal characters `\r\n`, four of them, not the two bytes they name. LF
    /// is the default and writes nothing.
    ///
    /// Text on purpose. An EOL converter run over the whole file rewrites every
    /// line ending in it and leaves this untouched, which is why the data's EOL
    /// survives a conversion that changes the file's.
    pub(crate) fn opener_suffix(self) -> &'static str {
        match self {
            Self::Lf => "",
            Self::CrLf => "\\r\\n",
        }
    }
}


pub(crate) struct IndentFrame {
    /// Amount added to file indents to get logical (structural) indents.
    offset: usize,
    /// File column where the matching ` />` close glyph must appear.
    close_file_indent: FileIndent,
}

/// What a line turned out to be, when asked whether it closes the open frame.
///
/// [`Self::Misplaced`] carries where the closer belonged, because that is the
/// number the reader needs and the only place that knows it is the frame -- an
/// error raised further out has to guess, and the one that used to be raised
/// guessed the enclosing object's indent instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseGlyph {
    /// The frame closed. The caller advances past this line.
    Closed,
    /// Not a close glyph. Ordinary content, to be parsed as such.
    NotACloser,
    /// A ` />` that is not where this frame's closer belongs.
    Misplaced { expected: FileIndent },
    /// A ` />` with no ` /<` open to close.
    ///
    /// Split from [`Self::NotACloser`] for the reason [`Self::Misplaced`] was:
    /// the two are opposite facts. `NotACloser` means the line is ordinary
    /// content, which is legal wherever it sits; this means the line is a closer
    /// and there is nothing for it to close, which is legal nowhere. Merged, it
    /// reached whichever parser claimed the line next and was reported as that
    /// parser's disappointment -- `invalid object key` at one indent, a decent
    /// message at another, decided by position rather than by the fault.
    NothingOpen,
}

/// Tracks the active indent offset caused by ` /<` / ` />` glyphs.
pub(crate) struct IndentTracker {
    stack: Vec<IndentFrame>,
}

impl IndentTracker {
    fn new() -> Self {
        Self { stack: vec![] }
    }

    /// Current offset: amount added to file indents to get logical indents.
    fn offset(&self) -> usize {
        self.stack.last().map_or(0, |f| f.offset)
    }

    /// Cross from the file frame into the structural one. The only way in.
    fn to_logical(&self, file: FileIndent) -> LogicalIndent {
        file.shifted_right(self.offset())
    }

    /// Cross back out, for the column a glyph or a span must actually occupy.
    fn to_file(&self, logical: LogicalIndent) -> FileIndent {
        logical.shifted_left(self.offset())
    }

    /// Push a glyph context. `glyph` is the file column of the ` /<` line's
    /// structure, not counting the glyph's own leading space.
    ///
    /// Everything nested inside shifts by the glyph's *logical* column, which is
    /// why this goes through [`Self::to_logical`] rather than adding the current
    /// offset a second time by hand.
    fn push_glyph(&mut self, glyph: FileIndent) {
        self.stack.push(IndentFrame {
            offset: self.to_logical(glyph).as_shift(),
            close_file_indent: glyph,
        });
    }

    /// Does `line` close the current indent context? Pops the frame if so.
    ///
    /// Answers in three cases rather than yes/no. A ` />` at the wrong column is
    /// neither a close nor ordinary content, and a `bool` forces it to be read as
    /// one of the two -- it used to come back `false` and be reparsed as content,
    /// which is legal at any deeper indent, so a closer written two columns too
    /// far in silently became an array element and the document changed meaning
    /// with no diagnostic at all. The third case is that bug made unrepresentable.
    fn try_pop_close(&mut self, line: Line<'_>) -> CloseGlyph {
        // Spaces and then `/>` and nothing else: the shape of a closer, wherever
        // it happens to sit. Where it sits is the next question, not this one.
        if line.text().trim_start_matches(' ') != Glyph::IndentClose.body() {
            return CloseGlyph::NotACloser;
        }
        let Some(frame) = self.stack.last() else {
            // A closer with nothing open is a different complaint, and it used to
            // be deferred to "the caller that knows there is no frame to talk
            // about" -- which all three callers then dropped with
            // `NotACloser => {}`, leaving the line to be read as ordinary content
            // and diagnosed by whatever tried to claim it next. An obligation
            // stated in a comment and enforced nowhere; a variant instead, so the
            // caller has to answer for it.
            return CloseGlyph::NothingOpen;
        };
        let expected = frame.close_file_indent;
        // One test covers both halves: the line must reach the frame's indent,
        // and what sits there must be exactly the glyph -- whose own leading
        // space is the column the frame's indent names.
        let sits_where_it_belongs = line
            .byte_offset_of(expected)
            .is_some_and(|at| &line[at..] == Glyph::IndentClose.text());
        if sits_where_it_belongs {
            self.stack.pop();
            return CloseGlyph::Closed;
        }
        CloseGlyph::Misplaced { expected }
    }
}

pub(crate) struct Parser<'a, T: Tree> {
    input: &'a str,
    line_offsets: Vec<LineSpan>,
    line: usize,
    /// The caller's reading of the format. Consulted rather than copied apart:
    /// the lookalike sets it carries are what `is_*_like` here means.
    options: ParseOptions,
    /// The document's outermost structural level, crossed out of
    /// [`ParseOptions::start_indent`] once.
    ///
    /// That field is a bare `usize` because it is configuration; this is the same
    /// number as a position, and the crossing asserts what the rest of the parser
    /// then relies on -- that the outermost level is even, like every other. Done
    /// here rather than at each use, where eleven `LogicalIndent::new` calls each
    /// re-asserted it and any one of them could have been handed something else.
    start: LogicalIndent,
    /// How many containers deep the parse currently is.
    ///
    /// Held rather than derived, and kept in step with the call stack by the two
    /// tail wrappers alone -- neither has a `?` in it, so no path can skip the
    /// decrement.
    depth: usize,
    idt: IndentTracker,
    /// Comment lines seen but not yet attached to a node. Only populated when
    /// `T::KEEPS_COMMENTS`; drained at the next node-creating site, so a comment
    /// always attaches to the next structural thing after it.
    pending_comments: Vec<RawComment>,
    target: PhantomData<T>,
}

pub(crate) struct LineSpan {
    /// Byte offset of the first character of the line in the original input.
    start: DocumentOffset,
    /// Byte length of the line content, excluding any line-ending bytes (`\r\n` or `\n`).
    len: usize,
}

impl LineSpan {
    /// This line's content, borrowed from the input it was scanned from.
    ///
    /// The one place a document offset and a byte length are added together.
    /// Both are unambiguously bytes here -- the offset came from scanning this
    /// input and the length was measured on this line -- which is the only
    /// condition under which the arithmetic means anything.
    fn text<'a>(&self, input: &'a str) -> Line<'a> {
        let start = self.start.bytes();
        Line::new(&input[start..start + self.len])
    }
}

pub(crate) fn scan_lines(input: &str) -> std::result::Result<Vec<LineSpan>, ParseError> {
    let mut offsets = Vec::new();
    let mut pos = 0usize;
    for (line_index, raw) in input.split('\n').enumerate() {
        let len = if raw.ends_with('\r') { raw.len() - 1 } else { raw.len() };
        let content = &raw[..len];
        for (col, ch) in content.chars().enumerate() {
            // The reason travels into the message: which of the FORBIDDEN
            // CHARACTERS rules caught this is the whole of what a reader needs,
            // and `forbidden character U+200E` on its own tells them nothing
            // about what U+200E is or why it is out.
            if let Err(reason) = check_forbidden_literal(ch) {
                return Err(ParseError::new(
                    line_index + 1,
                    Column::one_based(col + 1),
                    format!("forbidden character: {}", reason.describe(ch)),
                    None,
                ));
            }
        }
        // A final newline terminates the last line rather than starting another, so
        // the empty tail `split('\n')` hands back is not a line of the document.
        // Keeping it made every loop that walks lines see one line that is not
        // there -- which is how an unterminated `` reported "body lines must start
        // with '| '" against a line that does not exist, instead of the
        // unterminated-string error that was already written and simply never
        // reached for any file ending the way files normally end.
        let is_phantom_tail = raw.is_empty() && line_index > 0 && pos == input.len();
        if !is_phantom_tail {
            offsets.push(LineSpan { start: DocumentOffset::new(pos), len });
        }
        // `split('\n')` consumes exactly the one byte it split on, and a CRLF
        // line's `\r` is still inside `raw` -- `len` above excludes it from the
        // content, `raw.len()` here does not. So this advances by the whole
        // physical line under either line ending, and the `1` is the separator
        // the split ate, not an assumption that a line ending is one byte.
        pos += raw.len() + 1;
    }
    Ok(offsets)
}

impl<'a, T: Tree> Parser<'a, T> {
    pub(crate) fn parse_document(
        input: &'a str,
        options: ParseOptions,
    ) -> std::result::Result<T, ParseError> {
        // Span offsets are stored as u32 (see tree::Span); bound the input before any
        // are produced so an oversized document fails loudly instead of mis-spanning.
        if input.len() > u32::MAX as usize {
            return Err(ParseError::new(1, Column::FIRST, "input larger than 4 GiB is not supported", None));
        }

        // Settled here, before anything reads a line, so the rest of the parser
        // never has to know the input might open with a character that is not
        // content. Byte 0 only: U+FEFF anywhere else is an invisible character
        // sitting inside data, and `scan_lines` refuses it under both readings.
        let input = match (input.strip_prefix('\u{FEFF}'), options.byte_order_mark) {
            (Some(rest), ByteOrderMark::Discard) => rest,
            (Some(_), ByteOrderMark::Reject) => {
                return Err(ParseError::new(
                    1,
                    Column::FIRST,
                    "this input opens with a byte order mark (U+FEFF), which TJSON has no \
                     place for. It is invisible, so the file looks identical to one that \
                     loads -- save it as UTF-8 without a BOM, which most editors offer as \
                     an encoding choice.",
                    None,
                ));
            }
            (None, _) => input,
        };

        let mut parser = Self {
            input,
            line_offsets: scan_lines(input)?,
            line: 0,
            start: LogicalIndent::new(options.start_indent),
            depth: 0,
            options,
            idt: IndentTracker::new(),
            pending_comments: Vec::new(),
            target: PhantomData,
        };
        parser.skip_ignorable_lines()?;
        if parser.line >= parser.line_offsets.len() {
            // A file with comments in it is not empty on screen, so saying only
            // "empty input" reads as the parser being broken. It is still the right
            // verdict -- comments have no JSON representation, so a document of
            // nothing but comments carries no value -- and saying which of the two
            // cases this is costs a `trim`.
            let only_comments = !input.trim().is_empty();
            return Err(ParseError::new(
                1,
                Column::FIRST,
                if only_comments {
                    "empty input: a TJSON document must contain a value, and this one has only comments"
                } else {
                    "empty input: a TJSON document must contain a value"
                },
                None,
            ));
        }
        let root_pending = parser.take_pending_comments();
        let mut value = parser.parse_root_value()?;
        if T::KEEPS_COMMENTS && !root_pending.is_empty() {
            T::attach_comments_before(&mut value, root_pending, parser.start);
        }
        parser.skip_ignorable_lines()?;
        if T::KEEPS_COMMENTS {
            let trailing = parser.take_pending_comments();
            if !trailing.is_empty() {
                T::attach_trailing_comments(&mut value, trailing);
            }
        }
        if parser.line < parser.line_offsets.len() {
            let current = parser.current_line().map_or("", Line::text).trim_start();
            let msg = if current.starts_with("/>") {
                "unexpected /> indent offset glyph: no previous matching /< indent offset glyph"
            } else if current.starts_with(Marker::Fold.text()) {
                "unexpected fold marker: no open string to fold"
            } else {
                // Two different mistakes arrive here and the line alone cannot
                // tell them apart, so the message names both rather than
                // guessing: a line that was meant to be part of the value above
                // but is not indented under it, and a genuine second value.
                // Stated as the rule that holds today -- one value per document
                // -- without claiming anything about what a later reading of
                // several values in one input might allow.
                "unexpected trailing content: the document's value ended above this line, so \
                 nothing on it belongs to that value. A TJSON document holds exactly one value \
                 -- either this line's indent is wrong and it should sit under the value above, \
                 or it is a second value and needs a document of its own."
            };
            return Err(parser.error_current(msg));
        }
        Ok(value)
    }

    // ---- Facts plumbing ----
    //
    // Spans handed to Tree constructors cover the token's bytes in the original input
    // when the parser can compute them cheaply (single-line tokens with a known column),
    // and degrade to the whole current line otherwise (fold continuations, folded table
    // rows — anything reassembled across lines). Columns threaded through the inline
    // consumption loops are raw byte offsets within the physical line, NOT logical
    // indents: spans always address real input bytes.

    fn line_span(&self, index: usize) -> Span {
        match self.line_offsets.get(index) {
            Some(line) => Span::new(line.start.bytes(), line.len),
            None => Span::default(),
        }
    }

    fn current_span(&self) -> Span {
        self.line_span(self.line)
    }

    /// Span of `len` bytes at byte column `col` of the current line; the whole current
    /// line when the caller lost column tracking (`col == None`).
    fn span_at(&self, col: Option<ByteOffset>, len: usize) -> Span {
        match (col, self.line_offsets.get(self.line)) {
            (Some(col), Some(line)) if col.bytes() <= line.len => {
                Span::new(line.start.plus(col.bytes()).bytes(), len.min(line.len - col.bytes()))
            }
            _ => self.current_span(),
        }
    }

    fn scalar_facts_at(&self, col: Option<ByteOffset>, len: usize) -> ScalarFacts {
        ScalarFacts { span: self.span_at(col, len) }
    }

    fn string_facts_at(&self, form: StringForm, col: Option<ByteOffset>, len: usize) -> StringFacts {
        StringFacts { form, span: self.span_at(col, len) }
    }

    fn container_facts_from(&self, span: Span) -> ContainerFacts {
        ContainerFacts { span, table: false }
    }

    fn container_facts(&self) -> ContainerFacts {
        ContainerFacts { span: self.current_span(), table: false }
    }

    fn entry_facts(&self, key_form: KeyForm) -> EntryFacts {
        EntryFacts { key_form, key_span: self.current_span() }
    }

    fn parse_root_value(&mut self) -> std::result::Result<T, ParseError> {
        let line = self
            .current_line()
            // Defensive: the constructor rejects a valueless document before this
            // runs, so reaching here means the line cursor moved without a line to
            // move to. Worded the same as that check so the two cannot look like
            // different faults.
            .ok_or_else(|| {
                ParseError::new(
                    1,
                    Column::FIRST,
                    "empty input: a TJSON document must contain a value",
                    None,
                )
            })?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let leading = line
            .leading()
            .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
        let indent = self.idt.to_logical(leading.indent);

        // An opener means the value is a bare string, whatever its text looks
        // like: `  [ [ 1` after an opening quote is the string "[ [ 1", not a
        // marker chain. `unopened` is that rule rather than a guard beside it --
        // there is no way to reach the text without answering the question.
        if indent == self.start
            && let Some(unopened) = leading.unopened(line)
            && starts_with_marker_chain(unopened)
        {
            return self.parse_marker_chain_line(unopened, indent);
        }

        // Standalone root-level start glyph: ` /<` one level below the root.
        let root_glyph = self.idt.to_file(self.start.deeper(1));
        if leading.opener == Opener::Glyph && leading.indent == root_glyph {
            self.idt.push_glyph(root_glyph);
            self.line += 1;
            self.skip_ignorable_lines()?;
            return self.parse_root_value();
        }

        // `<=` alone, where this used to allow one more column. Both operands are
        // even -- the type says so now -- so the slack could only ever have
        // mattered for an odd starting level, which cannot be constructed.
        if indent <= self.start {
            return self
                .parse_standalone_scalar_line(
                    self.content_at(line, self.start),
                    self.byte_offset_of(line, self.start)
                        .unwrap_or_else(|| line.end()),
                    self.start,
                );
        }

        if indent >= self.start.deeper(1) {
            let child_content = self.content_at(line, self.start.deeper(1));
            if self.looks_like_object_start(child_content, self.start.deeper(1))? {
                return self.parse_implicit_object(self.start);
            }
            return self.parse_implicit_array(self.start);
        }

        Err(self.error_current("expected a value at the starting indent"))
    }

    /// The two implicit-container constructors are the whole surface of
    /// [`MissingIndentMarker::RequireForced`]: every other container the parser
    /// builds either consumed a marker on its way in or is a spelling rather
    /// than a nesting (`[]`, `{}`, and the packed array a key opens with `  `,
    /// which `force_markers` deliberately leaves alone).
    ///
    /// The demand is always satisfiable without changing the document, which is
    /// what makes the reading coherent: a marker written where a container is
    /// already implied names that container instead of opening another one, so
    /// adding it moves no data. Opening a *new* marker slot would add a level --
    /// a different edit, and not the one this asks for.
    fn require_marker_error(&self, glyph: &str) -> ParseError {
        self.error_current(format!(
            "this container has no nesting marker, and the reading in force requires one at \
             every level. Write `{glyph}` at the start of this line -- a marker in this \
             position names the container the indentation already implies, so it does not \
             change the value or move anything deeper."
        ))
    }

    fn too_deep(&self) -> ParseError {
        self.error_current(format!(
            "this document nests more than {MAX_DEPTH} containers deep, which is as \
             far as this parser goes for now. The limit is serde_json's: MINIMAL JSON \
             inside a document is parsed by serde_json, which stops at {MAX_DEPTH} \
             itself, so this parser stops in the same place rather than somewhere \
             past it. Expect it to be raised in a future release."
        ))
    }

    fn parse_implicit_object(
        &mut self,
        parent_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        if self.options.missing_indent_marker == MissingIndentMarker::RequireForced {
            return Err(self.require_marker_error(Marker::Object.text()));
        }
        // Implicit containers have no opener token; their span is the line their first
        // entry starts on, captured before parsing moves past it.
        let open_span = self.current_span();
        let mut entries = Vec::new();
        self.parse_object_tail(parent_indent.deeper(1), &mut entries)?;
        if entries.is_empty() {
            return Err(self.error_current("expected at least one object entry"));
        }
        Ok(T::new_object(entries, self.container_facts_from(open_span)))
    }

    fn parse_implicit_array(
        &mut self,
        parent_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        if self.options.missing_indent_marker == MissingIndentMarker::RequireForced {
            return Err(self.require_marker_error(Marker::Array.text()));
        }
        self.skip_ignorable_lines()?;
        let elem_indent = parent_indent.deeper(1);
        let line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected array contents"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let leading = line
            .leading()
            .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
        let indent = self.idt.to_logical(leading.indent);
        if indent < elem_indent {
            return Err(self.error_current("expected array elements indented by two spaces"));
        }
        // A table's `|` is a cell edge and so structure, which means it only
        // counts where indentation ended. On a line that opened a value the `|`
        // is the value's first character, and a bare string may not begin with
        // one -- that is `check_bare_string`'s business to report, not a reason
        // to read the line as a table header.
        if leading.unopened(line).is_some_and(|content| content.starts_with('|')) {
            return self.parse_table_array(elem_indent);
        }
        let open_span = self.current_span();
        let mut elements = Vec::new();
        self.parse_array_tail(parent_indent, &mut elements)?;
        if elements.is_empty() {
            return Err(self.error_current("expected at least one array element"));
        }
        Ok(T::new_array(elements, self.container_facts_from(open_span)))
    }

    fn parse_table_array(
        &mut self,
        elem_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        let header_line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected a table header"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let header = self.content_at(header_line, elem_indent);
        let header_span = self.current_span();
        let columns = self.parse_table_header(header, elem_indent)?;
        self.line += 1;
        let mut rows = Vec::new();
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line() else {
                break;
            };
            match self.idt.try_pop_close(line) {
                CloseGlyph::Closed => {
                    self.line += 1;
                    continue;
                }
                CloseGlyph::Misplaced { expected } => {
                    return Err(self.misplaced_closer(line, expected));
                }
                CloseGlyph::NothingOpen => {
                    return Err(self.closer_with_nothing_open(line));
                }
                CloseGlyph::NotACloser => {}
            }
            self.ensure_line_has_no_tabs(self.line)?;
            let leading = line
                .leading()
                .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
            let indent = self.idt.to_logical(leading.indent);
            if indent < elem_indent {
                break;
            }
            if indent != elem_indent {
                return Err(self.error_current("expected a table row at the array indent"));
            }
            // `unopened`, not `text_start`: a row's leading `|` is structure and
            // has to sit where the indent ended. Reading it one column right --
            // past a bare string's opening quote -- accepted a row misaligned
            // from its own header and produced the same table either way.
            //
            // The two failures are told apart rather than merged: a line that
            // opened a value *is* a row, written one column over, and telling its
            // author that a table may only contain rows would be answering a
            // question they did not ask.
            let row = match leading.unopened(line) {
                Some(row) if row.starts_with('|') => row,
                Some(_) => {
                    return Err(self.error_current("table arrays may only contain table rows"));
                }
                None => {
                    let column = line.column_at(leading.content_start(line)).number();
                    return Err(self.error_current(format!(
                        "this row's `|` is at column {}, one column right of the table. A row's \
                         cell edges line up with the header's or the columns stop being columns, \
                         so every `|` on this line is one past where it belongs. Delete the space \
                         before it.",
                        column + 1,
                    )));
                }
            };
            // Collect fold continuation lines: `/ ` marker at pair_indent (elem_indent - 2),
            // two characters to the left of the opening `|` per spec.
            // Blank lines and `//` comments between a partial row and its continuation are
            // skipped. A parser would also be within its rights to reject them.
            let pair_indent = elem_indent.shallower(1);
            let mut row_owned = row.to_owned();
            // Taken before the fold loop, which advances `self.line` to the last
            // continuation. A row that stays on one line keeps this position; one
            // that folds gives it up below, because by then it names the wrong line.
            let row_start = self.byte_offset_past(elem_indent, 0);
            let mut folded = false;
            loop {
                // Peek past comments to find the next meaningful line, and
                // remember where the first one was.
                //
                // Spec: "A comment may not be within a fold." This used to skip
                // them silently, which discarded the comment -- neither
                // rejecting nor keeping it, and the only outcome nobody would
                // choose. Whether the comment turns out to be inside a fold is
                // not known until the next meaningful line is classified: if it
                // is a `/ ` continuation the comment was inside, and if it is
                // the next row the comment was legally between two of them.
                let mut offset = 1usize;
                let mut comment_line: Option<usize> = None;
                while let Some(peek) = self.line_str(self.line + offset) {
                    let trimmed = peek.text().trim_start_matches(' ');
                    if trimmed.starts_with("//") {
                        if comment_line.is_none() {
                            comment_line = Some(self.line + offset);
                        }
                        offset += 1;
                    } else {
                        break;
                    }
                }
                let cont_suffix = {
                    let Some(next_line) = self.line_str(self.line + offset) else {
                        break;
                    };
                    let next_leading = Leading::of(next_line).map_err(|fault| {
                        self.marker_off_column(self.line + offset, next_line, fault)
                    })?;
                    let next_indent = self.idt.to_logical(next_leading.indent);
                    if next_indent != pair_indent {
                        break;
                    }
                    // Measured on `next_line` and read from `next_line`. This
                    // used to slice with a position taken from `line`, the row
                    // above -- the two agreed only because both lines' leading
                    // columns happen to be spaces.
                    let Some(continued) = next_leading
                        .unopened(next_line)
                        .and_then(|content| Marker::Fold.strip(content))
                    else {
                        break;
                    };
                    continued.to_owned()
                };
                // A continuation was found, so any comment skipped to reach it
                // was inside this row's fold.
                if let Some(at) = comment_line {
                    self.comment_in_fold(
                        at,
                        "this comment sits between a table row and the `/ ` line \
                         continuing it -- move the comment above the row, or below it",
                    )?;
                }
                // Consume ignorable lines then the continuation line.
                for i in 1..offset {
                    self.ensure_line_has_no_tabs(self.line + i)?;
                }
                self.line += offset;
                self.ensure_line_has_no_tabs(self.line)?;
                row_owned.push_str(&cont_suffix);
                folded = true;
            }
            let pending = self.take_pending_comments();
            let mut parsed_row = self.parse_table_row(
                &columns,
                &row_owned,
                elem_indent,
                (!folded).then_some(row_start),
            )?;
            if T::KEEPS_COMMENTS && !pending.is_empty() {
                T::attach_comments_before(&mut parsed_row, pending, elem_indent);
            }
            rows.push(parsed_row);
            self.line += 1;
        }
        if rows.is_empty() {
            return Err(self.error_current("table arrays must contain at least one row"));
        }
        Ok(T::new_array(rows, ContainerFacts { span: header_span, table: true }))
    }

    fn parse_table_header(&self, row: &str, indent: LogicalIndent) -> std::result::Result<Vec<(String, KeyForm)>, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_indent(self.line, indent, "invalid table header"))?;
        if cells.first().is_some_and(|cell| cell.text.is_empty()) {
            cells.remove(0);
        }
        if !cells.last().is_some_and(|cell| cell.text.is_empty()) {
            return Err(self.error_at_line(self.line, self.byte_offset_past(indent, row.len()), "table header must end with \"  |\" (two spaces of padding then pipe)"));
        }
        cells.pop();
        if cells.is_empty() {
            return Err(self.error_at_line(self.line, ByteOffset::START, "table headers must list columns"));
        }
        // Each cell says where it starts, so the row's own position in the file is
        // the only crossing left. This used to walk a running total of cell lengths
        // and separator widths, which had to stay in step with the empty leading
        // cell removed above; and before that it was derived from the logical
        // indent, which left it unshifted under an active ` /<` and one column
        // right of the truth.
        cells
            .into_iter()
            .map(|cell| {
                let cell_col = self.byte_offset_past(indent, cell.at);
                self.parse_table_header_key(cell.text.trim_end(), cell_col)
            })
            .collect()
    }

    fn parse_table_header_key(&self, cell: &str, col: ByteOffset) -> std::result::Result<(String, KeyForm), ParseError> {
        if let Some(end) = parse_bare_key_prefix(cell, &self.options)
            && end == cell.len() {
                return Ok((cell.to_owned(), KeyForm::Bare));
            }
        if let Some((value, end)) = parse_json_string_prefix(cell)
            && end == cell.len() {
                return Ok((value, KeyForm::Quoted));
            }
        // "invalid table header key" names the construct and no rule, which is
        // the least useful thing to say about a cell that is nearly always one
        // space away from working. A header cell is a KEY, so the key rules
        // explain it -- and the one that catches people is padding: a table pads
        // on the right, and a leading space makes the cell start with a space,
        // which no bare key may do.
        if let Some(unpadded) = cell.strip_prefix(' ') {
            return Err(self.error_at_line(
                self.line,
                col,
                format!(
                    "a table header cell is a key, and a key cannot begin with a space. \
                     Pad a column on the right instead -- write `|{}` rather than `|{}`",
                    cell.trim_start_matches(' '),
                    if unpadded.is_empty() { " " } else { cell }
                ),
            ));
        }
        if let Err(fault) = check_bare_key(cell, &self.options) {
            return Err(self.error_at_line(
                self.line,
                col,
                format!("invalid table header key: {}", fault.describe()),
            ));
        }
        Err(self.error_at_line(self.line, col, "invalid table header key"))
    }

    /// `row_start` is where `row` begins in the current line's bytes, and `None`
    /// when the row was reassembled across a fold. A folded row's text spans
    /// several physical lines, so an offset into it points at no single line --
    /// and the parser has advanced to the last of them by now, so resolving one
    /// against `self.line` would land somewhere real and wrong. Cells carry no
    /// position in that case rather than a plausible false one.
    fn parse_table_row(
        &self,
        columns: &[(String, KeyForm)],
        row: &str,
        indent: LogicalIndent,
        row_start: Option<ByteOffset>,
    ) -> std::result::Result<T, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_indent(self.line, indent, "invalid table row"))?;
        if cells.first().is_some_and(|cell| cell.text.is_empty()) {
            cells.remove(0);
        }
        if !cells.last().is_some_and(|cell| cell.text.is_empty()) {
            return Err(self.error_at_line(self.line, self.byte_offset_past(indent, row.len()), "table row must end with \"  |\" (two spaces of padding then pipe)"));
        }
        cells.pop();
        if cells.len() != columns.len() {
            return Err(self.error_at_line(
                self.line,
                self.byte_offset_past(indent, row.len()),
                "table row has wrong number of cells",
            ));
        }
        let mut entries = Vec::new();
        for (index, (key, key_form)) in columns.iter().enumerate() {
            let cell = cells[index];
            let text = cell.text.trim_end();
            if text.is_empty() {
                continue;
            }
            // `trim_end` cannot move where the cell starts, so the offset still
            // stands. `row_start` is `None` for a folded row, and then so is this.
            let at = row_start.map(|start| start.plus(cell.at));
            let value = self.parse_table_cell_value(text, at)?;
            entries.push(T::new_entry(key.clone(), value, self.entry_facts(*key_form)));
        }
        Ok(T::new_object(entries, self.container_facts()))
    }

    /// `at` is where `cell` begins in the current line's bytes, or `None` when the
    /// row it came from was reassembled across a fold and no position in it maps
    /// to the file. Every fault below points at the cell, not at the row: which
    /// cell is wrong is the first thing a reader needs and the hardest to count
    /// out by eye in a wide table.
    fn parse_table_cell_value(
        &self,
        cell: &str,
        at: Option<ByteOffset>,
    ) -> std::result::Result<T, ParseError> {
        if cell.is_empty() {
            return Err(self.error_at_col(at, "empty table cells mean the key is absent"));
        }
        // Cell facts carry row-line spans: folded rows are reassembled strings, so
        // per-cell byte columns are not reliably recoverable from the physical line.
        // The cell's opening quote, whichever way it was written. `_` occupies
        // the same column as the space and says the same thing out loud.
        let cell_bare_form = if cell.starts_with('_') { BareForm::Marked } else { BareForm::Plain };
        if let Some(value) = cell.strip_prefix(' ').or_else(|| cell.strip_prefix('_')) {
            // The commonest way to reach here is padding on the wrong side, the
            // same mistake the header rejects: a column is padded on the right,
            // so a second leading space is not alignment, it is a second value.
            // Saying which side to pad teaches the rule; naming the broken rule
            // alone leaves the writer to guess which space was the extra one.
            if value.starts_with(' ') {
                let content = cell.trim_start_matches(' ');
                let width = cell.len();
                let padded = format!(" {content}{}", " ".repeat(width.saturating_sub(content.len() + 1)));
                return Err(self.error_at_col(
                    at,
                    format!(
                        "a table cell is padded on the right, not the left. The first \
                         space after the `|` is the bare string's opening quote, so the \
                         second one starts a second value -- write `|{padded}|` rather \
                         than `|{cell}|`, or `|{content}` with no space at all for a \
                         number or a boolean."
                    ),
                ));
            }
            if let Err(fault) = check_bare_string(value, &self.options) {
                return Err(self.error_at_col(
                    at,
                    format!("invalid bare string in table cell: {}", fault.describe()),
                ));
            }
            let facts = self.string_facts_at(StringForm::Bare(cell_bare_form), None, 0);
            return Ok(T::new_string(value.to_owned(), facts));
        }
        if let Some((value, end)) = parse_json_string_prefix(cell)
            && end == cell.len() {
                let facts = self.string_facts_at(StringForm::Quoted, None, 0);
                return Ok(T::new_string(value, facts));
            }
        if cell == "true" {
            return Ok(T::new_bool(true, self.scalar_facts_at(None, 0)));
        }
        if cell == "false" {
            return Ok(T::new_bool(false, self.scalar_facts_at(None, 0)));
        }
        if cell == "null" {
            return Ok(T::new_null(self.scalar_facts_at(None, 0)));
        }
        if cell == "[]" {
            return Ok(T::new_array(Vec::new(), self.container_facts()));
        }
        if cell == "{}" {
            return Ok(T::new_object(Vec::new(), self.container_facts()));
        }
        if let Ok(n) = cell.parse::<Number>() {
            return Ok(T::new_number(n, self.scalar_facts_at(None, 0)));
        }
        // MINIMAL JSON in a cell. Generators never emit this -- a column holding
        // arrays makes them abandon the table for block objects -- so it exists for
        // hand editing, and this is where the compact form earns its keep most:
        // unpacking a table to add one nested value means rewriting the whole
        // structure. It is safe because MINIMAL JSON is self-delimiting and admits
        // no whitespace, and `split_pipe_cells` already tracks string state, so a
        // `|` inside one of its strings does not split the cell.
        //
        // The leading-space rule still separates the two readings, as everywhere
        // else: `|[2,3]|` is the array, `| [2,3]|` is the string "[2,3]".
        if is_minimal_json_candidate(cell) && minimal_json_end(cell) == Some(cell.len()) {
            let span = self.span_at(None, cell.len());
            return self.parse_minimal_json_line(cell, span);
        }
        Err(self.error_at_col(at, "invalid table cell value"))
    }

    /// Read an object's entries, one container deeper.
    ///
    /// The whole wrapper is the depth accounting: increment, run the body, put it
    /// back. It carries no `?` deliberately -- every early return lives in
    /// [`Self::object_tail`], so there is no path out of here that skips the
    /// decrement, and the count cannot drift from the call stack.
    fn parse_object_tail(
        &mut self,
        pair_indent: LogicalIndent,
        entries: &mut Vec<T::Entry>,
    ) -> std::result::Result<(), ParseError> {
        self.depth += 1;
        let result = if self.depth > MAX_DEPTH {
            Err(self.too_deep())
        } else {
            self.object_tail(pair_indent, entries)
        };
        self.depth -= 1;
        result
    }

    fn object_tail(
        &mut self,
        pair_indent: LogicalIndent,
        entries: &mut Vec<T::Entry>,
    ) -> std::result::Result<(), ParseError> {
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line() else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            // Close glyph: pop offset and continue so the loop re-evaluates indent.
            match self.idt.try_pop_close(line) {
                CloseGlyph::Closed => {
                    self.line += 1;
                    continue;
                }
                CloseGlyph::Misplaced { expected } => {
                    return Err(self.misplaced_closer(line, expected));
                }
                CloseGlyph::NothingOpen => {
                    return Err(self.closer_with_nothing_open(line));
                }
                CloseGlyph::NotACloser => {}
            }
            let leading = line
                .leading()
                .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
            let indent = self.idt.to_logical(leading.indent);
            if indent < pair_indent {
                break;
            }
            if indent != pair_indent {
                let content = line[leading.text_start(line)..].to_owned();
                // Both indents are structural; a message names a column on the
                // page, and under an active ` /<` those are not the same number.
                let found_col = Column::of_indent(self.idt.to_file(indent));
                let want_col = Column::of_indent(self.idt.to_file(pair_indent));
                let msg = if content.starts_with(Glyph::IndentClose.body()) {
                    format!(
                        "misplaced {} indent offset glyph: found at column {}, expected at column {}",
                        Glyph::IndentClose.body(), found_col.number(), want_col.number(),
                    )
                } else if content.starts_with(Marker::Fold.text()) {
                    format!(
                        "misplaced fold marker: found at column {}, expected at column {}",
                        found_col.number(), want_col.number(),
                    )
                } else {
                    "expected an object entry at this indent".to_owned()
                };
                // At the indent the message names, not at the text after it. Both
                // numbers here are structural -- a glyph's leading space belongs to
                // the glyph, so its column is where the indent ends -- while
                // `error_current` points at the first character *past* that space.
                // The caret then sat one column right of the number in its own
                // prose, which is the one place a reader has no way to tell which
                // to believe.
                return Err(self.error_at_indent(self.line, indent, msg));
            }
            // An entry begins where the indentation ends. A line carrying an
            // opener has spent that column on a value, and a value is not an
            // entry -- an object holds nothing else -- so there is no reading
            // under which this line belongs here. It used to be sliced at
            // `text_start`, which handed the key parser the text one column
            // right and accepted every key written a space too far in.
            let Some(content) = leading.unopened(line) else {
                return Err(self.opener_where_an_entry_belongs(line, leading));
            };
            if content.is_empty() {
                return Err(self.error_current("blank lines are not valid inside objects"));
            }
            // Comments preceding this line attach to the line's first entry; comments
            // captured while parsing nested values drain at deeper sites.
            let pending = self.take_pending_comments();
            let mut line_entries =
                self.parse_object_line_content(content, pair_indent, Some(leading.content_start(line)))?;
            if T::KEEPS_COMMENTS
                && !pending.is_empty()
                && let Some(first) = line_entries.first_mut()
            {
                T::attach_entry_comments(first, pending, pair_indent);
            }
            entries.extend(line_entries);
        }
        Ok(())
    }

    fn parse_object_line_content(
        &mut self,
        content: &str,
        pair_indent: LogicalIndent,
        col0: Option<ByteOffset>,
    ) -> std::result::Result<Vec<T::Entry>, ParseError> {
        let mut rest = content.to_owned();
        // Byte column of `rest`'s first byte within the current physical line. Lost
        // (None) once a fold continuation moves part of the entry to another line.
        let mut col = col0;
        let mut entries = Vec::new();
        loop {
            let key_line = self.line;
            let prev_len = rest.len();
            let (key, key_form, after_colon) = self.parse_key(&rest, pair_indent)?;
            // A key that folded has no extent on any one line: `prev_len` was
            // measured where the key started and `after_colon` begins on a later
            // line, so their difference is not a length -- it is routinely
            // negative, which is what `  ""\n    [ { e\n        / :"` used to
            // panic on. There is nothing to measure against either, since the
            // column the key started at is gone the moment the fold happens.
            let key_facts;
            if self.line != key_line {
                col = None;
                key_facts = EntryFacts { key_form, key_span: self.span_at(None, 0) };
            } else {
                // Raw source extent of the key: everything before the colon, quotes included.
                let key_raw_len = prev_len - after_colon.len() - 1;
                key_facts = EntryFacts { key_form, key_span: self.span_at(col, key_raw_len) };
                col = col.map(|c| c.plus(key_raw_len + 1));
            }
            rest = after_colon;

            if rest.is_empty() {
                self.line += 1;
                let value = self.parse_value_after_key(pair_indent)?;
                entries.push(T::new_entry(key, value, key_facts));
                return Ok(entries);
            }

            // Inline indent glyph: `key: /<` — value follows on next lines at shifted indent.
            if rest == Glyph::IndentOpen.text() {
                let glyph = self.idt.to_file(pair_indent);
                self.idt.push_glyph(glyph);
                self.line += 1;
                let value = self.parse_value_after_key(pair_indent)?;
                entries.push(T::new_entry(key, value, key_facts));
                return Ok(entries);
            }

            // kv packing is only legal when every value on the line is a BASIC TYPE.
            if !entries.is_empty() && opens_array_starter_2(&rest) {
                return Err(self.error_at_col(
                    col,
                    "a packed array is not a BASIC TYPE, so it cannot share a line with \
                     another key-value pair; consider unpacking this line onto \
                     multiple lines",
                ));
            }

            let (value, consumed) =
                self.parse_inline_value(&rest, pair_indent, ArrayLineValueContext::ObjectValue, col)?;
            entries.push(T::new_entry(key, value, key_facts));

            let Some(consumed) = consumed else {
                return Ok(entries);
            };

            rest = rest[consumed..].to_owned();
            if rest.is_empty() {
                self.line += 1;
                return Ok(entries);
            }
            // Everything left is spaces, so nothing follows this entry and no
            // separator was intended -- these are trailing spaces, not a
            // malformed second pair, and saying otherwise describes a line the
            // author did not write.
            if rest.bytes().all(|b| b == b' ') {
                match self.options.trailing_spaces {
                    TrailingSpaces::Discard => {
                        self.line += 1;
                        return Ok(entries);
                    }
                    TrailingSpaces::Reject => {
                        return Err(self.error_current(
                            "this line ends with spaces that carry nothing -- delete them",
                        ));
                    }
                }
            }
            if !rest.starts_with("  ") {
                return Err(self
                    .error_current("expected at least two spaces between object entries on the same line"));
            }
            // Consume all leading spaces. Generators must produce even counts only;
            // a parser would be within its rights to reject an odd number of spaces here.
            let space_count = rest.bytes().take_while(|&b| b == b' ').count();
            rest = rest[space_count..].to_owned();
            col = col.map(|c| c.plus(consumed + space_count));
            if rest.is_empty() {
                match self.options.trailing_spaces {
                    TrailingSpaces::Discard => {
                        self.line += 1;
                        return Ok(entries);
                    }
                    TrailingSpaces::Reject => {
                        return Err(self.error_current(
                            "two or more spaces after an entry start another key-value pair \
                             on the same line, and none follows -- delete the trailing \
                             spaces, or add the entry they were separating",
                        ));
                    }
                }
            }
        }
    }

    fn parse_value_after_key(
        &mut self,
        pair_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        self.skip_ignorable_lines()?;
        let child_indent = pair_indent.deeper(1);
        let line = self
            .current_line()
            .ok_or_else(|| self.error_at_line(self.line, ByteOffset::START, "expected a nested value"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let leading = line
            .leading()
            .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
        let indent = self.idt.to_logical(leading.indent);
        // Both branches below look for a marker, so both ask `unopened`: a marker
        // stands in the indent, and on a line that opened a value the indent is
        // already spent.
        let unopened = leading.unopened(line);
        if let Some(unopened) = unopened
            && starts_with_marker_chain(unopened)
            && (indent == pair_indent || indent == child_indent)
        {
            return self.parse_marker_chain_line(unopened, indent);
        }
        // Fold after colon: value starts on a "/ " continuation line at pair_indent.
        // Spec: key and basic value are folded as a single unit; fold marker is allowed
        // immediately after the ":" (preferred), treating the junction at pair_indent+2 indent.
        if indent == pair_indent
            && let Some(continuation_content) = unopened.and_then(|it| Marker::Fold.strip(it))
        {
            let (value, consumed) = self.parse_inline_value(
                continuation_content,
                pair_indent,
                ArrayLineValueContext::ObjectValue,
                Some(leading.content_start(line).plus(Marker::Fold.width())),
            )?;
            if consumed.is_some() {
                self.line += 1;
            }
            return Ok(value);
        }
        // Own-line indent glyph: ` /<` whose structure sits at pair_indent. The
        // glyph's leading space is its own first character, not indentation, so
        // the indent here is pair_indent exactly rather than one past it.
        if leading.opener == Opener::Glyph && indent == pair_indent {
            let glyph = self.idt.to_file(pair_indent);
            self.idt.push_glyph(glyph);
            self.line += 1;
            return self.parse_value_after_key(pair_indent);
        }
        if indent < child_indent {
            return Err(self.error_current("nested values must be indented by two spaces"));
        }
        // A value more than one level below its key. TJSON writes that depth as a
        // marker chain (`[ [ 3`), so under the specification's reading this is an
        // error, reported by the container walk below exactly as it always was.
        //
        // Under `Infer` the chain is read off the indentation instead. Only the
        // deepest level can be an object -- an object cannot sit directly inside
        // an object, having nowhere to put the key -- so every level above it is
        // an array, and the deepest is settled by recursing, which puts it
        // through the very test that settles an ordinary one-level nesting.
        //
        // The gap is a whole number of levels: `Leading` has already taken any
        // opening quote out of `indent`, so a bare string's one-sided quote is
        // not in this arithmetic and there is nothing to round away.
        let extra_levels =
            indent.levels_below(child_indent);
        if extra_levels > 0 && self.options.missing_indent_marker == MissingIndentMarker::Infer {
            // The synthesized arrays have no line of their own, so they take the
            // span of the line their one element starts on -- which is what every
            // other implicit container here does.
            let open_span = self.current_span();
            // One level short of the gap: the loop below wraps the value once per
            // level, and this is the innermost of them. Guarded by `extra_levels > 0`
            // above, so there is a level to step back from.
            let mut value = self.parse_value_after_key(child_indent.deeper(extra_levels - 1))?;
            for _ in 0..extra_levels {
                value = T::new_array(vec![value], self.container_facts_from(open_span));
            }
            return Ok(value);
        }
        let content = self.content_at(line, child_indent);
        // `content` sits at `child_indent`, so that is where its fold continuations
        // are too. Passing `pair_indent` here looked for them two columns to the
        // left and never found them, so a folded key on a nested line was not
        // recognised as opening an object.
        if self.looks_like_object_start(content, child_indent)? {
            self.parse_implicit_object(pair_indent)
        } else {
            self.parse_implicit_array(pair_indent)
        }
    }

    /// `content_at` is where `content` begins in its line's bytes. It is a
    /// parameter rather than something rederived here because only the caller
    /// holds the line, and the two numbers it would otherwise be guessed from --
    /// a logical indent and a byte position -- agree only when no ` /<` is active
    /// and the indent is ASCII.
    fn parse_standalone_scalar_line(
        &mut self,
        content: &str,
        content_at: ByteOffset,
        line_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        // Spec: MINIMAL JSON "must be on a line by itself ... nothing may come after
        // it on that line". So a candidate takes the whole line; if it does not parse
        // as such, that is an error rather than a packed element.
        if is_minimal_json_candidate(content) {
            let span = self.span_at(Some(content_at), content.len());
            let value = self.parse_minimal_json_line(content, span)?;
            self.line += 1;
            return Ok(value);
        }
        let (value, consumed) = self.parse_inline_value(
            content,
            line_indent,
            ArrayLineValueContext::SingleValue,
            Some(content_at),
        )?;
        if let Some(consumed) = consumed {
            if consumed != content.len() {
                return Err(self.error_current("only one value may appear here"));
            }
            self.line += 1;
        }
        Ok(value)
    }

    /// Read an array's elements, one container deeper. See
    /// [`Self::parse_object_tail`] for why the wrapper has no `?` in it.
    fn parse_array_tail(
        &mut self,
        parent_indent: LogicalIndent,
        elements: &mut Vec<T>,
    ) -> std::result::Result<(), ParseError> {
        self.depth += 1;
        let result = if self.depth > MAX_DEPTH {
            Err(self.too_deep())
        } else {
            self.array_tail(parent_indent, elements)
        };
        self.depth -= 1;
        result
    }

    fn array_tail(
        &mut self,
        parent_indent: LogicalIndent,
        elements: &mut Vec<T>,
    ) -> std::result::Result<(), ParseError> {
        let elem_indent = parent_indent.deeper(1);
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line() else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            // Close glyph: pop offset and continue.
            match self.idt.try_pop_close(line) {
                CloseGlyph::Closed => {
                    self.line += 1;
                    continue;
                }
                CloseGlyph::Misplaced { expected } => {
                    return Err(self.misplaced_closer(line, expected));
                }
                CloseGlyph::NothingOpen => {
                    return Err(self.closer_with_nothing_open(line));
                }
                CloseGlyph::NotACloser => {}
            }
            let leading = line
                .leading()
                .map_err(|fault| self.marker_off_column(self.line, line, fault))?;
            let indent = self.idt.to_logical(leading.indent);
            if indent < parent_indent {
                break;
            }
            if let Some(unopened) = leading.unopened(line)
                && starts_with_marker_chain(unopened)
                && indent == elem_indent
            {
                elements.push(self.parse_marker_chain_line(unopened, indent)?);
                continue;
            }
            if indent < elem_indent {
                break;
            }
            // An opener is the value's own first column, not indentation, so a
            // line carrying one sits at `elem_indent` exactly. Which kind it is
            // was settled when the line was measured.
            if leading.opener.is_present() && indent == elem_indent {
                if leading.opener == Opener::Glyph {
                    self.idt.push_glyph(leading.indent);
                    self.line += 1;
                    continue;
                }
                let pending = self.take_pending_comments();
                let first_new = elements.len();
                self.parse_array_line_content(
                    &line[leading.content_start(line)..],
                    elem_indent,
                    elements,
                    Some(leading.content_start(line)),
                )?;
                if T::KEEPS_COMMENTS
                    && !pending.is_empty()
                    && let Some(first) = elements.get_mut(first_new)
                {
                    T::attach_comments_before(first, pending, elem_indent);
                }
                continue;
            }
            // Standalone glyph one level below the elements: introduces a nested sub-array.
            if leading.opener == Opener::Glyph && indent == elem_indent.deeper(1) {
                self.idt.push_glyph(leading.indent);
                let open_span = self.current_span();
                let pending = self.take_pending_comments();
                self.line += 1;
                let mut sub_elements = Vec::new();
                self.parse_array_tail(elem_indent, &mut sub_elements)?;
                let mut sub_array = T::new_array(sub_elements, self.container_facts_from(open_span));
                if T::KEEPS_COMMENTS && !pending.is_empty() {
                    T::attach_comments_before(&mut sub_array, pending, elem_indent);
                }
                elements.push(sub_array);
                continue;
            }
            // One step in from the element column: a container that is the next
            // element, with its marker left off. Nothing is missing from the
            // page here -- one step is a step the reader sees directly, which
            // is why the marker is required only beyond one and not at it. The
            // kind is settled by the same question that settles an ordinary
            // one-level nesting, and the whole run of lines at that depth goes
            // inside the one container: a second container at the same depth
            // would need a marker to say it starts, and there is none.
            //
            // The generator still writes the marker here. A scalar element
            // above does not frame what follows it the way a key does, and a
            // bare string element sits one column right of its structural
            // position, close enough to what follows that the step stops
            // reading as a step. Neither makes the document unreadable, so
            // neither is grounds to refuse it -- they are reasons to be
            // explicit when writing, not reasons to reject when reading.
            // `!elements.is_empty()` is the whole condition, not a guard on it:
            // a step is only a step if there is something to step in from. With
            // an element already at this column, the reader sees a line and then
            // a line one in, and reads the second as inside the first. With
            // nothing at this column yet, the first thing on the page is already
            // deeper than the level it belongs to, and no step is visible -- that
            // is a jump, and jumps are spelled with markers.
            //
            // What the step contains is settled from `unopened`, so a line that
            // opened a value never gets asked. An opener says the line is a bare
            // string and says it before any of its text is read; slicing past it
            // handed `x: y` to the object test and made ` x: y` a container while
            // `_x: y` -- the same opener written as a marker -- stayed a string.
            // Two spellings of one thing have to read alike, so the `None` arm
            // takes the array branch, which is where a scalar element belongs.
            if indent == elem_indent.deeper(1) && !elements.is_empty() {
                let opens_object = match leading.unopened(line) {
                    Some(unopened) => self.looks_like_object_start(unopened, indent)?,
                    None => false,
                };
                let pending = self.take_pending_comments();
                let mut nested = if opens_object {
                    self.parse_implicit_object(elem_indent)?
                } else {
                    self.parse_implicit_array(elem_indent)?
                };
                if T::KEEPS_COMMENTS && !pending.is_empty() {
                    T::attach_comments_before(&mut nested, pending, elem_indent);
                }
                elements.push(nested);
                continue;
            }
            if indent != elem_indent {
                return Err(self.error_current("invalid indent level: array elements must be indented by exactly two spaces"));
            }
            let content = &line[leading.text_start(line)..];
            if content.is_empty() {
                return Err(self.error_current("blank lines are not valid inside arrays"));
            }
            if content.starts_with('|') {
                return Err(self.error_current("table arrays are only valid as the entire array"));
            }
            let pending = self.take_pending_comments();
            let first_new = elements.len();
            // Spec: MINIMAL JSON owns its line -- it may never be packed with
            // another value, so it is never an element of a packed array.
            if is_minimal_json_candidate(content) {
                let span = self.span_at(Some(leading.text_start(line)), content.len());
                elements.push(self.parse_minimal_json_line(content, span)?);
                self.line += 1;
            } else {
                self.parse_array_line_content(content, elem_indent, elements, Some(leading.text_start(line)))?;
            }
            // Comments preceding this line attach to the line's first element.
            if T::KEEPS_COMMENTS
                && !pending.is_empty()
                && let Some(first) = elements.get_mut(first_new)
            {
                T::attach_comments_before(first, pending, elem_indent);
            }
        }
        Ok(())
    }

    /// Apply the trailing-space policy to a line remainder that is nothing but
    /// spaces, reporting whether the line is finished because they were discarded.
    ///
    /// One home for the question, because it is asked from several points along a
    /// line and the answer must not depend on which one arrived first.
    ///
    /// Only ever asked about text *between* values. Spaces that are data never
    /// reach here: a multiline body consumes its own lines, and a fold's two
    /// halves are joined by whatever sits at the end of the first one -- `hello `
    /// folded onto `world` is "hello world", and the same run held alone is a
    /// string of spaces. Those are values, not leftovers.
    fn trailing_spaces_end_the_line(
        &self,
        rest: &str,
        at: Option<ByteOffset>,
    ) -> std::result::Result<bool, ParseError> {
        if rest.is_empty() || !rest.bytes().all(|b| b == b' ') {
            return Ok(false);
        }
        match self.options.trailing_spaces {
            TrailingSpaces::Discard => Ok(true),
            TrailingSpaces::Reject => Err(self.error_at_col(
                at,
                "this line ends with spaces that carry nothing -- delete them",
            )),
        }
    }

    fn parse_array_line_content(
        &mut self,
        content: &str,
        elem_indent: LogicalIndent,
        elements: &mut Vec<T>,
        col0: Option<ByteOffset>,
    ) -> std::result::Result<(), ParseError> {
        let mut rest = content;
        // Both packed formats are homogeneous, in opposite directions, and the parser
        // must reject a violation rather than quietly reading it some other way.
        //
        // Array format 2 REQUIRES "either all BARE STRINGS that do not contain
        // commas, or no BARE STRINGS at all". Without that, `  word,  word, 5,
        // word, 6` parses as three bare strings two of which contain commas --
        // legal to the walk, but almost certainly an editing mistake and
        // unreadable either way.
        //
        // Array format 3 REQUIRES every element on the line to be a bare string.
        // A quoted element carries no leading space, so the gaps alternate three
        // and two across the line and the eye reads irregular spacing rather than
        // two kinds of element. There is no reading of that which is worth
        // preserving, and the fix is always the same: give the value its own line.
        //
        // An opener at the element position is exactly "this element is a bare
        // string": it is the one-sided opening quote, written as a space or, when
        // the writer wants it visible, as `_`. Both occupy the same column, so the
        // alternating-gap problem above is the same either way.
        let mut comma_packed = false;
        let mut space_packed = false;
        let mut saw_bare = false;
        let mut bare_scalar: Option<String> = None;
        let mut saw_non_bare = false;
        let mut bare_holds_comma = false;
        // `rest` is always a suffix of `content`, so where it currently starts is
        // recoverable from how much has been consumed.
        //
        // Asked at each fault rather than once per element, because the two are
        // different positions: the element's column goes stale the moment `rest`
        // advances past it, and the separator faults below are about what comes
        // after. They used to answer that by pointing at the start of the line --
        // a trailing space at column 22 reported at column 3.
        let at = |rest: &str| col0.map(|c| c.plus(content.len() - rest.len()));
        loop {
            let col = at(rest);
            // What this element can be told about the line it is on. Both flags
            // can be set at once by a line that mixes separators; the checks below
            // reject that, and the space packed rule is the stricter of the two,
            // so it is the one worth explaining.
            let packing = if space_packed {
                Packing::Space
            } else if comma_packed {
                Packing::Comma
            } else {
                Packing::Undetermined
            };
            let element_is_bare = rest.starts_with(' ') || rest.starts_with('_');
            let (value, consumed) = self.parse_inline_value(
                rest,
                elem_indent,
                ArrayLineValueContext::ArrayLine(packing),
                col,
            )?;
            if element_is_bare {
                saw_bare = true;
                if let NodeRef::String(text) = value.node() {
                    if text.contains(',') {
                        bare_holds_comma = true;
                    }
                    // Remembered for the diagnostic below: a bare string that
                    // reads as a scalar is almost always a scalar with one space
                    // too many in front of it, and saying so beats describing the
                    // rule it broke.
                    if bare_scalar.is_none() && scalar_spelling(text).is_some() {
                        bare_scalar = Some(text.to_owned());
                    }
                }
            } else {
                saw_non_bare = true;
            }
            // Checked after every element, not only at separators: the offending
            // element is often the last one, which no separator follows.
            if comma_packed {
                if saw_bare && saw_non_bare {
                    return Err(self.mixed_pack_error(col, bare_scalar.as_deref()));
                }
                if bare_holds_comma {
                    return Err(self.bare_comma_error(col));
                }
            }
            if space_packed && saw_non_bare {
                return Err(self.error_at_col(
                    col,
                    "a space separated packed array requires every element to be a bare \
                     string; consider unpacking this line onto multiple lines",
                ));
            }
            elements.push(value);
            let Some(consumed) = consumed else {
                return Ok(());
            };
            rest = &rest[consumed..];
            if rest.is_empty() {
                self.line += 1;
                return Ok(());
            }
            // Nothing but spaces left, so no element follows and no separator was
            // intended: these are the line's trailing spaces, and what happens to
            // them is the caller's policy.
            //
            // The object entry loop has asked this since the option existed; the
            // element loop never did, so trailing spaces here reached the
            // separator tests and came back as a complaint about separators --
            // and `Discard` could not discard them, because nothing on this path
            // ever consulted it.
            if self.trailing_spaces_end_the_line(rest, at(rest))? {
                self.line += 1;
                return Ok(());
            }
            if rest == "," {
                self.line += 1;
                return Ok(());
            }
            if let Some(next) = rest.strip_prefix(", ") {
                // Taken before the advance: the fault below is the separator, not
                // whatever follows it.
                let separator_at = at(rest);
                rest = next;
                comma_packed = true;
                // Re-check the elements already seen now that we know the array is
                // comma packed -- the first element is parsed before any separator.
                if saw_bare && saw_non_bare {
                    return Err(self.mixed_pack_error(col, bare_scalar.as_deref()));
                }
                if bare_holds_comma {
                    return Err(self.bare_comma_error(col));
                }
                // Spaces after a separator are still the line's trailing spaces.
                // Discarding them leaves the separator with nothing after it,
                // which is the next thing wrong with the line rather than the
                // same thing said twice.
                if self.trailing_spaces_end_the_line(rest, at(rest))? {
                    rest = "";
                }
                if rest.is_empty() {
                    return Err(
                        self.error_at_col(separator_at, "array lines cannot end with a separator")
                    );
                }
                continue;
            }
            if let Some(next) = rest.strip_prefix("  ") {
                let separator_at = at(rest);
                rest = next;
                space_packed = true;
                // Re-check the elements already seen now that we know the line is
                // space packed -- the first element is parsed before any separator.
                if saw_non_bare {
                    return Err(self.error_at_col(
                        col,
                        "a space separated packed array requires every element to be a bare \
                         string; consider unpacking this line onto multiple lines",
                    ));
                }
                if self.trailing_spaces_end_the_line(rest, at(rest))? {
                    rest = "";
                }
                if rest.is_empty() {
                    return Err(
                        self.error_at_col(separator_at, "array lines cannot end with a separator")
                    );
                }
                continue;
            }
            // `rest` starts at the text that is not a separator, which is the
            // offender itself -- most often one space where two belong.
            return Err(self.error_at_col(
                at(rest),
                "array elements on the same line are separated by ', ' or by two spaces in \
                 all-bare-string arrays",
            ));
        }
    }

    fn parse_marker_chain_line(
        &mut self,
        content: &str,
        line_indent: LogicalIndent,
    ) -> std::result::Result<T, ParseError> {
        // Every container introduced by this marker line carries the marker line's span.
        let open_span = self.current_span();
        // Comments preceding a marker line attach to the container it introduces.
        //
        // DESIGN INTENT, NOT YET IMPLEMENTED. What follows is the rule decided on
        // 2026-08-09; the code below does not do it. Written in the future tense on
        // purpose, because a comment describing behaviour the code lacks is how
        // `render.rs`'s `unreachable!` came to be trusted and never checked (see
        // P2 in `local/fuzzer-found-breakage.md`). Today this function attaches to
        // the OUTERMOST container of the chain, and the renderer then drops the
        // comment entirely on every path but one -- so a comment above a marker
        // line is silently lost (C1, same file).
        //
        // The rule: a comment annotates the thing it ALIGNS WITH on the line
        // immediately below it. This is the format's own doctrine -- location is
        // depth -- applied to comments, and it is what makes a packed marker chain
        // unambiguous. A chain lays out one level every two columns:
        //
        //       k:
        //     //comment          <- column 2, annotates the outer array
        //       [ [ [ [ { b:3
        //         ^ ^ ^ ^ ^ ^
        //         2 4 6 8 | 12   <- 2,4,6,8 arrays; 10 object; 12 the key `b`
        //                 10
        //
        // A marker occupies two columns, `[` and the space after it, and a comment
        // starting at either one unambiguously names that container. Both are
        // legal; the canonical form is the first, with the leading `/` directly
        // above the `[`. A generator normalizes to that.
        //
        // In a table the same rule reads off the row instead of the indent: the
        // leading `|` is the row object, and the first character after any `|` is
        // that cell's key (header line) or value (data line). Normalization for a
        // cell is to the character immediately after the `|`, *including* when
        // that character is the leading space or `_` of a bare string -- the
        // opening quote is part of the string, so the string starts there and not
        // at its first letter.
        //
        // Note the asymmetry: the `|` column is structural and identical in every
        // conforming rendering, while a cell's column depends on padding, which is
        // the generator's choice (see the specification's column-width section).
        // So the attachment is a fact about the tree and the column is re-derived
        // at render time; it is never stored as a column.
        let pending = self.take_pending_comments();
        // `line_indent` is logical; spans need the raw byte column of `content`'s start.
        let base_col = self
            .current_line()
            .and_then(|line| self.byte_offset_of(line, line_indent))
            .unwrap_or(ByteOffset::START);
        let mut rest = content;
        let mut markers = Vec::new();
        // Which levels the writer typed and which this read in. An explicit
        // marker is a fact about the document and is never revised; an inferred
        // one is a level the indentation already established, whose only
        // missing part is the glyph that would have told a reader about it.
        let mut inferred = Vec::new();
        loop {
            if let Some(next) = Marker::Array.strip(rest) {
                markers.push(ContainerKind::Array);
                inferred.push(false);
                rest = next;
                continue;
            }
            if let Some(next) = Marker::Object.strip(rest) {
                markers.push(ContainerKind::Object);
                inferred.push(false);
                rest = next;
                break;
            }
            // Two spaces standing where `[ ` or `{ ` was expected. The document
            // is broken either way: the specification requires the chain be
            // written once a value moves more than one level, and it is not.
            // What the option decides is only whether that error is reported or
            // stepped over -- `Infer` is a suppression for testing, not a
            // reading under which this document becomes legal.
            //
            // Stepping over it can produce the right tree because the depth was
            // never carried by the glyphs in the first place; the column holds
            // it, and holds it whether or not anyone wrote a marker. That is
            // what makes the suppression useful for exercising the parser
            // rather than merely permissive.
            //
            // Provisionally an array; the fixup below is the only place a kind
            // is revised, and it can only reach the last one.
            if rest.starts_with("  ") {
                if self.options.missing_indent_marker != MissingIndentMarker::Infer {
                    return Err(self.missing_marker_error(
                        base_col.plus(content.len() - rest.len()),
                        markers.len(),
                    ));
                }
                markers.push(ContainerKind::Array);
                inferred.push(true);
                rest = &rest[2..];
                continue;
            }
            break;
        }
        if markers.is_empty() {
            return Err(self.error_current("expected an explicit nesting marker"));
        }
        // A marker chain nests without recursing: the levels are built by the loops
        // below, so they never pass through the tails that count depth. Charged
        // here in one go instead, which is possible precisely because the whole
        // chain's depth is known before any of it is built.
        //
        // Without this a chain was the one way past the limit, and the crash it
        // led to was not even in the parser -- the tree it built was deep enough
        // that walking it to drop or serialize it overflowed the stack later, far
        // from the line responsible.
        if self.depth + markers.len() > MAX_DEPTH {
            return Err(self.too_deep());
        }
        // Only the deepest level can be an object, and an inferred one has no
        // glyph saying which it is, so it answers the same question an ordinary
        // one-level nesting answers: a key and a colon make it an object, and
        // anything else leaves it the array it was assumed to be.
        if *inferred.last().unwrap()
            && self.looks_like_object_start(rest, line_indent.deeper(markers.len()))?
        {
            *markers.last_mut().unwrap() = ContainerKind::Object;
        }
        // Non-empty from here down: the two `unwrap`s above establish it, and the
        // `len() - 1` slices below rely on it. Stated once so the two readings
        // agree -- previously this line defended against an empty vector three
        // lines after two others insisted it could not be.
        debug_assert!(!markers.is_empty(), "a marker chain line has at least one marker");
        if markers[..markers.len() - 1]
            .iter()
            .any(|kind| *kind != ContainerKind::Array)
        {
            return Err(
                self.error_current("only the final explicit nesting marker on a line may be '{'")
            );
        }
        let deepest_parent_indent = line_indent.deeper(markers.len().saturating_sub(1));

        // Indent glyph after markers: `[ [ /<` — content follows on next lines at shifted indent.
        if rest == Glyph::IndentOpen.text() {
            let glyph = self.idt.to_file(deepest_parent_indent.deeper(1));
            self.idt.push_glyph(glyph);
            self.line += 1;
            // The deepest container's content starts on the next lines.
            let mut value = match *markers.last().unwrap() {
                ContainerKind::Array => {
                    let mut elements = Vec::new();
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                    if elements.is_empty() {
                        return Err(self.error_current("expected at least one array element after indent glyph"));
                    }
                    T::new_array(elements, self.container_facts_from(open_span))
                }
                ContainerKind::Object => {
                    let pair_indent = deepest_parent_indent.deeper(1);
                    let mut entries = Vec::new();
                    self.parse_object_tail(pair_indent, &mut entries)?;
                    if entries.is_empty() {
                        return Err(self.error_current("expected at least one object entry after indent glyph"));
                    }
                    T::new_object(entries, self.container_facts_from(open_span))
                }
            };
            for level in (0..markers.len().saturating_sub(1)).rev() {
                let parent_indent = line_indent.deeper(level);
                let mut wrapped = vec![value];
                self.parse_array_tail(parent_indent, &mut wrapped)?;
                value = T::new_array(wrapped, self.container_facts_from(open_span));
            }
            if T::KEEPS_COMMENTS && !pending.is_empty() {
                T::attach_comments_before(&mut value, pending, line_indent);
            }
            return Ok(value);
        }

        if rest.is_empty() {
            return Err(self.error_current("a nesting marker must be followed by content"));
        }

        // Special case: the last `[` marker followed immediately by a table header means
        // the last `[` IS the table array itself, not a wrapper around it.
        if *markers.last().unwrap() == ContainerKind::Array {
            let rest_trimmed = rest.trim_start_matches(' ');
            if rest_trimmed.starts_with('|') {
                let leading_spaces = rest.len() - rest_trimmed.len();
                // The marker loop above consumes whole levels, so at most one
                // space can survive to here, and one space means a bare
                // string's opening quote. A table row cannot be one -- a bare
                // string may not begin with a pipe -- so that space explains
                // nothing and the header is simply misaligned. This used to
                // slide the table right to wherever the pipe happened to sit,
                // which let a container land on a depth nobody wrote.
                if leading_spaces != 0 {
                    return Err(self.error_at_line(
                        self.line,
                        base_col.plus(content.len() - rest.len()),
                        "a table header is one space right of the level it belongs to. A \
                         single space before a value opens a bare string, and a bare string \
                         cannot begin with a pipe, so nothing here explains the space -- \
                         delete it to put the header at its level, or add one more to put \
                         the table a level deeper",
                    ));
                }
                // `leading_spaces` is zero here and cannot be anything else: the guard
                // above returns on any non-zero count. Adding it made this look like a
                // position that could land off a level, which is the one thing a
                // structural indent may never do.
                let table_elem_indent = deepest_parent_indent.deeper(1);
                let mut value = self.parse_table_array(table_elem_indent)?;
                for level in (0..markers.len().saturating_sub(1)).rev() {
                    let parent_indent = line_indent.deeper(level);
                    let mut wrapped = vec![value];
                    self.parse_array_tail(parent_indent, &mut wrapped)?;
                    value = T::new_array(wrapped, self.container_facts_from(open_span));
                }
                if T::KEEPS_COMMENTS && !pending.is_empty() {
                    T::attach_comments_before(&mut value, pending, line_indent);
                }
                return Ok(value);
            }
        }

        let rest_col = base_col.plus(content.len() - rest.len());
        let mut value = match *markers.last().unwrap() {
            ContainerKind::Array => {
                let mut elements = Vec::new();
                if is_minimal_json_candidate(rest) {
                    let span = self.span_at(Some(rest_col), rest.len());
                    elements.push(self.parse_minimal_json_line(rest, span)?);
                    self.line += 1;
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                } else {
                    self.parse_array_line_content(
                        rest,
                        deepest_parent_indent.deeper(1),
                        &mut elements,
                        Some(rest_col),
                    )?;
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                }
                T::new_array(elements, self.container_facts_from(open_span))
            }
            ContainerKind::Object => {
                let pair_indent = line_indent.deeper(markers.len());
                let mut entries =
                    self.parse_object_line_content(rest, pair_indent, Some(rest_col))?;
                self.parse_object_tail(pair_indent, &mut entries)?;
                T::new_object(entries, self.container_facts_from(open_span))
            }
        };
        for level in (0..markers.len().saturating_sub(1)).rev() {
            let parent_indent = line_indent.deeper(level);
            let mut wrapped = vec![value];
            self.parse_array_tail(parent_indent, &mut wrapped)?;
            value = T::new_array(wrapped, self.container_facts_from(open_span));
        }
        if T::KEEPS_COMMENTS && !pending.is_empty() {
            T::attach_comments_before(&mut value, pending, line_indent);
        }
        Ok(value)
    }

    /// Parse an object key, returning `(key_string, key_form, rest_after_colon)`.
    /// Handles fold continuations (`/ `) for both bare keys and JSON string keys.
    // TODO: this String KeyForm String tuple should probably be some sort of kvpair type struct - consider more throughly before changing.  I think we have one already elsewhere but it might not be suitable.
    fn parse_key(
        &mut self,
        content: &str,
        fold_indent: LogicalIndent,
    ) -> std::result::Result<(String, KeyForm, String), ParseError> {
        // Bare key on this line
        if let Some(end) = parse_bare_key_prefix(content, &self.options) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((
                    content[..end].to_owned(),
                    KeyForm::Bare,
                    content[end + ':'.len_utf8()..].to_owned(),
                ));
            }
            // Bare key fills the whole line — look for fold continuations
            if only_held_back_tail(content, end, &self.options) {
                let mut key_acc = content.to_owned();
                let mut next = self.line + 1;
                loop {
                    let (colon_pos, cont_owned) = match self.classify_fold_next(next, fold_indent) {
                        FoldNext::Continues(cont) => (cont.find(':'), cont.to_owned()),
                        // Spec: "A comment may not be within a fold." Caught
                        // here rather than left to break the loop, because a
                        // key that stops collecting mid-fold goes on to fail
                        // as something else entirely and names the wrong line.
                        FoldNext::Comment => {
                            self.comment_in_fold(
                                next,
                                "this comment sits in the middle of a key that \
                                 continues below it -- move the comment above the \
                                 whole key, where it still comments the same thing",
                            )?;
                            next += 1;
                            continue;
                        }
                        FoldNext::Ends => break,
                        // Not merged with `Ends`: a marker at another column is
                        // the opposite fact from the value having finished, and
                        // merging them is what sent this line 2700 lines on to
                        // be guessed at from its text. Nothing shallower can be
                        // open here -- folding continues a scalar, and a scalar
                        // has no children -- so any other column is a mistake,
                        // and `stray_fold_marker` names the one it belonged at.
                        FoldNext::ContinuesElsewhere => {
                            return Err(self.stray_fold_marker(next, fold_indent));
                        }
                    };
                    next += 1;
                    if let Some(colon_pos) = colon_pos {
                        key_acc.push_str(&cont_owned[..colon_pos]);
                        self.line = next - 1; // point to last fold line; caller will +1
                        return Ok((
                            key_acc,
                            KeyForm::Bare,
                            cont_owned[colon_pos + ':'.len_utf8()..].to_owned(),
                        ));
                    }
                    key_acc.push_str(&cont_owned);
                }
            }
        }
        // JSON string key on this line
        if let Some((value, end)) = parse_json_string_prefix(content)
            && content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((value, KeyForm::Quoted, content[end + ':'.len_utf8()..].to_owned()));
            }
        // JSON string key that doesn't close on this line — look for fold continuations
        if content.starts_with('"') && parse_json_string_prefix(content).is_none() {
            let mut json_acc = content.to_owned();
            let mut next = self.line + 1;
            loop {
                let rest = match self.classify_fold_next(next, fold_indent) {
                    FoldNext::Continues(rest) => rest,
                    // Spec: "A comment may not be within a fold."
                    FoldNext::Comment => {
                        self.comment_in_fold(
                            next,
                            "this comment sits in the middle of a quoted key that \
                             continues below it -- move the comment above the whole \
                             key, where it still comments the same thing",
                        )?;
                        next += 1;
                        continue;
                    }
                    FoldNext::Ends => break,
                        // Not merged with `Ends`: a marker at another column is
                        // the opposite fact from the value having finished, and
                        // merging them is what sent this line 2700 lines on to
                        // be guessed at from its text. Nothing shallower can be
                        // open here -- folding continues a scalar, and a scalar
                        // has no children -- so any other column is a mistake,
                        // and `stray_fold_marker` names the one it belonged at.
                        FoldNext::ContinuesElsewhere => {
                            return Err(self.stray_fold_marker(next, fold_indent));
                        }
                };
                json_acc.push_str(rest);
                next += 1;
                if let Some((value, end)) = parse_json_string_prefix(&json_acc)
                    && json_acc.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                        self.line = next - 1; // point to last fold line; caller will +1
                        return Ok((
                            value,
                            KeyForm::Quoted,
                            json_acc[end + ':'.len_utf8()..].to_owned(),
                        ));
                    }
            }
        }
        // A key follows the bare string rules, so the low line is barred at its
        // start too -- and for a sharper reason than the other exclusions. That
        // column is where a bare string's opening marker goes, so a key opening
        // with one is not merely irregular, it is a line that already reads as
        // something else.
        if self.options.is_underscore_like(content.chars().next().unwrap_or(' ')) {
            return Err(self.error_at_indent(
                self.line,
                fold_indent,
                "a bare key cannot begin with `_` or a character shaped like one. Keys \
                 follow the bare string rules, and this column is where a bare string's \
                 opening marker goes -- so a key starting here would be read as a marked \
                 string rather than a key. Double quote it.",
            ));
        }
        // A marker standing where a key belongs is worth naming, because the
        // writer meant something by it and what they meant cannot be true. A
        // marker says a container starts at its column. This column is where
        // this object's keys go, and a container cannot be an entry without a
        // key to hold it -- so nothing starts here, and the marker asserts
        // otherwise. The usual case is a continuation: the array opened after
        // the key above and the writer carried it down, but a marker cannot
        // continue a container, only begin one.
        if starts_with_marker_chain(content) {
            return Err(self.error_at_indent(
                self.line,
                fold_indent,
                "a nesting marker here says a container starts at this column, but this is \
                 where this object's keys go and a container cannot be an entry without a \
                 key. If this is meant to continue a value from the line above, a marker \
                 cannot do that -- it can only start something new; indent the continuation \
                 instead. If it is meant to be a new entry, give it a key.",
            ));
        }
        Err(self.error_at_indent(self.line, fold_indent, "invalid object key"))
    }

    fn parse_inline_value(
        &mut self,
        content: &str,
        line_indent: LogicalIndent,
        context: ArrayLineValueContext,
        col: Option<ByteOffset>,
    ) -> std::result::Result<(T, Option<usize>), ParseError> {
        let first = content
            .chars()
            .next()
            .ok_or_else(|| self.error_current("expected a value"))?;
        match first {
            ' ' | '_' => {
                // Both openers are one byte and occupy the same column, so every
                // width and offset below is unchanged by which one was written.
                // Only the branches that test for a literal space -- the packed
                // array and the multiline opener -- distinguish them, and those
                // are space-only constructs by definition.
                let bare_form =
                    if first == '_' { BareForm::Marked } else { BareForm::Plain };
                // `[]` and `{}` after a leading space are the *strings* "[]" and "{}",
                // not empty containers. The space is the bare string's one-sided
                // opening quote and it means here exactly what it means everywhere
                // else -- `k:true` is the boolean and `k: true` is the string, so
                // `k:[]` is the empty array and `k: []` is the string. The spec lists
                // "[]" and "{}" among the values allowed as bare strings, so they get
                // no special case: fall through to the bare string path below.
                if context == ArrayLineValueContext::ObjectValue
                    && let Some(rest) = content.strip_prefix(ARRAY_STARTER)
                {
                    let consumed = content.len() - rest.len();
                    let value = self.parse_inline_array(rest, line_indent, col.map(|c| c.plus(consumed)))?;
                    return Ok((value, None));
                }
                if let Some((opener_len, body_len)) =
                    Glyph::MultilineSingle.split_opener(content)
                {
                    // Opener facts are captured before the body parse moves past
                    // it. Both numbers come from the split, so neither site says
                    // how wide the opener is.
                    let opener_span = self.span_at(col.map(|c| c.plus(opener_len)), body_len);
                    let (value, flavor) = self.parse_multiline_string(content, line_indent)?;
                    let facts = StringFacts { form: StringForm::Multiline(flavor), span: opener_span };
                    return Ok((T::new_string(value, facts), None));
                }
                let end = bare_string_end(content, context);
                if end == 0 {
                    // Two different faults land here. A run of spaces where a value
                    // should start is not a forbidden character, so saying so sends
                    // the reader looking at the wrong thing entirely.
                    //
                    // Every space up to the third opens something; the fault is that
                    // this one opens nothing. Spelling the ladder out is the whole
                    // message, because a reader who counted wrong cannot tell which
                    // rung they meant from a complaint that there is "too much
                    // space", and the three rungs do not imply one another.
                    if content.starts_with("  ") {
                        return Err(self.error_at_col(
                            col,
                            "too many spaces before this value. One space starts an \
                             ordinary value (`k: x` is the string x), two open a packed \
                             array (`k:  1, 2`), and a third opens a bare string inside \
                             that array (`k:   x` is an array holding the string x). \
                             Nothing is left for a fourth to open -- delete the extra \
                             spaces, or put the value on its own line below the key.",
                        ));
                    }
                    return Err(self.error_at_col(
                        col,
                        "bare strings cannot start with a forbidden character",
                    ));
                }
                let value = &content[first.len_utf8()..end]; // the opener, space or `_`
                // A fold is transparent: the value continues on the next line, so the
                // character rules describe the reassembled value, not the segment
                // sitting on this line. A segment may therefore end on a comma -- the
                // comma is interior to the finished string, and only looks final
                // because the fold split the line there.
                //
                // The walk holds back nothing but a single space, so anything past
                // `end` that is not a space is another value and rules a fold out.
                let tail = &content[end..];
                let ends_line = tail.chars().all(|c| c == ' ');
                let mut folded: Option<(String, usize)> = None;
                if ends_line {
                    let mut acc = content[first.len_utf8()..].to_owned();
                    let mut next = self.line + 1;
                    let mut fold_count = 0usize;
                    loop {
                        // Asked rather than matched inline so a comment reaches
                        // the policy instead of quietly ending the fold and
                        // leaving the `/ ` line below to fail as a stray key.
                        let continuation = match self.classify_fold_next(next, line_indent) {
                            FoldNext::Continues(rest) => Some(rest.to_owned()),
                            FoldNext::Comment => None,
                            FoldNext::Ends => break,
                        // Not merged with `Ends`: a marker at another column is
                        // the opposite fact from the value having finished, and
                        // merging them is what sent this line 2700 lines on to
                        // be guessed at from its text. Nothing shallower can be
                        // open here -- folding continues a scalar, and a scalar
                        // has no children -- so any other column is a mistake,
                        // and `stray_fold_marker` names the one it belonged at.
                        FoldNext::ContinuesElsewhere => {
                            return Err(self.stray_fold_marker(next, line_indent));
                        }
                        };
                        let Some(rest) = continuation else {
                            self.comment_in_fold(
                                next,
                                "this comment sits in the middle of a value that \
                                 continues below it -- move the comment above the \
                                 whole value, or below it",
                            )?;
                            next += 1;
                            continue;
                        };
                        acc.push_str(&rest);
                        next += 1;
                        fold_count += 1;
                    }
                    if fold_count > 0 {
                        folded = Some((acc, next));
                    }
                }
                let complete = folded.as_ref().map_or(value, |(acc, _)| acc.as_str());
                if let Err(fault) = check_bare_string(complete, &self.options) {
                    // `describe` speaks for the string on its own, and for a leading
                    // `"` it says to delete the opening space -- correct everywhere
                    // except here, where doing so leaves a quoted element on a line
                    // that admits none, and trades this error for that one. The
                    // element cannot be spelled on this line at all, so say that
                    // instead of handing over a fix that fails.
                    if let (ArrayLineValueContext::ArrayLine(Packing::Space),
                            BareStringFault::LeadingDoubleQuote) = (context, fault)
                    {
                        return Err(self.error_at_col(
                            col,
                            "a space separated packed array holds bare strings only, so a \
                             double quoted element has no spelling on this line -- give \
                             the array one element per line, or write this element bare",
                        ));
                    }
                    return Err(self.error_at_col(col, fault.describe()));
                }
                if let Some((acc, next)) = folded {
                    // Facts before the line advance so the span lands on the opener line.
                    let facts = self.string_facts_at(
                        StringForm::Bare(bare_form),
                        col.map(|c| c.plus(Opener::BareString.width())),
                        end.saturating_sub(Opener::BareString.width()),
                    );
                    self.line = next;
                    return Ok((T::new_string(acc, facts), None));
                }
                Ok((
                    T::new_string(
                        value.to_owned(),
                        self.string_facts_at(
                            StringForm::Bare(bare_form),
                            col.map(|c| c.plus(Opener::BareString.width())),
                            end.saturating_sub(Opener::BareString.width()),
                        ),
                    ),
                    Some(end),
                ))
            }
            '"' => {
                if let Some((value, end)) = parse_json_string_prefix(content) {
                    return Ok((
                        T::new_string(value, self.string_facts_at(StringForm::Quoted, col, end)),
                        Some(end),
                    ));
                }
                // Facts before the fold consumption moves past the opening line.
                let facts = self.string_facts_at(StringForm::Quoted, col, content.len());
                let value = self.parse_folded_json_string(content, line_indent)?;
                Ok((T::new_string(value, facts), None))
            }
            '[' => {
                if content.starts_with("[]") {
                    let facts = ContainerFacts { span: self.span_at(col, 2), table: false };
                    return Ok((T::new_array(Vec::new(), facts), Some(2)));
                }
                // Spec, array starters 2 and 3, inline start variant: `[ ` may
                // stand where the `  ` that opens a packed array on the key's
                // line would go, "if the writer wants to be particularly
                // explicit". It occupies the same two columns and means the
                // same thing, which is why one strip serves both starters --
                // the comma-packed and the space-packed forms differ only in
                // what follows, and that is the inline array parser's problem.
                //
                // No fact is recorded about which spelling was used, so a
                // document written this way renders back as `  `. That is a
                // choice about effort rather than a rule: echoing a spelling a
                // person actually typed would be a fine thing to do, it is just
                // not worth the machinery to carry. Nothing should be built on
                // the normalization -- it is free to change.
                if context == ArrayLineValueContext::ObjectValue
                    && let Some(rest) = Marker::Array.strip(content)
                {
                    let consumed = content.len() - rest.len();
                    let value = self.parse_inline_array(rest, line_indent, col.map(|c| c.plus(consumed)))?;
                    return Ok((value, None));
                }
                if is_minimal_json_candidate(content) {
                    // Spec: MINIMAL JSON "MUST NEVER be packed in a TJSON line with
                    // any other value", with one exception -- "a non folded bare or
                    // quoted key immediately before [it] on its same line". So it is
                    // allowed as an object value or alone, never as an element of a
                    // packed array.
                    if matches!(context, ArrayLineValueContext::ArrayLine(_)) {
                        return Err(self.error_current(
                            "MINIMAL JSON may not be packed with other values; \
                             it must be alone on its line, or follow a key",
                        ));
                    }
                    let Some(end) = minimal_json_end(content) else {
                        // Spec: "MINIMAL JSON cannot be wrapped or folded." Failing to
                        // close on this line is the only way to reach here, so say so
                        // rather than leaving serde to report an EOF.
                        return Err(self.error_current(
                            "MINIMAL JSON must open and close on the same line; \
                             it cannot be wrapped or folded",
                        ));
                    };
                    let span = self.span_at(col, end);
                    let value = self.parse_minimal_json_line(&content[..end], span)?;
                    return Ok((value, Some(end)));
                }
                Err(self.error_current("nonempty arrays require container context"))
            }
            '{' => {
                if content.starts_with("{}") {
                    let facts = ContainerFacts { span: self.span_at(col, 2), table: false };
                    return Ok((T::new_object(Vec::new(), facts), Some(2)));
                }
                if is_minimal_json_candidate(content) {
                    // Spec: MINIMAL JSON "MUST NEVER be packed in a TJSON line with
                    // any other value", with one exception -- "a non folded bare or
                    // quoted key immediately before [it] on its same line". So it is
                    // allowed as an object value or alone, never as an element of a
                    // packed array.
                    if matches!(context, ArrayLineValueContext::ArrayLine(_)) {
                        return Err(self.error_current(
                            "MINIMAL JSON may not be packed with other values; \
                             it must be alone on its line, or follow a key",
                        ));
                    }
                    let Some(end) = minimal_json_end(content) else {
                        // Spec: "MINIMAL JSON cannot be wrapped or folded." Failing to
                        // close on this line is the only way to reach here, so say so
                        // rather than leaving serde to report an EOF.
                        return Err(self.error_current(
                            "MINIMAL JSON must open and close on the same line; \
                             it cannot be wrapped or folded",
                        ));
                    };
                    let span = self.span_at(col, end);
                    let value = self.parse_minimal_json_line(&content[..end], span)?;
                    return Ok((value, Some(end)));
                }
                Err(self.error_current("nonempty objects require object or array context"))
            }
            't' if content.starts_with("true") => {
                self.check_literal_boundary(content, "true", col, context)?;
                Ok((T::new_bool(true, self.scalar_facts_at(col, 4)), Some(4)))
            }
            'f' if content.starts_with("false") => {
                self.check_literal_boundary(content, "false", col, context)?;
                Ok((T::new_bool(false, self.scalar_facts_at(col, 5)), Some(5)))
            }
            'n' if content.starts_with("null") => {
                self.check_literal_boundary(content, "null", col, context)?;
                Ok((T::new_null(self.scalar_facts_at(col, 4)), Some(4)))
            }
            '-' | '0'..='9' => {
                let end = token_end(content, context);
                let token = &content[..end];
                // Check for fold continuations when the number fills the rest of the line
                if end == content.len() {
                    let mut acc = token.to_owned();
                    let mut next = self.line + 1;
                    let mut fold_count = 0usize;
                    loop {
                        // Asked rather than matched inline so a comment reaches
                        // the policy instead of quietly ending the fold and
                        // leaving the `/ ` line below to fail as a stray key.
                        let continuation = match self.classify_fold_next(next, line_indent) {
                            FoldNext::Continues(rest) => Some(rest.to_owned()),
                            FoldNext::Comment => None,
                            FoldNext::Ends => break,
                        // Not merged with `Ends`: a marker at another column is
                        // the opposite fact from the value having finished, and
                        // merging them is what sent this line 2700 lines on to
                        // be guessed at from its text. Nothing shallower can be
                        // open here -- folding continues a scalar, and a scalar
                        // has no children -- so any other column is a mistake,
                        // and `stray_fold_marker` names the one it belonged at.
                        FoldNext::ContinuesElsewhere => {
                            return Err(self.stray_fold_marker(next, line_indent));
                        }
                        };
                        let Some(rest) = continuation else {
                            self.comment_in_fold(
                                next,
                                "this comment sits in the middle of a value that \
                                 continues below it -- move the comment above the \
                                 whole value, or below it",
                            )?;
                            next += 1;
                            continue;
                        };
                        acc.push_str(&rest);
                        next += 1;
                        fold_count += 1;
                    }
                    if fold_count > 0 {
                        let n = acc.parse::<Number>()
                            .map_err(|_| self.error_current(format!("invalid JSON number after folding: \"{acc}\"")))?;
                        // Facts before the line advance so the span lands on the opener line.
                        let facts = self.scalar_facts_at(col, end);
                        self.line = next;
                        return Ok((T::new_number(n, facts), None));
                    }
                }
                let n = token.parse::<Number>()
                    .map_err(|_| self.error_at_col(col, format!("invalid JSON number: \"{token}\"")))?;
                Ok((T::new_number(n, self.scalar_facts_at(col, end)), Some(end)))
            }
            '.' if content[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                let end = token_end(content, context);
                let token = &content[..end];
                Err(self.error_at_col(col, format!("invalid JSON number: \"{token}\" (numbers must start with a digit)")))
            }
            _ => {
                // Nothing here starts a value. If a colon appears later on the
                // line the writer probably meant a key, so report what is wrong
                // with the key rather than a generic value fault that points at
                // the wrong construct and names no rule.
                if let Err(fault) = check_attempted_bare_key(content, &self.options) {
                    return Err(self.error_at_col(col, fault.describe()));
                }
                // The text would be a perfectly good bare string if a space came
                // first, so the fault is a miscount of the opening spaces rather
                // than anything wrong with the characters. That is worth saying,
                // because a bare string is the one value whose opening quote is a
                // space -- which is exactly the part nobody guesses -- and because
                // "invalid value start" points at the text, where nothing is
                // wrong, instead of at the gap before it.
                // A `/ ` line is a fold continuation and nothing else, so meeting
                // one where a value should start means the line above it did not
                // leave a value open. "invalid value start" describes the text,
                // which is fine; the fault is the missing thing above it.
                // Before both the no-colon reading and the spacing ladder, because
                // it is the more specific fault and the only one of the three that
                // mentions the case rule the writer actually broke.
                let token = &content[..token_end(content, context)];
                if let Some(literal) = miscased_literal(token) {
                    return Err(self.error_at_col(
                        col,
                        format!(
                            "`{token}` is not a TJSON literal -- the literals are \
                             lowercase, so write `{literal}`. Putting a space in front \
                             of it instead is valid TJSON, but it writes the string \
                             \"{token}\" rather than the {}.",
                            if literal == "null" { "null value" } else { "boolean" }
                        ),
                    ));
                }
                if content.starts_with(Marker::Fold.text()) || content == "/" {
                    return Err(self.error_at_col(
                        col,
                        "a `/ ` line continues a value from the line above it, and \
                         nothing above this line is left open. Remove the `/ `, or \
                         put the value it was meant to continue on the line above.",
                    ));
                }
                // No colon anywhere on the line: an entry needs one, and that is a
                // likelier reading than anything about spaces. Both ways out are
                // worth naming, because which was meant is not knowable here.
                // The whole physical line, not this value fragment: a fragment
                // after a colon naturally has none of its own, and reading that
                // as "no colon on this line" hijacks every value fault.
                if self.current_line().is_some_and(|line| !line.text().contains(':'))
                    && content.starts_with(|c: char| is_unicode_letter_or_number(c))
                {
                    return Err(self.error_at_col(
                        col,
                        "there is no colon on this line, so it is not a key and a \
                         value. An object entry is written `key: value`. If the whole \
                         line was meant as a string, it needs a space in front of it \
                         -- the space is what opens a bare string.",
                    ));
                }
                // `check_bare_string` works out exactly which rule the text breaks
                // and has a written message for each. Asking it `is_ok()` and then
                // falling through to "invalid value start" threw that away and named
                // nothing -- while the bare-string arm of this same match already
                // reports the fault. It matters most for the lookalike sets: an
                // ASCII `[` reaches its own arm above, but a character merely shaped
                // like one arrives here, and "invalid value start" is the least
                // useful thing to say to someone who cannot see the difference.
                let Err(fault) = check_bare_string(content, &self.options) else {
                    let ladder = match context {
                        // Deliberately not "add a space": inside a *comma* packed
                        // array that produces a bare string among non-bare
                        // elements, which the all-or-none rule then rejects. The
                        // advice would trade this error for another one, and a
                        // suggestion that does not work is worse than none.
                        ArrayLineValueContext::ArrayLine(Packing::Comma) =>
                            "A comma packed array cannot hold a bare string at all: the \
                             space that opens one makes the comma after it part of the \
                             string rather than a separator, so there is no spacing that \
                             works here. Double quote this element, or put the array on \
                             multiple lines",
                        // The opposite line, and the opposite advice: here a bare
                        // string is the only thing allowed, so adding the opening
                        // space is the fix and it works. This used to get the
                        // sentence above, which explained commas to a writer whose
                        // line had none.
                        ArrayLineValueContext::ArrayLine(Packing::Space) =>
                            "In a space separated packed array every element carries its \
                             own opening space, on top of the two that separate it from \
                             the element before -- so this one wants three spaces in \
                             front of it rather than two",
                        // The first element on the line, parsed before any separator
                        // has said how the line is packed. The general rule is the
                        // honest thing to give, since either packing is still open.
                        _ =>
                            "Counting from the colon: one space writes the string \
                             (`k: x`), two open a packed array (`k:  1, 2`), and \
                             three put a bare string inside that array (`k:   x`)",
                    };
                    return Err(self.error_at_col(
                        col,
                        format!(
                            "a bare string opens with a space -- the space is its \
                             opening quote, and there is none here. {ladder}"
                        ),
                    ));
                };
                Err(self.error_at_col(col, fault.describe()))
            }
        }
    }

    fn parse_inline_array(
        &mut self,
        content: &str,
        parent_indent: LogicalIndent,
        col0: Option<ByteOffset>,
    ) -> std::result::Result<T, ParseError> {
        let open_span = self.span_at(col0, content.len());
        let mut values = Vec::new();
        self.parse_array_line_content(content, parent_indent.deeper(1), &mut values, col0)?;
        self.parse_array_tail(parent_indent, &mut values)?;
        Ok(T::new_array(values, self.container_facts_from(open_span)))
    }

    /// Is there enough in this fence to be a MULTILINE STRING?
    ///
    /// All three body parsers ask, because the rule is about the string they
    /// produced and not about how it was written. It reads the assembled value
    /// rather than counting lines: under `Character` what is forbidden is an
    /// empty string, and a transparent fence around a single zero-length line
    /// produces one while still having a line in it.
    ///
    /// The empty case is a parse error under every reading. The no-linefeed
    /// case is legal TJSON that only [`MultilineMinimum::Eol`] refuses, so its
    /// message says who is refusing it rather than claiming the format does.
    fn check_multiline_minimum(
        &self,
        value: &str,
        glyph: &str,
        body_lines: usize,
    ) -> std::result::Result<(), ParseError> {
        // The reported line is the closing glyph, which is where the reader can
        // see that nothing came before it.
        let closer = self.line - 1;
        if value.is_empty() {
            // Only two shapes reach here: no body line at all, and exactly one
            // that is blank. Two blank ones already join into "\n", which is a
            // data character and not empty. They are different mistakes, so
            // each is told what it did and nothing about the other -- the
            // blank-line advice is the answer to a question only the second
            // one asked.
            let (cause, alternative) = if body_lines == 0 {
                ("there is nothing at all between the opening and closing glyph", "")
            } else {
                (
                    "its one body line is blank, and a blank line holds no data",
                    " Two blank body lines would instead make a string holding a single \
                     EOL, which is valid.",
                )
            };
            return Err(self.error_at_line(
                closer,
                ByteOffset::START,
                format!(
                    "A multiline must contain at least one data character, and this one \
                     does not -- {cause}.  An empty multiline string is not allowed: \
                     TJSON gives each empty value exactly one representation -- \"\" for \
                     the empty string, [] and {{}} for the empty array and object. If you \
                     want an empty string, delete the whole {glyph}...{glyph} block and \
                     write \"\" in its place.{alternative}"
                ),
            ));
        }
        // What this asks is whether the data holds a data EOL. A data EOL is
        // `MultilineLocalEol::as_str` -- `"\n"` under one LOCAL EOL INDICATOR
        // and `"\r\n"` under the other -- and the linefeed is the part both
        // forms are built on, so finding a linefeed is finding a data EOL.
        if self.options.multiline_minimum == MultilineMinimum::Eol && !value.contains('\n') {
            return Err(self.error_at_line(
                closer,
                ByteOffset::START,
                format!(
                    "this multiline string holds no real linefeed, which the strict \
                     reading refuses; write the value as an ordinary string, or read it \
                     with the default multiline minimum to keep the {glyph}...{glyph} block"
                ),
            ));
        }
        Ok(())
    }

    fn parse_multiline_string(
        &mut self,
        content: &str,
        line_indent: LogicalIndent,
    ) -> std::result::Result<(String, MultilineFlavor), ParseError> {
        // Longest first: ` `` ` is a prefix of ` ``` `. Each glyph carries its own
        // leading space, so this matches the same text the closer is built from
        // and the renderer emits -- one spelling, three uses.
        let opener = [Glyph::MultilineTriple, Glyph::MultilineDouble, Glyph::MultilineSingle]
            .into_iter()
            .find_map(|g| content.strip_prefix(g.text()).map(|rest| (g, rest)));
        let Some((opener, suffix)) = opener else {
            return Err(self.error_current("invalid multiline string opener"));
        };

        let local_eol = match suffix {
            "" | "\\n" => MultilineLocalEol::Lf,
            "\\r\\n" => MultilineLocalEol::CrLf,
            _ => {
                return Err(self.error_current(
                    "multiline string opener only allows \\n or \\r\\n after the backticks",
                ));
            }
        };

        // Closer must exactly match opener glyph including any explicit suffix.
        //
        // Built in the file frame, because it is compared literally against source
        // lines and those are what the file says. `line_indent` is structural, so
        // under an active ` /<` it sits `offset` columns right of where the text
        // actually is -- which made a correctly written closer read as a body line
        // and lost the document.
        let closer_indent = self.idt.to_file(line_indent).spaces();
        let closer = opener.at_with_suffix(closer_indent, suffix);
        let opener_line = self.line;
        self.line += 1;

        let (body, flavor) = match opener {
            Glyph::MultilineTriple => (
                self.parse_triple_backtick_body(local_eol, &closer, opener_line)?,
                MultilineFlavor::Triple,
            ),
            Glyph::MultilineDouble => (
                self.parse_double_backtick_body(local_eol, &closer, opener_line)?,
                MultilineFlavor::Double,
            ),
            Glyph::MultilineSingle => (
                self.parse_single_backtick_body(line_indent, local_eol, &closer, opener_line)?,
                MultilineFlavor::Single,
            ),
            Glyph::IndentOpen | Glyph::IndentClose => {
                unreachable!("only multiline glyphs reach here; the opener match cannot yield these")
            }
        };
        Ok((body, flavor))
    }

    fn parse_triple_backtick_body(
        &mut self,
        local_eol: MultilineLocalEol,
        closer: &str,
        opener_line: usize,
    ) -> std::result::Result<String, ParseError> {
        let mut value = String::new();
        let mut line_count = 0usize;
        loop {
            let Some(line) = self.current_line() else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line.text() == closer {
                self.line += 1;
                break;
            }
            if line_count > 0 {
                value.push_str(local_eol.bytes());
            }
            value.push_str(line.text());
            line_count += 1;
            self.line += 1;
        }
        self.check_multiline_minimum(&value, Glyph::MultilineTriple.body(), line_count)?;
        Ok(value)
    }

    fn parse_double_backtick_body(
        &mut self,
        local_eol: MultilineLocalEol,
        closer: &str,
        opener_line: usize,
    ) -> std::result::Result<String, ParseError> {
        let mut value = String::new();
        let mut line_count = 0usize;
        // The column the margin sits at, taken from the first body line. Any
        // column is accepted -- the margin points at space rather than carrying
        // depth, so where it sits is not structural -- but every line of one
        // string has to agree, because a margin's whole job is to give the reader
        // a straight edge to run down. A ragged one keeps the marker and destroys
        // what the marker is for. A folded line's `/ ` replaces the columns the
        // `| ` occupies -- the specification's "replaces the last two spaces of
        // the indent of `| `" -- so it sits at the margin too, not beside it.
        let mut margin: Option<ByteOffset> = None;
        loop {
            let Some(line) = self.current_line() else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line.text() == closer {
                self.line += 1;
                break;
            }
            let at = ByteOffset::new(count_leading_spaces(line.text()));
            let trimmed = &line[at..];
            if let Some(content_part) = trimmed.strip_prefix(Marker::Body.text()) {
                match margin {
                    None => margin = Some(at),
                    Some(expected) if at != expected => {
                        return Err(self.ragged_margin_error(at, expected, Marker::Body));
                    }
                    Some(_) => {}
                }
                if line_count > 0 {
                    value.push_str(local_eol.bytes());
                }
                value.push_str(content_part);
                line_count += 1;
            } else if let Some(cont_part) = trimmed.strip_prefix(Marker::Fold.text()) {
                let Some(expected) = margin else {
                    return Err(self.error_current(
                        "fold continuation cannot appear before any content in a `` multiline string",
                    ));
                };
                if at != expected {
                    return Err(self.ragged_margin_error(at, expected, Marker::Fold));
                }
                value.push_str(cont_part);
            } else if let Some(detail) = self.closer_fault(
                line,
                closer,
                "it reads as a body line -- and a body line has to start with '| ' or '/ '",
            ) {
                return Err(self.error_current(detail));
            } else {
                return Err(self.error_current(
                    format!("`` multiline string body lines must start with '{}' or '{}'", Marker::Body.text(), Marker::Fold.text()),
                ));
            }
            self.line += 1;
        }
        self.check_multiline_minimum(&value, Glyph::MultilineDouble.body(), line_count)?;
        Ok(value)
    }

    fn parse_single_backtick_body(
        &mut self,
        line_indent: LogicalIndent,
        local_eol: MultilineLocalEol,
        closer: &str,
        opener_line: usize,
    ) -> std::result::Result<String, ParseError> {
        // Everything below measures and slices the file's own lines, so cross to
        // the file frame once here. `n` arrives structural, and under an active
        // ` /<` that is `offset` columns right of where the text is -- which made
        // the indent check reject good bodies and, worse, sliced the value itself
        // at the wrong column.
        let body_indent = self.idt.to_file(line_indent);
        let content_indent = body_indent.deeper(1);
        let fold_marker = format!("{}{}", body_indent.spaces(), Marker::Fold.text());
        let mut value = String::new();
        let mut line_count = 0usize;
        loop {
            let Some(line) = self.current_line() else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line.text() == closer {
                self.line += 1;
                break;
            }
            if line.text().starts_with(&fold_marker) {
                if line_count == 0 {
                    return Err(self.error_current(
                        "fold continuation cannot appear before any content in a ` multiline string",
                    ));
                }
                value.push_str(line.byte_offset_of(content_indent).map_or("", |at| line.from(at)));
                self.line += 1;
                continue;
            }
            // Asked before the indent complaint below, which would otherwise
            // answer a plainly-written closer with a lecture about content lines.
            if let Some(detail) = self.closer_fault(
                line,
                closer,
                "it reads as one more line of the string's content",
            ) {
                return Err(self.error_current(detail));
            }
            // A line that does not reach the content indent at all is under it,
            // which is the same complaint -- `None` is the far side of `<`, not a
            // case with nothing to say.
            let under_indented = line
                .byte_offset_of(content_indent)
                .is_none_or(|at| ByteOffset::new(count_leading_spaces(line.text())) < at);
            if under_indented {
                return Err(self.error_current(
                    "` multiline string content lines must be indented at n+2 spaces",
                ));
            }
            if line_count > 0 {
                value.push_str(local_eol.bytes());
            }
            value.push_str(line.byte_offset_of(content_indent).map_or("", |at| line.from(at)));
            line_count += 1;
            self.line += 1;
        }
        self.check_multiline_minimum(&value, Glyph::MultilineSingle.body(), line_count)?;
        Ok(value)
    }

    fn parse_folded_json_string(
        &mut self,
        content: &str,
        fold_indent: LogicalIndent,
    ) -> std::result::Result<String, ParseError> {
        let mut json = content.to_owned();
        let start_line = self.line;
        self.line += 1;
        loop {
            let line = self
                .current_line()
                .ok_or_else(|| self.error_at_indent(start_line, fold_indent, "unterminated JSON string"))?
                .to_owned();
            self.ensure_line_has_no_tabs(self.line)?;
            // Spec: "A comment may not be within a fold." Checked before the
            // indent test, because a comment may sit at any indentation and
            // would otherwise be reported as an unterminated string, blaming
            // the line where the string opened rather than the comment.
            if line.text().trim_start_matches(' ').starts_with("//") {
                self.comment_in_fold(
                    self.line,
                    "this comment sits in the middle of a quoted string that \
                     continues below it -- move the comment above the whole value, \
                     or below it",
                )?;
                self.line += 1;
                continue;
            }
            let raw_fi = ByteOffset::new(count_leading_spaces(line.text()));
            if self.byte_offset_of(line, fold_indent) != Some(raw_fi) {
                return Err(self.error_at_indent(start_line, fold_indent, "unterminated JSON string"));
            }
            let rest = &line[raw_fi..];
            let Some(continued) = rest.strip_prefix(Marker::Fold.text()) else {
                return Err(self.error_at_indent(start_line, fold_indent, "unterminated JSON string"));
            };
            json.push_str(continued);
            self.line += 1;
            if let Some((value, end)) = parse_json_string_prefix(&json) {
                if end != json.len() {
                    return Err(self.error_current(
                        "folded JSON strings may not have trailing content after the closing quote",
                    ));
                }
                return Ok(value);
            }
        }
    }

    fn parse_minimal_json_line(
        &self,
        content: &str,
        span: Span,
    ) -> std::result::Result<T, ParseError> {
        // Both errors below report a position inside `content`, and `content` is a
        // slice of the line -- six callers hand it a different one. Adding the
        // fragment's own start is what turns those into positions in the line;
        // without it every caret is short by however far in the fragment began.
        // Derived from the span the value already carries, so the two cannot
        // disagree. `within` is what enforces that: it takes two document offsets
        // and yields a position inside the line, or `None` when the span starts
        // before the line said to contain it -- which is not a small number, it
        // is a caller that paired an offset with the wrong line. Falling back to
        // the line's start is a choice made here, in the open, rather than an
        // underflow quietly becoming zero inside the arithmetic.
        let fragment_start = self
            .line_offsets
            .get(self.line)
            .and_then(|line| DocumentOffset::new(span.start as usize).within(line.start))
            .unwrap_or(ByteOffset::START);
        if let Err(col) = is_valid_minimal_json(content) {
            return Err(self.error_at_line(
                self.line,
                fragment_start.plus(col),
                "invalid MINIMAL JSON (whitespace outside strings is forbidden)",
            ));
        }
        let value: JsonValue = serde_json::from_str(content).map_err(|error| {
            // serde reports a 1-based column within the fragment it was given.
            let col = error.column().saturating_sub(1);
            self.error_at_line(
                self.line,
                fragment_start.plus(col),
                format!("MINIMAL JSON must be valid JSON: {}", serde_reason(&error)),
            )
        })?;
        // The target decides how source facts apply to the fragment's interior —
        // e.g. an annotated tree marks interior strings Quoted, since that is how
        // JSON spells strings.
        Ok(T::from_minimal_json(value, ContainerFacts { span, table: false }))
    }

    fn line_str(&self, index: usize) -> Option<Line<'a>> {
        self.line_offsets.get(index).map(|s| s.text(self.input))
    }

    fn current_line(&self) -> Option<Line<'a>> {
        self.line_str(self.line)
    }

    fn skip_ignorable_lines(&mut self) -> std::result::Result<(), ParseError> {
        let mut first_comment: Option<usize> = None;
        // The indent of the previous line in the current run of comments, so a run
        // that steps back out can be refused.
        //
        // Never needs resetting, and a local rather than a field for the same
        // reason: a run cannot outlive one call. The loop below continues only on
        // a comment, so the first line that is not one ends the call -- and a blank
        // line between two comments is refused as a blank line before the question
        // of a run arises. Anything else at the end of a run is content, which is
        // what the run annotates.
        let mut run_indent: Option<usize> = None;
        while let Some(line) = self.current_line() {
            self.ensure_line_has_no_tabs(self.line)?;
            let trimmed = line.text().trim_start_matches(' ');
            if trimmed.starts_with("//") {
                let indent = line.text().len() - trimmed.len();
                // A run of comment lines may stay level or step further in, never
                // back out.
                //
                // A comment belongs to what it sits against, so where it sits is
                // what says which thing it annotates. A run that steps outward may
                // need its lines reordered for each comment to stay with its
                // subject -- and *may* is the whole point. Sometimes it would not:
                // both comments can be close enough to annotate the same thing.
                // Deciding which is not a question every parser should have to
                // answer, so the format asks the run to hold its column or move
                // inward, and a parser that only compares two numbers gets the rule
                // right.
                //
                // Checked whatever the tree does with comments: a document is
                // well formed or it is not, and `Value` discarding them afterwards
                // does not make an ill-formed one legal.
                if let Some(above) = run_indent
                    && indent < above
                {
                    let found = line.column_at(ByteOffset::new(indent)).number();
                    let outer = line.column_at(ByteOffset::new(above)).number();
                    return Err(self.error_at_line(
                        self.line,
                        ByteOffset::new(indent),
                        format!(
                            "this comment is at column {found}, further out than the \
                             comment on the line above it at column {outer}. A run of \
                             comment lines may stay at one column or step further in, \
                             never back out: a comment belongs to whatever it sits \
                             against, so a run that steps outward may need reordering \
                             for each comment to stay with its subject. Sometimes it \
                             would not -- both may be close enough to annotate the same \
                             thing -- but that is not a question every parser should \
                             have to answer. Line them up at column {outer}, indent \
                             this one further, or put the value they annotate between \
                             them."
                        ),
                    ));
                }
                run_indent = Some(indent);
                if first_comment.is_none() {
                    first_comment = Some(self.line);
                }
                if T::KEEPS_COMMENTS {
                    let comment = RawComment { col: indent, text: trimmed.to_owned() };
                    self.pending_comments.push(comment);
                }
                self.line += 1;
                continue;
            }
            // Spec 0.5.0, EOL Handling and Post-Processing Resistance: "TJSON HAS
            // NO ZERO-LENGTH LINES WITHIN IT, ASIDE FROM WITHIN TRANSPARENT TYPE
            // MULTILINE STRINGS", with one allowance -- "Extra empty lines at the
            // end shouldn't break the parser."
            //
            // Multiline string bodies never reach here, since
            // parse_multiline_string consumes its own lines, so transparent
            // multilines keep their empty lines as the spec intends.
            if trimmed.is_empty() {
                // Spec 0.5.0: "TRAILING SPACES ARE TREATED AS ERRORS BY DEFAULT
                // WHERE NOT MEANINGFUL." A line of nothing but spaces is not
                // meaningful outside a multiline body, and the allowance above is
                // for *empty* lines -- zero length, not whitespace-bearing -- so
                // it does not cover this one even at the end of the file.
                // Multiline bodies never reach here (they consume their own
                // lines), so a spaces-only line at this point is never data and
                // `Discard` cannot lose anything by reading it as blank.
                if !line.is_empty() && self.options.trailing_spaces == TrailingSpaces::Reject {
                    return Err(self.error_at_line(
                        self.line,
                        ByteOffset::START,
                        "a line of nothing but spaces is only meaningful inside a \
                         multiline string; here it is trailing whitespace -- remove \
                         the spaces, or delete the line",
                    ));
                }
                if self.only_blank_lines_remain(self.line) {
                    self.line += 1;
                    continue;
                }
                return Err(self.error_at_line(
                    self.line,
                    ByteOffset::START,
                    "blank lines are not allowed within TJSON; only trailing blank \
                     lines at the end of the document are ignored",
                ));
            }
            break;
        }

        // Spec: "A comment may not be within a fold." A line whose first
        // non-space character begins `/ ` is always a fold continuation --
        // nothing else in TJSON may start that way, which is exactly why a
        // bare string is forbidden from beginning with `/`. So a comment
        // sitting immediately before one is inside a fold, and this is the
        // single place that can see it: every fold walker stops at the comment
        // and reports whatever it was left holding, which is never the truth.
        if let Some(comment_line) = first_comment
            && let Some(next) = self.current_line()
            && next.text().trim_start_matches(' ').starts_with(Marker::Fold.text())
        {
            self.comment_in_fold(
                comment_line,
                "the `/ ` line below continues a value that started above this \
                 comment -- move the comment above the whole value, or below it",
            )?;
        }

        Ok(())
    }

    /// Is every line from `from` onward blank? Only then is a blank line trailing.
    fn only_blank_lines_remain(&self, from: usize) -> bool {
        (from..self.line_offsets.len())
            .filter_map(|index| self.line_str(index))
            .all(|line| line.text().trim_start_matches(' ').is_empty())
    }

    /// Apply the comment-in-fold policy to a comment sitting at `line_no`.
    ///
    /// `Ok(())` means carry on as though the comment were not on that line;
    /// `Err` rejects the document. `detail` completes the sentence "a comment
    /// may not appear inside a fold; ..." and says which construct this fold
    /// belongs to, since that is what tells a reader where to move it.
    ///
    /// One place, because five different fold walkers can meet a comment and
    /// they have to answer identically -- and because a policy decision should
    /// read as one decision rather than five coincidences.
    fn comment_in_fold(
        &mut self,
        line_no: usize,
        detail: &str,
    ) -> std::result::Result<(), ParseError> {
        match self.options.comment_placement_error {
            CommentPlacementError::Reject => Err(self.error_at_line(
                line_no,
                ByteOffset::START,
                format!("a comment may not appear inside a fold; {detail}"),
            )),
            CommentPlacementError::Hoist => {
                // There is no position inside a fold for a comment to occupy,
                // so it goes to the nearest one that exists: pending, which
                // drains onto the next node created. That is usually the value
                // this fold is building, which is where a reader would have put
                // it. On the table path the next node is the following row, so
                // a hoisted comment can land one row later than it was written.
                if T::KEEPS_COMMENTS
                    && let Some(line) = self.line_str(line_no)
                {
                    let trimmed = line.text().trim_start_matches(' ');
                    let comment = RawComment {
                        col: line.text().len() - trimmed.len(),
                        text: trimmed.to_owned(),
                    };
                    self.pending_comments.push(comment);
                }
                Ok(())
            }
            CommentPlacementError::Discard => Ok(()),
        }
    }

    /// Does `literal` actually end where it appears to?
    ///
    /// `true`, `false` and `null` are recognised by prefix, so without this
    /// `active:trued` reads as the boolean followed by a stray `d`. The `d` then
    /// surfaced far away as "expected at least two spaces between object
    /// entries", pointing at the start of the line and describing a separator
    /// the writer never meant -- a fault reported in the wrong place, about the
    /// wrong construct, naming no rule.
    ///
    /// What may follow is what may follow any packed value: nothing, the two
    /// spaces that separate entries, or a comma inside an array.
    fn check_literal_boundary(
        &self,
        content: &str,
        literal: &str,
        col: Option<ByteOffset>,
        context: ArrayLineValueContext,
    ) -> std::result::Result<(), ParseError> {
        let rest = &content[literal.len()..];
        let Some(next) = rest.chars().next() else {
            return Ok(());
        };
        // Only as far as this token runs. `content` is the whole remainder of the
        // line, so quoting it swallows every packed pair after the fault -- and
        // the suggested replacement then means something else entirely, because
        // the two spaces separating those pairs cannot appear inside a bare
        // string. Suggesting a line that parses differently is worse than saying
        // nothing.
        let token = &content[..token_end(content, context)];
        if next == ' '
            || (next == ',' && matches!(context, ArrayLineValueContext::ArrayLine(_)))
        {
            return Ok(());
        }
        Err(self.error_at_col(
            col.map(|c| c.plus(literal.len())),
            format!(
                "`{literal}` is followed by `{next}`, and nothing may follow it. \
                 `k:{literal}` writes the {} itself, so it has to end there. If you \
                 meant the text `{token}`, write it as a bare string with a space \
                 after the colon -- `k: {token}` -- since the space is what opens a \
                 string.",
                if literal == "null" { "null" } else { "boolean" }
            ),
        ))
    }

    /// A body line whose margin does not line up with the rest of its string.
    ///
    /// Names both columns, because the miss is usually one space and invisible on
    /// the page -- the same reason `closer_misindent` exists. Says which column
    /// the string chose rather than implying a fixed one, since any column is
    /// allowed as long as they agree.
    fn ragged_margin_error(&self, found: ByteOffset, expected: ByteOffset, marker: Marker) -> ParseError {
        self.error_at_line(
            self.line,
            found,
            format!(
                "this `{}` is at column {}, but this string's margin is at column {}. \
                 Any column is fine, but every line of one multiline string has to use \
                 the same one -- a margin is what gives a reader a straight edge to \
                 read down, and it stops being one as soon as the lines disagree. Move \
                 this line to column {}, or move the others to match it.",
                marker.text().trim_end(),
                self.column_of_margin(found),
                self.column_of_margin(expected),
                self.column_of_margin(expected),
            ),
        )
    }

    /// The column a margin offset reads as in a message.
    ///
    /// Everything before a margin is spaces, so bytes and columns coincide here
    /// and the current line serves for either offset. Routed through [`Column`]
    /// anyway, because that is where 0-based becomes 1-based and a site that does
    /// it by hand is a site that can stop agreeing with the caret beside it.
    fn column_of_margin(&self, at: ByteOffset) -> usize {
        match self.line_str(self.line) {
            Some(line) => line.column_at(at).number(),
            None => Column::at_unknown_line(at).number(),
        }
    }

    /// Was this line trying to be the closing glyph, and what stopped it counting?
    ///
    /// A closer is the bare glyph, alone on its line, at one fixed column. Both
    /// ways of missing that -- a column off, or spaces after it -- are invisible
    /// on the page, and the writer did close the string; they are not owed a
    /// complaint about body lines, they are owed the difference between what they
    /// wrote and a closer.
    ///
    /// `misplaced_reads_as` says what the line is taken for where it sits, which
    /// differs by flavour: the `` body reads it as a body line wanting a marker,
    /// the ` body as an indented content line.
    ///
    /// Recognition goes through [`is_attempted_closer`], which is the whole point
    /// of this rewrite: it used to strip only the *leading* spaces here, so a
    /// closer with a space after it was not an attempted closer at all and came
    /// back as "body lines must start with `| `" -- an instruction whose edit
    /// makes the line a body line and leaves the string unterminated.
    fn closer_fault(
        &self,
        line: Line<'_>,
        closer: &str,
        misplaced_reads_as: &str,
    ) -> Option<String> {
        let glyph = closer.trim_start_matches(' ');
        let text = line.text();
        if !is_attempted_closer(text, glyph) {
            return None;
        }
        // The closer is spaces then the glyph, so its indent is what precedes the
        // glyph; both columns go through `Column` rather than being 1-based by
        // hand, so the numbers in the message and any caret cannot drift.
        let expected =
            Line::new(closer).column_at(ByteOffset::new(closer.len() - glyph.len())).number();
        let found = line.column_at(ByteOffset::new(count_leading_spaces(text))).number();
        let has_trailing = text.trim_start_matches(' ') != glyph;
        Some(match (found == expected, has_trailing) {
            // Already a closer, so the caller matched it before asking.
            (true, false) => return None,
            (true, true) => format!(
                "this {glyph} has spaces after it, and a closing glyph has to be the whole \
                 line -- delete them and it closes the string opened above"
            ),
            (false, false) => format!(
                "the closing {glyph} glyph is at column {found} but belongs at column \
                 {expected}, one space further in than the key that opened the string. \
                 Where it is, {misplaced_reads_as}"
            ),
            (false, true) => format!(
                "this {glyph} is at column {found}, belongs at column {expected}, and has \
                 spaces after it -- a closer is the bare glyph alone on its line. Move it \
                 to column {expected} and delete the spaces after it."
            ),
        })
    }

    /// The error for a multiline string that never closed.
    ///
    /// "Reached end of file" describes where the parser gave up, not what went
    /// wrong, and sends the reader to the opener when the fault is almost always
    /// on the line that tried to be the closer. So say which column the glyph
    /// belonged at, and if some line downstream tried to be it, say what stopped
    /// it counting -- a closer is a bare glyph on its own line, and both ways of
    /// missing that (wrong indent, trailing spaces) are invisible on the page.
    fn unterminated_multiline(&self, opener_line: usize, closer: &str) -> ParseError {
        let glyph = closer.trim_start_matches(' ');
        let column = Line::new(closer).column_at(ByteOffset::new(closer.len() - glyph.len())).number();

        let mut probe = opener_line + 1;
        while let Some(line) = self.line_str(probe) {
            if is_attempted_closer(line.text(), glyph) {
                // One offset, one conversion. The column the prose states and
                // the column the caret lands on are the same value derived the
                // same way -- computing them separately is how they came to
                // disagree.
                let found_at = ByteOffset::new(count_leading_spaces(line.text()));
                let found = line.column_at(found_at).number();
                let reason = if found == column {
                    "has trailing spaces after it, and a closing glyph has to be the \
                     whole line"
                        .to_owned()
                } else {
                    format!("is at column {found}, not column {column}")
                };
                return self.error_at_line(
                    probe,
                    found_at,
                    format!(
                        "unterminated multiline string: the {glyph} on this line {reason}, \
                         so it did not close the string opened on line {}. The closer must \
                         be {glyph} alone at column {column}",
                        opener_line + 1
                    ),
                );
            }
            probe += 1;
        }

        self.error_at_line(
            opener_line,
            ByteOffset::START,
            format!(
                "unterminated multiline string: no closing {glyph} was found. It must be \
                 {glyph} alone on its own line at column {column}, one space further in \
                 than the key that opened the string."
            ),
        )
    }

    /// Why a comma packed array may not mix bare strings with anything else.
    ///
    /// The rule is right, but stating it is only useful when the bare element is
    /// really a word. When it reads as a number, a boolean or null, the writer
    /// meant a scalar and typed one space too many -- a comma separator is `, `
    /// and a second space opens a bare string, which is invisible. Naming the
    /// element and the space beats naming the rule.
    /// Why a bare string in a comma packed array may not itself hold a comma.
    ///
    /// Shares a home with `mixed_pack_error` because both are checked twice --
    /// once per element and again once a separator reveals the line is comma
    /// packed -- and a message written out at each of the four sites is a message
    /// that drifts at three of them.
    fn bare_comma_error(&self, col: Option<ByteOffset>) -> ParseError {
        self.error_at_col(
            col,
            "a bare string in a comma separated packed array may not contain a comma; \
             double quote it, or consider unpacking this line onto multiple lines",
        )
    }

    fn mixed_pack_error(&self, col: Option<ByteOffset>, bare_scalar: Option<&str>) -> ParseError {
        match bare_scalar {
            Some(text) => self.error_at_col(
                col,
                format!(
                    "`{text}` has two spaces before it, which makes it a bare string, \
                     while the other elements of this packed array are not. A comma \
                     separator is `, ` -- a second space opens a bare string. Delete \
                     the extra space and this is {}",
                    scalar_spelling(text).unwrap_or("a value")
                ),
            ),
            None => self.error_at_col(
                col,
                "a comma separated packed array must be either all bare strings or no \
                 bare strings; consider unpacking this line onto multiple lines",
            ),
        }
    }

    fn take_pending_comments(&mut self) -> Vec<RawComment> {
        if T::KEEPS_COMMENTS {
            std::mem::take(&mut self.pending_comments)
        } else {
            Vec::new()
        }
    }

    fn ensure_line_has_no_tabs(&self, line_index: usize) -> std::result::Result<(), ParseError> {
        let Some(line) = self.line_str(line_index) else {
            return Ok(());
        };
        // Only reject tabs in the leading indent — tabs inside quoted string values are
        // allowed. The run has to be measured over spaces *and* tabs: measuring it over
        // spaces alone made `line[..indent_end]` all spaces by construction, so the
        // search below could never succeed and every tab surfaced as some unrelated
        // complaint about indent depth.
        let indent_end =
            ByteOffset::new(line.text().len() - line.text().trim_start_matches([' ', '\t']).len());
        if let Some(column) = line[..indent_end].find('\t') {
            return Err(self.error_at_line(
                line_index,
                ByteOffset::new(column),
                "tab characters are not allowed as indentation",
            ));
        }
        Ok(())
    }

    fn looks_like_object_start(
        &self,
        content: &str,
        fold_indent: LogicalIndent,
    ) -> std::result::Result<bool, ParseError> {
        if content.starts_with('|') || starts_with_marker_chain(content) {
            return Ok(false);
        }
        if let Some(end) = parse_bare_key_prefix(content, &self.options) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok(true);
            }
            // Bare run fills the whole line and continues with `/ `. That is
            // either a folded key whose colon lands on a later line, or a
            // folded scalar (an array element, or the whole root). Only the
            // colon tells them apart, so reassemble and look for it rather
            // than assuming. Assuming "key" made a folded number in an array
            // fail to parse as `invalid object key`.
            if only_held_back_tail(content, end, &self.options)
                && self.folded_bare_has_colon(content, fold_indent)?
            {
                return Ok(true);
            }
        }
        if let Some((_, end)) = parse_json_string_prefix(content) {
            return Ok(content.get(end..).is_some_and(|rest| rest.starts_with(':')));
        }
        // A quoted string that doesn't close on this line may be a folded object
        // key OR a folded string value (an array element, or the whole root).
        // Both sit at the same indent and both continue with `/ `, so the only
        // thing telling them apart is whether a colon follows the reassembled
        // string. Assuming "key" here made every folded quoted value at root
        // fail to parse as `invalid object key`.
        if content.starts_with('"') && parse_json_string_prefix(content).is_none() {
            return self.folded_json_string_has_colon(content, fold_indent);
        }
        Ok(false)
    }

    /// What follows a fold, decided in one place.
    ///
    /// Every fold walker needs the same three-way answer, and each used to
    /// inline its own copy of the indent-and-`/ ` test. Keeping one definition
    /// means they cannot drift, and gives the comment case somewhere to live:
    /// the spec says a comment may not be within a fold, so it is neither a
    /// continuation nor a clean end.
    fn classify_fold_next(&self, line_no: usize, fold_indent: LogicalIndent) -> FoldNext<'a> {
        let Some(line) = self.line_str(line_no) else {
            return FoldNext::Ends;
        };
        let raw_fi = ByteOffset::new(count_leading_spaces(line.text()));
        // `Line::from` rather than `&line[raw_fi..]`: the tail is returned to the
        // caller inside `FoldNext`, so it has to borrow the input and not the
        // local `Line`.
        let rest = line.from(raw_fi);
        // A comment may sit at any indent, so this is checked before the indent
        // match rather than after it.
        //
        // Being a comment is not enough to be *inside* a fold, though: a comment
        // after the last line of a value is an ordinary comment belonging to
        // whatever comes next. What settles it is whether a continuation follows,
        // so look past this comment and any stacked below it before answering.
        if rest.starts_with("//") {
            let mut ahead = line_no + 1;
            while let Some(peek) = self.line_str(ahead) {
                if peek.text().trim_start_matches(' ').starts_with("//") {
                    ahead += 1;
                    continue;
                }
                break;
            }
            return match self.classify_fold_next(ahead, fold_indent) {
                FoldNext::Continues(_) => FoldNext::Comment,
                _ => FoldNext::Ends,
            };
        }
        if !rest.starts_with(Marker::Fold.text()) {
            return FoldNext::Ends;
        }
        if self.byte_offset_of(line, fold_indent) != Some(raw_fi) {
            // A fold marker, at a column that is not the one asked about. Saying
            // `Ends` here is what let a wrong `fold_indent` look like a finished
            // value.
            return FoldNext::ContinuesElsewhere;
        }
        FoldNext::Continues(&rest[2..])
    }

    /// Reassemble a quoted string across its `/ ` fold continuations and report
    /// whether a `:` follows it. Read-only lookahead: the caller re-walks the
    /// same lines once it has decided how to interpret them.
    fn folded_json_string_has_colon(
        &self,
        content: &str,
        fold_indent: LogicalIndent,
    ) -> std::result::Result<bool, ParseError> {
        let mut acc = content.to_owned();
        let mut next = self.line + 1;
        loop {
            let rest = match self.classify_fold_next(next, fold_indent) {
                FoldNext::Continues(rest) => rest,
                // As in the bare case, a comment cannot answer the question --
                // step over it and let the colon, or its absence, decide.
                FoldNext::Comment => {
                    next += 1;
                    continue;
                }
                FoldNext::Ends => break,
                // A `/ ` in a column this value does not own. Reported, not
                // absorbed: `Ends` used to swallow it and answer "the value
                // finished", so a stray marker became part of whatever the caller
                // built next -- no error, a different document.
                //
                // Two causes reach here. A malformed document, which is what this
                // message is for and is reachable from input (the sweep mutates
                // one into `  "` then `/ ` at column 1). Or a caller that passed
                // the wrong `fold_indent`, in which case a well-formed folded key
                // stops being recognised -- and now says so loudly instead of
                // quietly becoming an array element. See G2.
                // Now reported rather than absorbed. If this ever fires on a
                // well-formed document it means the caller's `fold_indent` is
                // wrong -- which is the other half of G2, and an error is how it
                // announces itself instead of quietly changing the document.
                FoldNext::ContinuesElsewhere => {
                    return Err(self.stray_fold_marker(next, fold_indent));
                }
            };
            acc.push_str(rest);
            next += 1;
            if let Some((_, end)) = parse_json_string_prefix(&acc) {
                return Ok(acc.get(end..).is_some_and(|r| r.starts_with(':')));
            }
        }
        Ok(false)
    }

    /// Reassemble a bare run across its `/ ` fold continuations and report
    /// whether a `:` follows it. The bare twin of `folded_json_string_has_colon`,
    /// and it exists for the same reason: at a given indent a folded key and a
    /// folded scalar look identical until the colon shows up, or fails to.
    ///
    /// Read-only lookahead; the caller re-walks the same lines once it has
    /// decided how to read them.
    fn folded_bare_has_colon(
        &self,
        content: &str,
        fold_indent: LogicalIndent,
    ) -> std::result::Result<bool, ParseError> {
        let mut acc = content.to_owned();
        let mut next = self.line + 1;
        loop {
            let rest = match self.classify_fold_next(next, fold_indent) {
                FoldNext::Continues(rest) => rest,
                // A comment cannot decide this question, so step over it and
                // keep looking. A packed array run ending in a comma looks
                // exactly like an unterminated bare key -- `1, 2,` parses as
                // the key `1, 2` with the comma held back -- and the two are
                // told apart only by whether a colon ever arrives below. If
                // one does this was a folded key, and the consuming path
                // reports the comment; if none does it was an array run, and
                // the comment was legally between two of them.
                FoldNext::Comment => {
                    next += 1;
                    continue;
                }
                FoldNext::Ends => break,
                // A `/ ` in a column this value does not own. Reported, not
                // absorbed: `Ends` used to swallow it and answer "the value
                // finished", so a stray marker became part of whatever the caller
                // built next -- no error, a different document.
                //
                // Two causes reach here. A malformed document, which is what this
                // message is for and is reachable from input (the sweep mutates
                // one into `  "` then `/ ` at column 1). Or a caller that passed
                // the wrong `fold_indent`, in which case a well-formed folded key
                // stops being recognised -- and now says so loudly instead of
                // quietly becoming an array element. See G2.
                // Now reported rather than absorbed. If this ever fires on a
                // well-formed document it means the caller's `fold_indent` is
                // wrong -- which is the other half of G2, and an error is how it
                // announces itself instead of quietly changing the document.
                FoldNext::ContinuesElsewhere => {
                    return Err(self.stray_fold_marker(next, fold_indent));
                }
            };
            acc.push_str(rest);
            next += 1;
            if let Some(end) = parse_bare_key_prefix(&acc, &self.options)
                && acc.get(end..).is_some_and(|r| r.starts_with(':'))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Error at a known byte column within the current line.
    ///
    /// `error_current` reports the line's first non-space character, which is the
    /// right place only for faults at the start of a line. Most faults are not:
    /// the column is threaded through parsing precisely so a mid-line problem can
    /// point at itself. Falls back when the column has been lost, which happens
    /// once a fold continuation moves content off the physical line.
    /// Two spaces sitting where a nesting marker belongs.
    ///
    /// Reported at the first of the two spaces, because that column is the one
    /// the missing glyph would have occupied. `depth` is how many markers were
    /// already read on this line, which is what makes the message able to say
    /// which level went unwritten rather than just that something is off.
    fn missing_marker_error(&self, col: ByteOffset, depth: usize) -> ParseError {
        self.error_at_line(
            self.line,
            col,
            format!(
                "two spaces sit here where a nesting marker belongs. This column is one \
                 level deeper than the {depth} marker(s) before it, so TJSON reads the \
                 level as being there either way -- what is missing is the `[ ` that \
                 tells a reader about it. Write `[ ` in place of the two spaces (only \
                 the deepest marker on a line may be `{{ `), or move the value two \
                 columns left to put it at the level above"
            ),
        )
    }

    fn error_at_col(&self, col: Option<ByteOffset>, message: impl Into<String>) -> ParseError {
        match col {
            Some(c) => self.error_at_line(self.line, c, message),
            None => self.error_current(message),
        }
    }

    /// Where a structural position lands in `line`'s bytes.
    ///
    /// The full reduction in one place: a logical indent crosses to the file
    /// frame (which is where an active ` /<` offset is accounted for), and only
    /// then does the line turn that column count into a byte position. Every
    /// error that points at a structural position goes through here or through
    /// [`Self::error_at_indent`], so none of them can skip the glyph offset the
    /// way a bare `indent + 1` did.
    ///
    /// Takes the line because the last step needs the text: an indent counts
    /// columns and a slice wants bytes, and only the characters in between say
    /// how far apart those are.
    fn byte_offset_of(&self, line: Line<'_>, indent: LogicalIndent) -> Option<ByteOffset> {
        line.byte_offset_of(self.idt.to_file(indent))
    }

    /// The content of `line` at and after a structural indent, empty when the
    /// line does not reach it.
    ///
    /// The clamp, stated where it is decided rather than hidden in the crossing:
    /// a line shorter than the indent has no content there, and empty is the
    /// honest answer for *slicing*. Comparisons must not use this -- they want
    /// [`Self::byte_offset_of`] and its `None`, or they end up comparing against
    /// the line's length by accident.
    fn content_at<'l>(&self, line: Line<'l>, indent: LogicalIndent) -> &'l str {
        self.byte_offset_of(line, indent).map_or("", |at| line.from(at))
    }

    /// A `/ ` continuation marker in a column no folded value owns.
    ///
    /// Raised from the fold *lookahead*, which is why it exists: the lookahead
    /// used to answer "the value ended" here, and a stray marker was absorbed
    /// into whatever the caller built next -- silently, and as a different
    /// document. See G2 in `local/fuzzer-found-breakage.md`.
    fn stray_fold_marker(&self, line_no: usize, fold_indent: LogicalIndent) -> ParseError {
        let marker = Marker::Fold.text().trim_end();
        let at = self
            .line_str(line_no)
            .map_or(ByteOffset::START, |line| {
                ByteOffset::new(count_leading_spaces(line.text()))
            });
        let found = self.line_str(line_no).map_or(1, |line| line.column_at(at).number());
        let want = Column::of_indent(self.idt.to_file(fold_indent)).number();
        self.error_at_line(
            line_no,
            at,
            format!(
                "this `{marker}` is at column {found}, but the value it would continue is \
                 folded at column {want}. A `{marker}` carries on the value above it and has \
                 to start in that value's column -- in any other column there is nothing \
                 above for it to continue, so it is not a continuation at all. Move it to \
                 column {want}, or delete it if the line above is complete.",
            ),
        )
    }

    /// A line inside an object that opens a value where an entry has to begin.
    ///
    /// An [`Opener`] is the value's own first column, so a line carrying one is
    /// a value and an object has nowhere to put a value that is not an entry.
    /// The reason it needs saying at all is that the text after the opener reads
    /// exactly like an entry -- `   b: 2` is a key one column right -- and it was
    /// accepted as one for as long as the slice stepped over the opener.
    fn opener_where_an_entry_belongs(&self, line: Line<'_>, leading: Leading) -> ParseError {
        let at = leading.content_start(line);
        let column = line.column_at(at).number();
        let opened = match leading.opener {
            Opener::BareString => "a bare string's opening space",
            Opener::Glyph => "the space that begins an indent offset glyph",
            // Unreachable: this is only called where `unopened` said no.
            Opener::None => "an opener",
        };
        self.error_at_line(
            self.line,
            at,
            format!(
                "column {column} holds {opened}, so this line is a value -- but it sits where an \
                 object's entries begin, and an object has nowhere to put a value that is not an \
                 entry. The text after it reads like a key because it is one column right of \
                 where a key goes. Delete the space so the key starts at column {column}, or \
                 move the value onto the line of the key it belongs to.",
            ),
        )
    }

    /// A marker written one column off the indent, where no marker may begin.
    ///
    /// Raised by [`Leading::of`] rather than by whichever walk happens to reach
    /// the line, because the column is wrong on its own terms: a marker stands
    /// inside the indent it points at, so it begins where a level ends, and the
    /// odd column between two levels is not one a marker can occupy whatever the
    /// line is doing. The two legal columns are the neighbours of the one found,
    /// which is the whole of what a reader needs to fix it.
    fn marker_off_column(&self, line_no: usize, line: Line<'_>, fault: OffColumnMarker) -> ParseError {
        let marker = fault.marker.text().trim_end();
        let found = line.column_at(fault.at).number();
        // Both neighbours are named as the nearest columns a marker may occupy,
        // never as two fixes to choose between: only one of them is the column
        // of the value this continues, and this function does not know which.
        // Whichever walk would have known is not reached -- the line is refused
        // where it is measured. Promising either one specifically would be an
        // escape route that leads somewhere else.
        self.error_at_line(
            line_no,
            fault.at,
            format!(
                "this `{marker}` is at column {found}, between two indent levels. A `{marker}` \
                 stands inside the indent it points at, so it can only begin where a level ends \
                 -- column {} or column {} here, and column {found} is neither. One space too \
                 many or one too few puts a marker here, and the space it then sits past is read \
                 as a bare string's opening quote, which a marker may not follow. Move it to the \
                 column the value it continues is folded at.",
                found - 1,
                found + 1,
            ),
        )
    }

    /// A ` />` written where no ` /<` has opened a frame.
    ///
    /// Names what is missing rather than what the line resembles. Nothing about
    /// this line is ambiguous -- it is unmistakably a closer -- so the fault is
    /// entirely that the thing it closes was never opened, and that is what the
    /// message says. It used to be read as ordinary content and reported by
    /// whichever parser claimed it, which produced `invalid object key` for a
    /// line containing no key.
    fn closer_with_nothing_open(&self, line: Line<'_>) -> ParseError {
        let close = Glyph::IndentClose.body();
        let open = Glyph::IndentOpen.body();
        let at = ByteOffset::new(count_leading_spaces(line.text()));
        self.error_at_line(
            self.line,
            at,
            format!(
                "this `{close}` closes an indent offset frame, but no `{open}` has opened one, \
                 so there is no frame here to close. A `{close}` is only ever the second half of \
                 a pair. Delete it, or add the `{open}` above that it was meant to close.",
            ),
        )
    }

    /// A ` />` that is not where the frame it would close puts its closer.
    ///
    /// Both directions land here: too shallow used to dedent out of the object
    /// and be reparsed as a root-level key, which reported `invalid object key`
    /// and never mentioned the glyph; too deep used to be read as ordinary
    /// content one level in, which raised nothing at all and silently moved the
    /// following entries inside the container.
    fn misplaced_closer(&self, line: Line<'_>, expected: FileIndent) -> ParseError {
        let glyph = Glyph::IndentClose.body();
        let found_at = ByteOffset::new(count_leading_spaces(line.text()));
        let found = line.column_at(found_at).number();
        // The frame's indent names the column the glyph's own leading space
        // occupies, so the `/>` a reader sees begins one column further right.
        let want = Column::of_indent(expected).number() + Opener::Glyph.width();
        self.error_at_line(
            self.line,
            found_at,
            format!(
                "this `{glyph}` is at column {found}, but the `{}` it closes puts its closer \
                 at column {want}. The two have to sit at the same column -- that pairing is \
                 the only thing that says where the shifted frame ends, so at any other \
                 column this line is not a closer at all and gets read as ordinary content \
                 one level further in. Move it to column {want}.",
                Glyph::IndentOpen.body(),
            ),
        )
    }

    /// A position `bytes` into the content that begins at `indent` on the current
    /// line.
    ///
    /// For errors that point at the end of a construct rather than its start: the
    /// indent is a column count, `bytes` is a length measured on the content
    /// already sliced out of the line, and the two may only be added once the
    /// first has become a byte position — which is what this does and what doing
    /// it by hand at a call site gets wrong.
    fn byte_offset_past(&self, indent: LogicalIndent, bytes: usize) -> ByteOffset {
        self.current_line()
            .and_then(|line| self.byte_offset_of(line, indent))
            .unwrap_or(ByteOffset::START)
            .plus(bytes)
    }

    /// Report an error at a structural indent on `line_index`.
    ///
    /// Saves every call site the crossing: they name the indent they are
    /// reasoning about, and the byte position a caret needs is worked out here,
    /// where the line is already in hand. A missing line reports at the margin,
    /// which is where a caret with no text to point into belongs.
    fn error_at_indent(
        &self,
        line_index: usize,
        indent: LogicalIndent,
        message: impl Into<String>,
    ) -> ParseError {
        let at = self
            .line_str(line_index)
            .and_then(|line| self.byte_offset_of(line, indent))
            .unwrap_or(ByteOffset::START);
        self.error_at_line(line_index, at, message)
    }

    fn error_current(&self, message: impl Into<String>) -> ParseError {
        let at = self
            .current_line()
            .map_or(ByteOffset::START, |line| ByteOffset::new(count_leading_spaces(line.text())));
        self.error_at_line(self.line, at, message)
    }

    /// Report at a byte offset into `line_index`. Callers pass the offset they
    /// already hold — a slice position, a scan result — and the column a reader
    /// is told is derived here, so no call site does that arithmetic itself.
    fn error_at_line(
        &self,
        line_index: usize,
        at: ByteOffset,
        message: impl Into<String>,
    ) -> ParseError {
        // An error raised once the input ran out holds a cursor, not a position.
        // The line it names does not exist, so it prints no source and no caret
        // and points the reader past the end of their own file. Where the
        // document actually stopped is the end of the last line someone wrote --
        // a trailing newline does not add a line anyone typed, and TJSON has no
        // blank lines, so the last non-empty one is that line.
        let (line_index, at) = match self.line_str(line_index) {
            Some(_) => (line_index, at),
            None => match self.last_written_line() {
                Some((last, len)) => (last, ByteOffset::new(len)),
                None => (line_index, at),
            },
        };
        let source = self.line_str(line_index);
        let column = match source {
            Some(line) => line.column_at(at),
            None => Column::at_unknown_line(at),
        };
        ParseError::new(line_index + 1, column, message, source.map(|line| line.text().to_owned()))
    }

    /// The last line with anything on it, and its byte length.
    fn last_written_line(&self) -> Option<(usize, usize)> {
        (0..self.line_offsets.len())
            .rev()
            .map(|index| (index, self.line_offsets[index].len))
            .find(|&(_, len)| len > 0)
    }
}


fn bare_string_end(content: &str, context: ArrayLineValueContext) -> usize {
    match context {
        ArrayLineValueContext::ArrayLine(_) | ArrayLineValueContext::ObjectValue => {
            bare_string_run(content)
        }
        ArrayLineValueContext::SingleValue => content.len(),
    }
}

/// Does the text after a colon open a packed array?
///
/// Array starter 2 (inline variant) is "space space", so a two-space gap after
/// the colon opens a nonempty packed array. That is not a BASIC TYPE -- the spec
/// is explicit that "an object key value pair is not a basic type" -- so it may
/// only be the first pair on a line, never a packed continuation.
///
/// MINIMAL JSON (`k:[1,2]`) is unaffected: it admits no whitespace at all, so it
/// never presents a gap here.
fn opens_array_starter_2(after_colon: &str) -> bool {
    after_colon.starts_with("  ")
}

/// How `text` would read if it were not a bare string: as a number, a boolean or
/// null. `None` when it is genuinely just a word.
fn scalar_spelling(text: &str) -> Option<&'static str> {
    match text {
        "true" | "false" => Some("a boolean"),
        "null" => Some("null"),
        _ if text.parse::<Number>().is_ok() => Some("a number"),
        _ => None,
    }
}

/// Is everything past `end` just the tail a bare key run holds back?
///
/// A run stops short of a trailing space, comma or quote because it may not end
/// on one. When a fold continuation follows, the value does not end there at all,
/// so that tail is interior content and the run does reach the end of the line.
///
/// The set is [`is_held_back_from_run_end`] and not a list written here: this and
/// [`parse_bare_key_prefix`] are one rule asked from two directions, and a copy
/// each is how they came to disagree about PIPELIKE.
fn only_held_back_tail(content: &str, end: usize, forms: &ParseOptions) -> bool {
    content[end..].chars().all(|ch| is_held_back_from_run_end(ch, forms))
}

/// How far a bare string runs.
///
/// A bare string has no closing delimiter, so there is nothing to search for.
/// It ends where the format's own rules say the next character can no longer
/// belong to it, which makes this a walk rather than a scan for a terminator.
///
/// One rule does all the work, and it comes straight from the definition of a
/// bare string: only single spaces inside it ("No space space allowed"), so a
/// second space in a row cannot be part of the string and ends it. `pending` is
/// the one space held back while we wait to see whether content follows -- if
/// the walk finishes with it still pending, it was separator or trailing
/// whitespace, never part of the string.
///
/// A comma is ordinary content here, including one sitting at the very end. The
/// walk is only ever reached for a bare string, and the one array format whose
/// separator is a comma forbids bare strings outright, so no comma this walk can
/// see is ever a separator. A bare string still may not *end* on one -- but that
/// is a rule about the finished value, so it belongs to `check_bare_string` and
/// not here. Dropping the comma instead would make `k: value,` silently parse as
/// "value", which is exactly the class of error the rule exists to catch.
///
/// Nothing here knows about arrays, and that is the point. A single `, ` cannot
/// end a bare string -- an internal comma and an internal single space are both
/// legal -- so `a, 1` is the one string "a, 1", not the two elements "a" and 1.
/// And because a bare string can never hold a double space, it can never hold
/// `,  ` either, which is why any bare string is automatically safe as an array
/// element and the renderer needs no comma check of its own.
fn bare_string_run(content: &str) -> usize {
    let mut end = 0; // bytes that are definitely part of the string
    let mut pending = 0; // a single trailing space, not yet part of it
    let mut prev_space = false;

    for ch in content.chars() {
        if ch == ' ' {
            if prev_space {
                break; // two in a row: cannot be inside a bare string
            }
            prev_space = true;
            pending += ch.len_utf8();
        } else {
            prev_space = false;
            end += pending + ch.len_utf8(); // content: everything held back is real
            pending = 0;
        }
    }
    end
}

/// Where a non-bare token (number, `true`, `false`, `null`) ends on an array line.
///
/// Unlike a bare string this *does* stop at `, `: a scalar carries no leading
/// space, so a single space after the comma is Array separator 2 rather than
/// content.
/// serde_json's description of a failure, without the ` at line N column M` it
/// appends.
///
/// Those coordinates are relative to the fragment serde was handed, not to the
/// document, so printing them lands two different frames of reference in one
/// sentence -- the caller's caret says column 4 while serde's text says column
/// 2, and both are correct about different things. The position is already
/// carried properly by the caller; this is only the reason.
fn serde_reason(error: &serde_json::Error) -> String {
    let text = error.to_string();
    match text.rfind(" at line ") {
        Some(cut) => text[..cut].to_owned(),
        None => text,
    }
}

/// The literal `token` was trying to spell, when it differs from one only by
/// case (`TRUE`, `Null`, `fAlSe`).
///
/// Worth asking as its own question because the bare-string ladder further down
/// answers this case with "a bare string opens with a space, and there is none
/// here" -- advice that is true, parses, and then silently yields the *string*
/// `"TRUE"` instead of the boolean the writer plainly meant. A suggestion that
/// works and produces the wrong value is worse than one that fails.
fn miscased_literal(token: &str) -> Option<&'static str> {
    ["true", "false", "null"]
        .into_iter()
        .find(|literal| token != *literal && token.eq_ignore_ascii_case(literal))
}

/// Was this line trying to be `glyph`, alone on its line?
///
/// One recognition test, so the places that ask cannot disagree about what counts
/// as an attempt. They did: the body loops stripped only the leading spaces while
/// the search for a would-be closer downstream stripped both ends, so a closer
/// with a space after it was an attempted closer to one of them and invisible to
/// the other -- and the reader got a message about body-line markers for a line
/// that was plainly a closer.
fn is_attempted_closer(text: &str, glyph: &str) -> bool {
    text.trim_start_matches(' ').trim_end() == glyph
}

fn simple_token_end(content: &str, context: ArrayLineValueContext) -> usize {
    match context {
        ArrayLineValueContext::ArrayLine(_) => {
            let mut end = content.len();
            if let Some(index) = content.find(", ") {
                end = end.min(index);
            }
            if let Some(index) = content.find("  ") {
                end = end.min(index);
            }
            if content.ends_with(',') {
                end = end.min(content.len() - 1);
            }
            end
        }
        ArrayLineValueContext::ObjectValue => content.find("  ").unwrap_or(content.len()),
        ArrayLineValueContext::SingleValue => content.len(),
    }
}

/// Where the token at the front of `content` ends, never including the spaces at
/// the end of the line.
///
/// The tokens this bounds are numbers and the three literals, none of which can
/// contain a space, so trailing ones were never part of the token -- they are the
/// line's, and the line has its own policy for them. Swallowing them turned a
/// stray space at the end of `scores:  90 ` into `invalid JSON number: "90 "`,
/// which names a token nobody wrote and says nothing about the space.
fn token_end(content: &str, context: ArrayLineValueContext) -> usize {
    let end = simple_token_end(content, context);
    content[..end].trim_end_matches(' ').len()
}

#[cfg(test)]
mod lookalike_set_tests {
    use super::*;
    use crate::value::Value;

    fn parse_with(input: &str, forms: ParseOptions) -> std::result::Result<Value, ParseError> {
        Parser::<Value>::parse_document(input, forms)
    }

    /// A caller with no lookalikes at all -- the recovery reading. Nothing is
    /// waived by it except deception, because that is all the sets hold.
    fn no_lookalikes() -> ParseOptions {
        ParseOptions::default()
            .commalike(&[])
            .expect("empty is always a legal set")
            .colonlike(&[])
            .expect("empty is always a legal set")
            .pipelike(&[])
            .expect("empty is always a legal set")
            .quotelike(&[])
            .expect("empty is always a legal set")
    }

    #[test]
    fn emptying_a_set_recovers_a_lookalike_but_never_the_delimiter() {
        // U+2502 BOX DRAWINGS LIGHT VERTICAL is PIPELIKE. It deceives a reader,
        // but it is not the character a table row is split on, so a document
        // containing one can be recovered by a caller who drops the set.
        let lookalike = "  k: abc\u{2502}";
        assert!(
            parse_with(lookalike, ParseOptions::default()).is_err(),
            "the specification's sets reject a pipelike"
        );
        assert!(
            parse_with(lookalike, no_lookalikes()).is_ok(),
            "a caller holding no lookalikes recovers it"
        );

        // The ASCII pipe is what a table row is actually split on. It is in no
        // set, so emptying every set does not reach it.
        let real = "  k: abc|";
        assert!(parse_with(real, ParseOptions::default()).is_err());
        assert!(
            parse_with(real, no_lookalikes()).is_err(),
            "a structural character is not a member of any set and cannot be waived"
        );
    }

    #[test]
    fn a_set_may_not_contain_its_own_structural_character() {
        // The whole reason the sets are safe to hand out: a caller cannot put
        // the real thing in one and then remove it.
        for (name, result) in [
            ("commalike", ParseOptions::default().commalike(&[','])),
            ("colonlike", ParseOptions::default().colonlike(&[':'])),
            ("pipelike", ParseOptions::default().pipelike(&['|'])),
            ("quotelike", ParseOptions::default().quotelike(&['"'])),
        ] {
            let error = result.expect_err("{name} must refuse its structural character");
            assert!(
                error.contains(name) && error.contains("it is TJSON syntax"),
                "{name} error should name the set and say why: {error}"
            );
        }
    }

    #[test]
    fn an_unsorted_or_repeating_set_is_refused() {
        // Silent misbehaviour otherwise: the sets are binary searched, so an
        // unsorted one stops matching some of its own members.
        let unsorted = ParseOptions::default()
            .pipelike(&['\u{2502}', '\u{00A6}'])
            .expect_err("descending set must be refused");
        assert!(unsorted.contains("sorted"), "should say what is wrong: {unsorted}");

        let repeated = ParseOptions::default()
            .commalike(&['\u{3001}', '\u{3001}'])
            .expect_err("repeated member must be refused");
        assert!(repeated.contains("twice"), "should say what is wrong: {repeated}");
    }

    #[test]
    fn narrowing_a_set_reaches_the_key_path_too() {
        // Bare keys read through the same sets as bare strings. This was the
        // half that used to be hardcoded to the specification's reading, so a
        // caller could recover a value holding a lookalike but not a key.
        //
        // Both characters below are what makes the sets load bearing for keys
        // at all: each is a *letter*, so the positive character rules a bare
        // key is built on admit it, and only its set turns it away. A lookalike
        // that is not a letter -- U+2502, say -- never reaches the sets, since
        // a bare key does not admit symbols in the first place.
        let leading_pipelike = "  \u{01C0}ab: 1"; // LATIN LETTER DENTAL CLICK, Lo
        let trailing_commalike = "  ab\u{02BB}: 1"; // MODIFIER LETTER TURNED COMMA, Lm

        for input in [leading_pipelike, trailing_commalike] {
            assert!(
                parse_with(input, ParseOptions::default()).is_err(),
                "{input:?} must be refused under the specification's sets"
            );
            assert!(
                parse_with(input, no_lookalikes()).is_ok(),
                "{input:?} must be recovered by a caller holding no lookalikes"
            );
        }
    }

    #[test]
    fn rules_that_are_not_lookalike_sets_are_never_waived() {
        // Two spaces separate packed entries, and the invisible classes cannot
        // be seen at all. Neither is a set, so there is nothing to empty.
        for input in ["  k: a  b", "  k: ab\u{200B}c"] {
            assert!(
                parse_with(input, ParseOptions::default()).is_err(),
                "{input:?} must fail under the specification"
            );
            assert!(
                parse_with(input, no_lookalikes()).is_err(),
                "{input:?} must still fail with every set emptied"
            );
        }
    }
}

#[cfg(test)]
mod parse_policy_tests {
    use super::*;
    use crate::document::Node;
    use crate::options::{
        ByteOrderMark, CommentPlacementError, MissingIndentMarker, MultilineMinimum,
        TrailingSpaces,
    };
    use crate::value::Value;

    fn parse(input: &str, options: ParseOptions) -> std::result::Result<Value, ParseError> {
        Parser::<Value>::parse_document(input, options)
    }

    /// The value stored at a top-level key, as a string.
    fn key_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
        let Value::Object(entries) = value else { return None };
        entries.iter().find(|entry| entry.key == key).and_then(|entry| match &entry.value {
            Value::String(text) => Some(text.as_str()),
            _ => None,
        })
    }

    #[test]
    fn trailing_spaces_reject_by_default_and_discard_on_request() {
        // Every shape of non-data trailing space: after a scalar, after a bare
        // string, two of them (which would otherwise read as a separator with
        // nothing after it), and a line holding nothing else.
        for input in [
            "  a: 1 \n",
            "  a: 1  \n",
            "  a: b \n",
            "  a:\"q\" \n",
        ] {
            assert!(
                parse(input, ParseOptions::default()).is_err(),
                "{input:?} must be refused by the specification's reading"
            );
            assert!(
                parse(input, ParseOptions::default().trailing_spaces(TrailingSpaces::Discard))
                    .is_ok(),
                "{input:?} must parse once trailing spaces are discarded"
            );
        }
    }

    #[test]
    fn a_spaces_only_line_is_a_blank_line_once_the_spaces_are_discarded() {
        // Discarding trailing spaces does not rescue this: take the spaces away
        // and a blank line is left, and a blank line in the middle of a document
        // breaks a different rule. The option moves which rule is reported, not
        // whether the document loads.
        let input = "  a: 1\n   \n  b: 2\n";
        let strict = parse(input, ParseOptions::default()).unwrap_err().to_string();
        assert!(strict.contains("nothing but spaces"), "{strict}");

        let discarded =
            parse(input, ParseOptions::default().trailing_spaces(TrailingSpaces::Discard))
                .unwrap_err()
                .to_string();
        assert!(
            !discarded.contains("nothing but spaces"),
            "the spaces are no longer the complaint: {discarded}"
        );
    }

    #[test]
    fn trailing_space_message_does_not_describe_a_packed_entry() {
        // A single trailing space used to be reported as a missing separator
        // between two entries on one line, which describes a line the author
        // did not write. Nothing follows, so nothing was being separated.
        let error = parse("  a: 1 \n", ParseOptions::default()).unwrap_err();
        let text = error.to_string();
        assert!(
            text.contains("ends with spaces"),
            "should name the trailing spaces: {text}"
        );
        assert!(
            !text.contains("between object entries"),
            "should not describe a separator that was never intended: {text}"
        );
    }

    #[test]
    fn trailing_spaces_inside_a_multiline_body_stay_data() {
        // The spec's "where not meaningful" clause: inside a multiline string a
        // trailing space is content, so neither reading may touch it.
        let input = "  a: ``\n| x \n| y\n   ``\n";
        for policy in [TrailingSpaces::Reject, TrailingSpaces::Discard] {
            let value = parse(input, ParseOptions::default().trailing_spaces(policy))
                .unwrap_or_else(|e| panic!("multiline must parse under {policy:?}: {e}"));
            let text = key_str(&value, "a").expect("string value");
            assert!(
                text.contains("x "),
                "the space after x is data and must survive {policy:?}: {text:?}"
            );
        }
    }

    #[test]
    fn byte_order_mark_rejected_by_default_and_discarded_on_request() {
        let input = "\u{FEFF}  a: 1\n";
        let error = parse(input, ParseOptions::default()).unwrap_err().to_string();
        assert!(
            error.contains("byte order mark") && error.contains("without a BOM"),
            "the error must name the mark and say how to remove it: {error}"
        );

        let value = parse(input, ParseOptions::default().byte_order_mark(ByteOrderMark::Discard))
            .expect("a leading mark is skipped on request");
        assert!(
            matches!(&value, Value::Object(entries) if entries.iter().any(|e| e.key == "a")),
            "the document past the mark parses normally"
        );
    }

    #[test]
    fn u_feff_stays_forbidden_away_from_byte_zero() {
        // Only byte 0 is an encoding artifact. Anywhere else it is an invisible
        // character sitting in data, and no reading of the option reaches it.
        let input = "  a: b\u{FEFF}c\n";
        for policy in [ByteOrderMark::Reject, ByteOrderMark::Discard] {
            assert!(
                parse(input, ParseOptions::default().byte_order_mark(policy)).is_err(),
                "U+FEFF inside a value must stay forbidden under {policy:?}"
            );
        }
    }

    #[test]
    fn too_many_spaces_before_a_value_shows_the_whole_ladder() {
        // The three rungs do not imply one another, and a reader who counted
        // wrong needs to see which one they meant. The old text described a
        // packed array followed by a key-value pair -- a different fault, which
        // has its own error.
        let error = parse("  a:    x\n", ParseOptions::default()).unwrap_err().to_string();
        for rung in ["`k: x`", "`k:  1, 2`", "`k:   x`"] {
            assert!(error.contains(rung), "the message must show {rung}: {error}");
        }
        assert!(
            !error.contains("BASIC TYPE"),
            "and must not explain a fault that is reported elsewhere: {error}"
        );
    }

    #[test]
    fn a_missing_opening_space_is_named_as_such() {
        // `a:  x` is one space short, not malformed: two spaces opened a packed
        // array and its element still needs the space that opens a bare string.
        // "invalid value start" pointed at the text, where nothing is wrong,
        // instead of at the gap before it.
        let error = parse("  a:  x\n", ParseOptions::default()).unwrap_err().to_string();
        assert!(
            error.contains("opening quote"),
            "the message must say the space is the opening quote: {error}"
        );
        assert!(
            !error.contains("invalid value start"),
            "and must not fall back to the generic fault: {error}"
        );

        // A character that could not open a bare string either way keeps the
        // generic message -- the space is not the whole story there.
        let quoted = parse("  a:  \u{201C}x\n", ParseOptions::default()).unwrap_err().to_string();
        assert!(
            !quoted.contains("opening quote is"),
            "a quotelike start is a different fault: {quoted}"
        );
    }

    #[test]
    fn a_literal_must_end_where_it_appears_to() {
        // `true`, `false` and `null` are matched by prefix, so `active:trued`
        // once read as the boolean plus a stray `d`. The `d` then surfaced as
        // "expected at least two spaces between object entries", pointing at
        // column 3 of a line whose fault was at its very end.
        let error = parse(
            "  name: Alice    age:30    active:trued\n",
            ParseOptions::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("`true` is followed by `d`"),
            "must name the literal and what follows it: {error}"
        );
        assert!(
            !error.to_string().contains("between object entries"),
            "must not blame the separators, which are fine: {error}"
        );
        assert!(
            error.column() > 30,
            "must point at the `d` near the end, not the start of the line: {error:?}"
        );

        for input in ["  a:nullx\n", "  a:falsey\n"] {
            assert!(
                parse(input, ParseOptions::default()).is_err(),
                "{input:?} must be refused"
            );
        }

        // What legitimately follows a literal keeps working: end of line, the
        // two spaces separating packed entries, and a comma inside an array.
        for input in ["  a:true\n", "  a:true  b:1\n", "  a:  true, false\n"] {
            assert!(
                parse(input, ParseOptions::default()).is_ok(),
                "{input:?} must still parse"
            );
        }
    }

    #[test]
    fn a_misindented_closer_names_both_columns() {
        // The glyph carries no indentation cue and the miss is usually one
        // space, so it cannot be seen on the page. Reporting it as a malformed
        // body line is true and useless -- the string was closed, just a column
        // off, and only naming where it went and where it belonged helps.
        for (input, found) in [
            ("  a: ``\n| x\n| y\n  ``\n", 3),
            ("  a: ``\n| x\n| y\n    ``\n", 5),
        ] {
            let error = parse(input, ParseOptions::default()).unwrap_err().to_string();
            assert!(
                error.contains(&format!("at column {found}")),
                "must say where the glyph is: {error}"
            );
            assert!(
                error.contains("belongs at column 4"),
                "must say where it belonged: {error}"
            );
            assert!(!error.contains("````"), "the glyph must not be re-quoted: {error}");
        }

        // The correctly indented closer is still accepted.
        assert!(parse("  a: ``\n| x\n| y\n   ``\n", ParseOptions::default()).is_ok());
    }

    #[test]
    fn an_unterminated_multiline_points_at_the_line_that_tried_to_close_it() {
        // "reached end of file" described where the parser gave up, not what
        // went wrong, and sent the reader to the opener when the fault was on
        // the closer. Both ways of missing a closer are invisible on the page,
        // so each has to be named outright.
        let trailing = parse("  a: ```\nx\ny\n   ```   \n", ParseOptions::default())
            .unwrap_err();
        assert_eq!(trailing.line(), 4, "must blame the closer line, not the opener");
        assert!(
            trailing.to_string().contains("trailing spaces"),
            "must say what stopped it counting: {trailing}"
        );

        let misindented = parse("  a: ```\nx\ny\n ```\n", ParseOptions::default())
            .unwrap_err();
        assert_eq!(misindented.line(), 4);
        assert!(
            misindented.to_string().contains("at column 2, not column 4"),
            "must name both columns: {misindented}"
        );

        // With nothing resembling a closer anywhere, the opener is the right
        // place to point, and the message says where the glyph belonged.
        let absent = parse("  a: ```\nx\ny\n", ParseOptions::default()).unwrap_err();
        assert_eq!(absent.line(), 1);
        assert!(
            absent.to_string().contains("at column 4"),
            "must still say where the closer belonged: {absent}"
        );

        assert!(parse("  a: ```\nx\ny\n   ```\n", ParseOptions::default()).is_ok());
    }

    /// A run of comment lines may stay level or step in, never back out.
    ///
    /// Where a comment sits is what says which thing it annotates, so a run that
    /// steps outward annotates something enclosing what the line above it
    /// annotated. Both cannot be kept with their subjects on a re-render without
    /// emitting them in an order the author did not write, so the run is refused
    /// rather than silently reordered.
    ///
    /// Only lines that touch form a run: separate them with the value they
    /// annotate and any indents are legal again.
    #[test]
    fn a_run_of_comments_may_not_step_back_out() {
        for legal in [
            "// a\n// b\n  k:1\n",              // level
            "// a\n  // b\n  k:1\n",            // stepping in
            "// a\n  // b\n    // c\n  k:1\n",  // stepping in twice
            "// a\n  k:1\n// b\n  j:2\n",       // not a run: content between them
        ] {
            parse(legal, ParseOptions::default())
                .unwrap_or_else(|e| panic!("{legal:?} is a legal run: {e}"));
        }

        for illegal in ["  // a\n// b\n  k:1\n", "    // a\n  // b\n  k:1\n"] {
            let error = parse(illegal, ParseOptions::default())
                .expect_err(&format!("{illegal:?} steps back out and must be refused"));
            assert!(
                error.message().contains("further out than the comment on the line above"),
                "{illegal:?} must be refused for stepping out, not for something else: {error}"
            );
        }
    }

    /// The renderer never writes a run the parser would now refuse.
    ///
    /// A new parse restriction is only safe if the generator already respects it;
    /// otherwise the two halves of the library disagree and a document this crate
    /// wrote cannot be read back by it.
    #[test]
    fn comment_runs_this_crate_writes_are_ones_it_accepts() {
        let sources = [
            "// above the root\n  a:1\n  b:2\n",
            "  a:1\n// between entries\n  b:2\n",
            "// one\n// two\n  a:1\n",
            "  k:\n// above a container\n    a:1\n",
            "  k:\n// above a marker chain\n  [ { a:1\n",
            "// outer\n  a:\n    // inner\n    b:1\n",
        ];
        for source in sources {
            let document: crate::Document =
                source.parse().unwrap_or_else(|e| panic!("{source:?}: {e}"));
            let rendered = document.to_tjson_with(crate::RenderOptions::default());
            rendered.parse::<crate::Document>().unwrap_or_else(|e| {
                panic!("re-rendered {source:?} into a document this parser refuses: {e}\n{rendered}")
            });
        }
    }

    /// Every multiline flavour recognises a line that was trying to be its closer.
    ///
    /// The three bodies each run their own loop, and only ``` used to say what was
    /// wrong with a closer carrying a space after it. `` called it a body line
    /// missing its marker and ` called it a content line at the wrong indent --
    /// both true about where the line sits, both useless about what the writer
    /// did, and the edit each one asks for leaves the string unterminated.
    #[test]
    fn every_multiline_flavour_names_a_closer_that_missed() {
        for (label, input) in [
            ("``", "  k: ``\n  | ab\n   `` \n"),
            ("`", "  k: `\n    ab\n   ` \n"),
            ("```", "  k: ```\n    ab\n   ``` \n"),
        ] {
            let error = parse(input, ParseOptions::default()).unwrap_err().to_string();
            assert!(
                error.contains("spaces after it"),
                "{label}: the fault is the spaces after the closer, and the message has to \
                 say so: {error}"
            );
            assert!(
                !error.contains("must start with"),
                "{label}: this line is a closer, not a body line missing a marker -- that \
                 advice does not close the string: {error}"
            );
        }
    }

    /// Trailing spaces mean the same thing however the value is spelled.
    ///
    /// Each row is a document with spaces after its last value, paired with the
    /// same document without them. The policy has to answer every spelling the
    /// same way: refused by default, discarded on request, and once discarded the
    /// value is exactly the one the clean document gives.
    ///
    /// The parity is the point. The old test covered only a lone object value, so
    /// nothing noticed that the element loop rejected trailing spaces under *both*
    /// policies -- it never consulted the option at all -- while the entry loop
    /// beside it honoured them.
    ///
    /// Spaces that are data are the other half of the same rule and are pinned in
    /// [`spaces_a_fold_joins_with_are_data`].
    ///
    /// The parity claimed here is about **trailing spaces only**, and it stops
    /// there deliberately. The spellings are not interchangeable in general -- a
    /// bare key may not hold a `"` at all, so a law reading "the quoted form and
    /// the bare form always agree" would demand an equivalence the format does
    /// not have, and would fail on a difference that is the design rather than a
    /// defect. Widen this only with a rule that survives that case.
    #[test]
    fn trailing_spaces_read_the_same_however_the_value_is_spelled() {
        for (dirty, clean) in [
            ("  k:90 \n", "  k:90\n"),
            ("  k:true \n", "  k:true\n"),
            ("  k:\"x\" \n", "  k:\"x\"\n"),
            ("  k: bare \n", "  k: bare\n"),
            ("  k:  1, 2 \n", "  k:  1, 2\n"),
            ("  k:   a   b \n", "  k:   a   b\n"),
        ] {
            assert!(
                parse(dirty, ParseOptions::default()).is_err(),
                "{dirty:?} must be refused while trailing spaces are errors"
            );
            let discarded =
                parse(dirty, ParseOptions::default().trailing_spaces(TrailingSpaces::Discard))
                    .unwrap_or_else(|e| panic!("{dirty:?} must parse once discarded: {e}"));
            let expected = parse(clean, ParseOptions::default())
                .unwrap_or_else(|e| panic!("{clean:?} is supposed to be valid: {e}"));
            assert_eq!(discarded, expected, "{dirty:?} and {clean:?} must agree");
        }
    }

    /// The spaces a fold joins its halves with are data, under either policy.
    ///
    /// The counterweight to [`trailing_spaces_read_the_same_however_the_value_is_spelled`]:
    /// a space at the end of a folded line is not trailing whitespace, it is the
    /// character between the two words, and a fold whose halves are both spaces
    /// is a string of spaces. Discarding those would silently change the data
    /// rather than refuse it, which is the one outcome worth pinning hardest.
    #[test]
    fn spaces_a_fold_joins_with_are_data() {
        for policy in [TrailingSpaces::Reject, TrailingSpaces::Discard] {
            let options = ParseOptions::default().trailing_spaces(policy);
            for (input, expected) in [
                ("  k: hello \n  / world\n", "hello world"),
                ("  k: hello\n  / world\n", "helloworld"),
                ("  k:\"hello \n  / world\"\n", "hello world"),
                ("  k:\"  \n  /   \"\n", "    "),
            ] {
                let value = parse(input, options.clone())
                    .unwrap_or_else(|e| panic!("{input:?} under {policy:?}: {e}"));
                assert_eq!(
                    key_str(&value, "k"),
                    Some(expected),
                    "{input:?} under {policy:?} must keep the spaces it folds with"
                );
            }
        }
    }

    /// The rewrite an error recommends has to be one that parses.
    ///
    /// Each row is a document that fails, paired with the document its message
    /// tells the writer to produce instead. A suggestion that merely trades this
    /// error for the next one is worse than no suggestion, because the writer
    /// spends the edit before finding out.
    ///
    /// The space packed array is the case that caught it: its elements used to be
    /// told to delete the opening space before a `"`, which is right in every
    /// other context and here left a quoted element on a line that admits none.
    /// The same line was separately told about commas it did not contain.
    #[test]
    fn the_rewrite_an_error_recommends_actually_parses() {
        for (broken, fixed) in [
            // "wants three spaces in front of it rather than two"
            ("  tags:   rust   wasm  extra\n", "  tags:   rust   wasm   extra\n"),
            // "give the array one element per line"
            ("  tags:   rust   \"quoted\"\n", "  tags:\n     rust\n    \"quoted\"\n"),
            // "delete that one space before the opening \"" -- still right here
            ("  tags:  1, 2,  \"quoted\"\n", "  tags:  1, 2, \"quoted\"\n"),
            ("  k: \"quoted\"\n", "  k:\"quoted\"\n"),
            // "the literals are lowercase, so write `true`"
            ("  v:TRUE\n", "  v:true\n"),
        ] {
            let error = parse(broken, ParseOptions::default())
                .expect_err(&format!("{broken:?} is supposed to be rejected"));
            assert!(
                parse(fixed, ParseOptions::default()).is_ok(),
                "the rewrite {fixed:?} does not parse, so the advice in this message \
                 costs the writer an edit and leaves them stuck: {error}"
            );
        }
    }

    /// A table cell fault points at the cell, not at the row.
    ///
    /// Written as "the caret lands between this cell's pipes" rather than as an
    /// expected column, because the number is the thing most likely to be edited
    /// to match a regression. Every cell error used to report column 1: the
    /// splitter returned owned strings and dropped the offsets it had just
    /// computed, so there was nothing to point with, and counting cells by eye in
    /// a wide table is exactly the work an error is supposed to save.
    #[test]
    fn a_bad_table_cell_is_pointed_at_rather_than_its_row() {
        // Third column, so a caret at the row's start, at the first cell, or at
        // the last one are all distinguishable failures.
        let row = "    |1|2|,bad|";
        let input = format!("  t:\n    |x|y|z|\n{row}\n");
        let error = parse(&input, ParseOptions::default()).unwrap_err();

        let opens = row.find(",bad").expect("the offending cell is in the row");
        let closes = opens + ",bad".len();
        // `column()` is 1-based and counted in characters; this row is ASCII.
        let column = error.column() - 1;
        assert!(
            (opens..closes).contains(&column),
            "caret at column {column} is outside the offending cell (bytes {opens}..{closes}): {error}"
        );
    }

    #[test]
    fn a_scalar_with_one_space_too_many_is_named_as_such() {
        // `,  92` reads as a bare string beside numbers. The rule it breaks is
        // real, but stating the rule leaves the writer to find which of four
        // elements is wrong; the element and the space are what they need.
        let error = parse("  scores:  90, 85,  92\n", ParseOptions::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("`92`"), "must name the element: {error}");
        assert!(error.contains("a number"), "must say what it would be: {error}");

        for (input, expected) in [
            ("  a:  1, 2,  true\n", "a boolean"),
            ("  a:  1, 2,  null\n", "null"),
        ] {
            let error = parse(input, ParseOptions::default()).unwrap_err().to_string();
            assert!(error.contains(expected), "{input:?}: {error}");
        }

        // A genuine word is a genuine mix, and the rule is the right thing to say.
        let word = parse("  a:  90, 85,  word\n", ParseOptions::default())
            .unwrap_err()
            .to_string();
        assert!(
            word.contains("all bare strings or no bare strings"),
            "a real mix keeps the rule statement: {word}"
        );
    }

    #[test]
    fn table_padding_is_taught_on_the_side_it_belongs() {
        // Header and cell reach the same mistake by different paths: a column is
        // padded on the right, and a leading space means something else in both
        // places -- nothing in "invalid table header key" says so.
        let header = parse(
            "  t:\n    |name    | age  |role    |\n    |a       |1     |b       |\n",
            ParseOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            header.contains("cannot begin with a space") && header.contains("on the right"),
            "the header must name the rule and the side: {header}"
        );

        let cell = parse(
            "  t:\n    |name    |age   |role    |\n    | Alice  |  30   | admin  |\n",
            ParseOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            cell.contains("padded on the right, not the left"),
            "the cell must name the side too: {cell}"
        );
        assert!(
            cell.contains("opening quote"),
            "and why the second space is not padding: {cell}"
        );
    }

    /// The two halves of one rule, checked against each other over the whole
    /// held-back set rather than over the one character that happened to differ.
    ///
    /// A run gives back what it may not end on; the fold lookahead asks whether
    /// what was given back is only that. Written as two lists, they disagreed
    /// about PIPELIKE -- reachable because U+01C0 is a PIPELIKE and a letter, so
    /// it gets into a run and is then stripped out of it -- and a folded bare key
    /// ending in one was refused with `there is no colon on this line`, which was
    /// false: the colon was on the continuation.
    ///
    /// A letter from each set is used, so the test is about the sets and not
    /// about `|` or `,`, neither of which can end a run in the first place.
    #[test]
    fn a_folded_bare_key_reads_the_same_whatever_it_holds_back() {
        // U+01C0 PIPELIKE, U+02BB COMMALIKE, U+02BC QUOTELIKE -- each also a
        // letter, so each reaches the end of a run.
        for held_back in ['\u{01C0}', '\u{02BB}', '\u{02BC}'] {
            let folded = format!("  ab{held_back}\n  / cd: 1\n");
            let value = parse(&folded, ParseOptions::default())
                .unwrap_or_else(|error| panic!("U+{:04X}: {error}", held_back as u32));
            // A fold joins the two fragments with nothing between them, so the
            // held-back character ends up interior rather than trailing -- which
            // is exactly why it stops being held back.
            assert_eq!(
                key_str(&value, &format!("ab{held_back}cd")),
                Some("1"),
                "U+{:04X}: a held-back character is interior once the fold continues it",
                held_back as u32,
            );
            // The rule it is held back by is untouched: ending there is still out.
            let ends_on_it = format!("  ab{held_back}: 1\n");
            let error = parse(&ends_on_it, ParseOptions::default())
                .expect_err("a bare key may not end on a held-back character")
                .to_string();
            assert!(error.contains("cannot end with"), "U+{:04X}: {error}", held_back as u32);
        }
    }

    #[test]
    fn a_line_with_no_colon_and_a_stray_fold_marker_say_so() {
        // Both used to arrive as "invalid value start", which names the text --
        // where nothing is wrong -- instead of the thing that is missing.
        let no_colon = parse("  a 1\n", ParseOptions::default()).unwrap_err().to_string();
        assert!(no_colon.contains("no colon on this line"), "{no_colon}");
        assert!(
            no_colon.contains("`key: value`"),
            "must show the shape of an entry: {no_colon}"
        );

        let stray = parse("  / x\n", ParseOptions::default()).unwrap_err().to_string();
        assert!(
            stray.contains("nothing above this line is left open"),
            "must name the missing thing, not the text: {stray}"
        );

        // The colon test reads the physical line, not the value fragment -- a
        // fragment after a colon has none of its own, and reading that as "no
        // colon" would hijack every value fault on a well-formed entry.
        let value_fault = parse("  a:  x\n", ParseOptions::default()).unwrap_err().to_string();
        assert!(
            value_fault.contains("opening quote"),
            "a value fault on a line that has a colon must survive: {value_fault}"
        );
    }

    /// A comment between a value and the `/ ` line continuing it.
    const COMMENT_IN_FOLD: &str = "  key: aaaaaaaaaa\n  // stray\n  / bbbbbbbbbb\n";

    #[test]
    fn comment_in_fold_rejected_by_default() {
        let error = parse(COMMENT_IN_FOLD, ParseOptions::default()).unwrap_err().to_string();
        assert!(
            error.contains("may not appear inside a fold"),
            "the error must name the rule: {error}"
        );
    }

    #[test]
    fn comment_in_fold_discard_keeps_the_value_and_drops_the_comment() {
        let value = parse(
            COMMENT_IN_FOLD,
            ParseOptions::default().comment_placement_error(CommentPlacementError::Discard),
        )
        .expect("the value survives");
        assert_eq!(
            key_str(&value, "key"),
            Some("aaaaaaaaaabbbbbbbbbb"),
            "the fold still reassembles across the discarded comment"
        );
    }

    #[test]
    fn comment_in_fold_hoist_keeps_both() {
        // Discard loses what someone wrote; Hoist is the reading that does not.
        // A Document keeps comments, so this is where the difference shows.
        let root = Parser::<Node>::parse_document(
            COMMENT_IN_FOLD,
            ParseOptions::default().comment_placement_error(CommentPlacementError::Hoist),
        )
        .expect("the value survives");

        // A hoisted comment goes to the next node created. When the fold is the
        // last thing in the document there is no next node, so it lands among
        // the trailing comments instead -- still kept, which is the whole point
        // of Hoist over Discard, but not necessarily back where it was written.
        let entries = root.entries().expect("an object at the root");
        let kept: Vec<&str> = entries
            .iter()
            .flat_map(|entry| entry.value().comments_before())
            .chain(root.trailing_comments())
            .map(|comment| comment.text())
            .collect();
        assert!(
            kept.iter().any(|text| text.contains("stray")),
            "Hoist must keep the comment, unlike Discard: {kept:?}"
        );
    }

    /// The value stored at a top-level key.
    fn key_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
        let Value::Object(entries) = value else { return None };
        entries.iter().find(|entry| entry.key == key).map(|entry| &entry.value)
    }

    #[test]
    fn a_multi_level_jump_without_a_marker_is_rejected_by_default() {
        // The specification's reading, and the one every existing document was
        // written against: depth below one level is spelled with a marker chain,
        // so indentation alone is an error however far it goes.
        for input in [
            "  m:\n      3\n",
            "  m:\n        3\n",
            "  m:\n      a: x\n",
            "  m:\n       bare\n",
        ] {
            let error = parse(input, ParseOptions::default())
                .expect_err("a jump of more than one level needs a marker chain")
                .to_string();
            assert!(
                error.contains("indent"),
                "the error must be about the indent, not something downstream: {input:?} gave {error}"
            );
        }
    }

    #[test]
    fn one_level_is_unaffected_by_either_reading() {
        // The option is about jumps of more than one level. Ordinary nesting --
        // and the odd column of a bare string, which is its opening quote and
        // not part of the indent -- must read identically under both.
        for input in [
            "  m:\n    3\n",
            "  m:\n     bare\n",
            "  m:\n    a: x\n",
            "  m:\n  [ [ 3\n",
        ] {
            let spec = parse(input, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{input:?} is valid TJSON: {error}"));
            let inferred = parse(
                input,
                ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer),
            )
            .unwrap_or_else(|error| panic!("{input:?} must still be valid: {error}"));
            assert_eq!(spec, inferred, "Infer changed a document it has no business touching: {input:?}");
        }
    }

    #[test]
    fn infer_reads_the_missing_levels_off_the_indentation() {
        // Two extra columns is one extra level, four is two. The levels above
        // the deepest can only be arrays, so the marker chain they stand for is
        // `[ [ 3` and `[ [ [ 3`.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);

        let one = parse("  m:\n      3\n", options).expect("one inferred level");
        assert_eq!(
            key_value(&one, "m"),
            parse("  m:\n  [ [ 3\n", ParseOptions::default())
                .ok()
                .as_ref()
                .and_then(|value| key_value(value, "m")),
            "a jump of two columns must read as the two-marker chain"
        );

        let two = parse("  m:\n        3\n", options).expect("two inferred levels");
        assert_eq!(
            key_value(&two, "m"),
            parse("  m:\n  [ [ [ 3\n", ParseOptions::default())
                .ok()
                .as_ref()
                .and_then(|value| key_value(value, "m")),
            "a jump of four columns must read as the three-marker chain"
        );
    }

    #[test]
    fn infer_lets_only_the_deepest_level_be_an_object() {
        // An object cannot sit directly inside an object -- there is nowhere to
        // write the key -- so the inferred levels are arrays and only the
        // bottom one answers to what is written there.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        let value = parse("  m:\n      a: x\n", options).expect("an object at the bottom");
        assert_eq!(
            key_value(&value, "m"),
            parse("  m:\n  [ { a: x\n", ParseOptions::default())
                .ok()
                .as_ref()
                .and_then(|value| key_value(value, "m")),
            "the deepest level takes the kind its content says, the level above it is an array"
        );

        // Two inferred levels, so two arrays above the object rather than one.
        let deeper = parse("  m:\n        a: x\n", options).expect("an object two levels down");
        assert_eq!(
            key_value(&deeper, "m"),
            parse("  m:\n  [ [ { a: x\n", ParseOptions::default())
                .ok()
                .as_ref()
                .and_then(|value| key_value(value, "m")),
            "every level above the deepest is an array however many there are"
        );
    }

    #[test]
    fn an_array_element_may_step_in_from_the_element_above_it() {
        // One step is a step the reader sees: a line, then a line one in. The
        // marker is not required at one step, and the kind is settled by what
        // is written there. Default reading -- this is legal TJSON, not a
        // relaxation.
        let object = parse("  5\n    key: value\n", ParseOptions::default())
            .expect("an object element written without its marker");
        assert_eq!(object.to_json().replace([' ', '\n'], ""), "[5,{\"key\":\"value\"}]");

        let array = parse("  5\n    1, 2\n", ParseOptions::default())
            .expect("an array element written without its marker");
        assert_eq!(array.to_json().replace([' ', '\n'], ""), "[5,[1,2]]");

        // The whole run at that depth goes inside the one container. A second
        // container beside it would have to say so, and nothing here does.
        let shared = parse("  5\n    a:1\n    b:2\n", ParseOptions::default())
            .expect("two lines at one depth");
        assert_eq!(shared.to_json().replace([' ', '\n'], ""), "[5,{\"a\":1,\"b\":2}]");

        // Writing the marker at the element column says the same thing.
        assert_eq!(object, parse("  5\n  { key: value\n", ParseOptions::default()).unwrap());
        assert_eq!(array, parse("  5\n  [ 1, 2\n", ParseOptions::default()).unwrap());
    }

    #[test]
    fn a_marker_two_columns_over_is_a_different_document() {
        // Location decides, so moving a marker is not a more explicit spelling
        // of the same page -- it is another page. At the element column the
        // object is the element; one step in, the object sits inside the
        // element, and the element is the container holding it.
        let at_element = parse("  5\n  { key: value\n", ParseOptions::default()).unwrap();
        let one_deeper = parse("  5\n    { key: value\n", ParseOptions::default()).unwrap();
        assert_eq!(at_element.to_json().replace([' ', '\n'], ""), "[5,{\"key\":\"value\"}]");
        assert_eq!(one_deeper.to_json().replace([' ', '\n'], ""), "[5,[{\"key\":\"value\"}]]");
        assert_ne!(at_element, one_deeper, "two columns apart is two documents");
    }

    #[test]
    fn a_step_needs_something_to_step_in_from() {
        // With nothing at the element column, the first thing on the page is
        // already deeper than the level it belongs to. No step is visible, so
        // this is a jump, and a jump is spelled with markers.
        for input in ["    1\n", "    key: value\n"] {
            parse(input, ParseOptions::default())
                .expect_err("nothing above it to sit inside");
        }
        // And the multi-level jump stays refused even with an element above,
        // because two steps is not a step the reader can see.
        parse("  5\n      key: value\n", ParseOptions::default())
            .expect_err("two steps needs the chain");
    }

    #[test]
    fn require_forced_refuses_the_unmarked_step() {
        // The reading that accepts only what `force_markers` writes: legal
        // TJSON, but with a level that has no mark on it.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::RequireForced);
        parse("  5\n    key: value\n", options)
            .expect_err("legal, but unmarked");
        assert_eq!(
            parse("  5\n  { key: value\n", ParseOptions::default()).unwrap().to_json(),
            parse("  5\n    key: value\n", ParseOptions::default()).unwrap().to_json(),
            "and the marked spelling it asks for means the same thing"
        );
    }

    #[test]
    fn both_bare_string_openers_read_as_the_same_string() {
        // `_` stands where the opening space would stand. It is the same string
        // written two ways, so the trees must be equal -- including in the
        // packed forms, where every element carries its own opener.
        for (marked, plain) in [
            ("  k:_foo\n", "  k: foo\n"),
            ("  k:  _a  _b\n", "  k:   a   b\n"),
            ("  _a\n  _b\n", "   a\n   b\n"),
            ("  |h  |\n  |_a |\n", "  |h  |\n  | a |\n"),
        ] {
            let left = parse(marked, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{marked:?}: {error}"));
            let right = parse(plain, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{plain:?}: {error}"));
            assert_eq!(left.to_json(), right.to_json(), "{marked:?} must read as {plain:?}");
        }
    }

    #[test]
    fn a_bare_string_may_not_start_with_the_marker_it_would_be_read_as() {
        // Every member of the set, not just the low line: the rule is about what
        // a reader takes for a marker, and that is the whole set by definition.
        for glyph in ['_', '\u{02CD}', '\u{23BC}', '\u{23BD}', '\u{2581}', '\u{FF3F}'] {
            let input = format!("  k: {glyph}foo\n");
            let error = parse(&input, ParseOptions::default())
                .expect_err("a bare string cannot open with a marker lookalike")
                .to_string();
            assert!(error.contains("marker"), "U+{:04X}: {error}", glyph as u32);
        }
        // Quoting it is the way out, and it still means what it says.
        assert_eq!(
            key_str(&parse("  k:\"_foo\"\n", ParseOptions::default()).unwrap(), "k"),
            Some("_foo")
        );
    }

    #[test]
    fn a_bare_string_may_not_start_with_a_solidus_lookalike() {
        // The solidus opens a fold continuation and both indent offset glyphs,
        // so a character shaped like one at the start of a bare string is read
        // as a fold marker by a person even though it is not one to the parser.
        // That is a sharper reason than the other sets have, which is why the
        // whole set is barred and not just `/`.
        for glyph in ['/', '\u{1735}', '\u{2044}', '\u{2215}', '\u{2571}', '\u{29F8}', '\u{FF0F}'] {
            let input = format!("  k: {glyph}foo\n");
            let error = parse(&input, ParseOptions::default())
                .expect_err("a bare string cannot open with a solidus lookalike")
                .to_string();
            assert!(error.contains("fold marker"), "U+{:04X}: {error}", glyph as u32);
        }
    }

    #[test]
    fn the_solidus_and_the_low_line_are_barred_at_the_start_only() {
        // Unlike the pipe and the quote, which a reader meets as edges and which
        // are therefore barred at both ends, these two open things and close
        // nothing. Inside a bare string they sit in running text, where no
        // reader takes them for syntax.
        for input in ["  k: a/b\n", "  k: a_b\n", "  k: ab/\n", "  k: ab_\n"] {
            parse(input, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{input:?} is a legal bare string: {error}"));
        }
    }

    #[test]
    fn the_explicit_inline_array_start_reads_as_the_two_space_one() {
        // Spec, array starters 2 and 3: `[ ` where the `  ` would go is legal
        // and means exactly the same thing. Under the default reading, since
        // this is the specification's own allowance and not a relaxation.
        for (explicit, normal) in [
            ("  k:[ 1, 2\n", "  k:  1, 2\n"),
            ("  k:[  a   b\n", "  k:   a   b\n"),
        ] {
            let read = parse(explicit, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{explicit:?} is legal TJSON: {error}"));
            let plain = parse(normal, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{normal:?}: {error}"));
            assert_eq!(read, plain, "{explicit:?} must read as {normal:?}");
        }
        // `[]` and MINIMAL JSON still take their own paths -- the new strip
        // needs the space and must not have eaten either of them.
        assert!(
            matches!(
                key_value(&parse("  k:[]\n", ParseOptions::default()).unwrap(), "k"),
                Some(Value::Array(elements)) if elements.is_empty()
            ),
            "`k:[]` is still the empty array, not a packed one"
        );
        assert_eq!(
            parse("  k:[1,2]\n", ParseOptions::default()).unwrap(),
            parse("  k:  1, 2\n", ParseOptions::default()).unwrap(),
        );
    }

    #[test]
    fn infer_reads_a_missing_marker_on_the_marker_line_itself() {
        // Every input on the left is a broken document -- the chain is required
        // once a value moves more than one level, and part of it is missing.
        // What is pinned is what stepping over that error yields: the same tree
        // as the correct spelling on the right, because both put their values
        // in the same columns and the columns are what carry the depth.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        for (inferred, spelled) in [
            ("  [   { key: value\n", "  [ [ { key: value\n"),
            ("  [   1\n", "  [ [ 1\n"),
            ("  [     1\n", "  [ [ [ 1\n"),
            ("  [   [ 1\n", "  [ [ [ 1\n"),
        ] {
            let read = parse(inferred, options)
                .unwrap_or_else(|error| panic!("{inferred:?} under Infer: {error}"));
            let written = parse(spelled, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{spelled:?} is valid TJSON: {error}"));
            assert_eq!(read, written, "{inferred:?} must read as {spelled:?}");
        }
    }

    #[test]
    fn an_inferred_slot_takes_its_kind_from_what_follows_it() {
        // Only the deepest level may be an object. When the inferred slot is
        // the deepest it answers to its content; when an explicit marker comes
        // after it, it is interior and can only be an array.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);

        let object = parse("  [   a: x\n", options).expect("a key makes the inferred level an object");
        assert_eq!(object, parse("  [ { a: x\n", ParseOptions::default()).unwrap());

        let array = parse("  [   1\n", options).expect("a scalar leaves it an array");
        assert_eq!(array, parse("  [ [ 1\n", ParseOptions::default()).unwrap());

        // Interior: the `{` after it is explicit, so the inferred level above
        // it is an array no matter that an object sits below.
        let interior = parse("  [   { a: x\n", options).expect("interior inferred level");
        assert_eq!(interior, parse("  [ [ { a: x\n", ParseOptions::default()).unwrap());
    }

    #[test]
    fn a_bare_string_still_gets_its_opening_quote_after_an_inferred_slot() {
        // The leftover space is the bare string's one-sided quote, not half a
        // level. Four spaces is one inferred level plus a quote, so it is the
        // string "1" one level down -- not the number, and not two levels.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        assert_eq!(
            parse("  [    1\n", options).expect("inferred level then a bare string"),
            parse("  [ [  1\n", ParseOptions::default()).expect("the same thing spelled out"),
        );
        // And the spelled-out form really is the string, so the assertion above
        // is pinning the bare reading rather than agreeing with itself.
        assert_eq!(
            parse("  [ [  1\n", ParseOptions::default()).unwrap(),
            parse("  [ [ \"1\"\n", ParseOptions::default()).unwrap(),
        );
    }

    #[test]
    fn siblings_at_one_depth_share_the_inferred_wrappers() {
        // Depth is satisfied by both readings, so the container count decides:
        // `[[[1, 2]]]` is three containers and `[[[1]], [[2]]]` is five. The
        // inferred level is opened once and both lines go inside it.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        let shared = parse("  [   1\n      2\n", options).expect("two lines at one inferred depth");
        assert_eq!(
            shared,
            parse("  [ [ 1\n      2\n", ParseOptions::default()).expect("spelled out"),
            "the second line joins the first's wrapper instead of getting its own"
        );
        assert_eq!(shared.to_json().replace([' ', '\n'], ""), "[[[1,2]]]");
    }

    #[test]
    fn reject_names_the_missing_marker_rather_than_counting_spaces() {
        // The old message came from the `k:` space ladder and talked about
        // keys, packed arrays and bare strings -- none of which is what a run
        // of spaces after a marker means. Default reading, so this is what a
        // user actually meets.
        let error = parse("  [   { key: value\n", ParseOptions::default())
            .expect_err("a missing marker is an error under the specification's reading")
            .to_string();
        assert!(error.contains("nesting marker"), "names the rule it broke: {error}");
        assert!(!error.contains("packed array"), "and not the `k:` ladder: {error}");
    }

    #[test]
    fn a_typed_marker_keeps_the_depth_it_was_written_at() {
        // Inference may add levels; it may never move or drop one the writer
        // typed. A table header one space right of its level used to slide the
        // whole table there, putting a container at a depth nobody wrote.
        let ragged = "  [  |a  |b  |\n     |1  |2  |\n";
        for policy in [MissingIndentMarker::Reject, MissingIndentMarker::Infer] {
            let error = parse(ragged, ParseOptions::default().missing_indent_marker(policy))
                .expect_err("one space cannot open a pipe")
                .to_string();
            assert!(error.contains("bare string"), "explains the space under {policy:?}: {error}");
        }
        // Two spaces is a level, and at that level the table is fine again.
        let deeper = "  [   |a  |b  |\n      |1  |2  |\n";
        let inferred = parse(
            deeper,
            ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer),
        )
        .expect("two spaces is a marker, and the table sits below it");
        assert_eq!(inferred, parse("  [ [ |a  |b  |\n      |1  |2  |\n", ParseOptions::default()).unwrap());
    }

    /// The same one-line string in each of the three fence forms.
    const ONE_LINE_FENCES: [(&str, &str); 3] = [
        ("transparent", "  doc: ```\nhello\n   ```\n"),
        ("bold",        "  doc: ``\n| hello\n   ``\n"),
        ("minimal",     "  doc: `\n    hello\n   `\n"),
    ];

    #[test]
    fn a_multiline_with_no_linefeed_parses_by_default() {
        // Spec: "the actual string data being displayed SHOULD contain at least
        // one linefeed and is REQUIRED to contain at least one data character",
        // and among the requirements, "MULTILINE STRINGS SHOULD contain at
        // least one real unescaped newline, but MUST contain at least one
        // character in order to parse." A SHOULD is not a parse error, so the
        // default reading takes all three forms.
        assert_eq!(
            ParseOptions::default().multiline_minimum,
            MultilineMinimum::Character,
            "the specification's floor is one character, so it is the default"
        );
        for (form, input) in ONE_LINE_FENCES {
            let value = parse(input, ParseOptions::default())
                .unwrap_or_else(|error| panic!("{form} is TJSON and must parse: {error}"));
            assert_eq!(
                key_str(&value, "doc"),
                Some("hello"),
                "{form} must yield the content with no trailing newline"
            );
        }
    }

    #[test]
    fn eol_refuses_a_fence_with_no_linefeed_in_it() {
        // The strict reading promotes the SHOULD to a refusal, for the caller
        // who would rather hear that a fence holds one unbroken line.
        let options = ParseOptions::default().multiline_minimum(MultilineMinimum::Eol);
        for (form, input) in ONE_LINE_FENCES {
            let error = parse(input, options)
                .expect_err("Eol refuses a fence with no linefeed")
                .to_string();
            assert!(
                error.contains("linefeed"),
                "{form} must be refused for the reason it is refused: {error}"
            );
        }
    }

    #[test]
    fn a_multiline_holding_one_character_that_is_an_eol_is_valid() {
        // The floor is one character and an EOL is a character, so data of
        // exactly "\n" clears it -- under both readings, since it is also a
        // data EOL. It takes two body lines to write, which is what separates
        // it from the empty fence one body line makes.
        for (form, input) in [
            ("transparent", "  doc: ```\n\n\n   ```\n"),
            ("bold", "  doc: ``\n| \n| \n   ``\n"),
            ("minimal", "  doc: `\n    \n    \n   `\n"),
        ] {
            for policy in [MultilineMinimum::Eol, MultilineMinimum::Character] {
                let value = parse(input, ParseOptions::default().multiline_minimum(policy))
                    .unwrap_or_else(|error| panic!("{form} under {policy:?}: {error}"));
                assert_eq!(key_str(&value, "doc"), Some("\n"), "{form} under {policy:?}");
            }
        }
    }

    #[test]
    fn an_empty_fence_is_rejected_under_every_reading() {
        // `""` is the only way to write the empty string and stays the only
        // way, so this is not what `Character` relaxes. Both shapes that could
        // produce one are covered: no body line at all, and a single body line
        // that is itself empty.
        for input in ["  doc: ```\n   ```\n", "  doc: ```\n\n   ```\n"] {
            for policy in [MultilineMinimum::Eol, MultilineMinimum::Character] {
                let error = parse(input, ParseOptions::default().multiline_minimum(policy))
                    .expect_err("an empty multiline is never a way to write \"\"")
                    .to_string();
                assert!(
                    error.contains("\"\""),
                    "the error must point at the spelling that does work, under {policy:?}: {error}"
                );
            }
        }
    }

    #[test]
    fn a_real_multiline_is_unaffected_by_either_reading() {
        let input = "  doc: ```\nhello\nthere\n   ```\n";
        let spec = parse(input, ParseOptions::default()).expect("valid under the spec reading");
        let relaxed = parse(
            input,
            ParseOptions::default().multiline_minimum(MultilineMinimum::Character),
        )
        .expect("and still valid when the floor is lowered");
        assert_eq!(spec, relaxed, "Character must not change a document Eol already accepted");
        assert_eq!(key_str(&spec, "doc"), Some("hello\nthere"));
    }

    #[test]
    fn infer_does_not_read_a_colon_inside_a_bare_string_as_a_key() {
        // The colon that makes an object is the one terminating a key, which
        // starts at the structural column. A bare string starts one column
        // right of it -- that column is its opening quote -- so every colon
        // inside it is content. `10:30:00` is the case the rule exists for.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        let value = parse("  m:\n       10:30:00\n", options).expect("a bare string at the bottom");
        let Some(Value::Array(outer)) = key_value(&value, "m") else {
            panic!("the inferred level is an array: {value:?}")
        };
        let [Value::Array(inner)] = outer.as_slice() else {
            panic!("one inferred level holding one array: {outer:?}")
        };
        assert_eq!(
            inner.as_slice(),
            [Value::String("10:30:00".to_owned())],
            "the colons are content, so this is one bare string and not an object"
        );
    }

    #[test]
    fn infer_still_rejects_a_document_that_is_simply_misaligned() {
        // Inference supplies levels; it does not forgive a line that belongs
        // nowhere. Here the second element dedents to a level no container is
        // open at, which is a fault about the document rather than about a
        // missing marker.
        let options = ParseOptions::default().missing_indent_marker(MissingIndentMarker::Infer);
        assert!(
            parse("  m:\n      3\n    4\n", options).is_err(),
            "a line at an indent no open container sits at must stay an error"
        );
    }
}



/// Properties that must hold across every document the corpus has, checked by
/// rendering it rather than by reasoning about it.
///
/// `force_markers` and [`MissingIndentMarker::RequireForced`] are two spellings
/// of one fact -- "every level that is a level carries a marker" -- one on the
/// writing side and one on the reading side. Kept honest against each other
/// rather than each against a hand-maintained list, because a list is where the
/// two would quietly drift apart.
///
/// Lives in the crate rather than `tests/` because `RequireForced` is
/// `pub(crate)`: it is a lever for shaping the parser, not a public reading.
#[cfg(test)]
mod corpus_invariants {
    use super::*;
    use crate::options::{MissingIndentMarker, ParseOptions, RenderOptions};
    use crate::value::Value;
    use std::path::{Path, PathBuf};

    /// The corpus, resolved exactly as `tests/file_tests.rs` resolves it.
    /// `None` when it is absent, so a checkout without the test subrepository
    /// skips rather than fails -- this test is about agreement between two
    /// options, and has nothing to say when there are no documents to try.
    fn corpus() -> Option<PathBuf> {
        let base = match std::env::var("TJSON_TESTS_DIR") {
            Ok(dir) => PathBuf::from(dir),
            Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        };
        base.is_dir().then_some(base)
    }

    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "tjson") {
                out.push(path);
            }
        }
    }

    /// The bare string marker replaces the space that opens a bare string; it
    /// does not sit beside it. So marking changes which character occupies a
    /// column and never how many there are -- every line keeps its width, and
    /// with it every alignment the renderer computed. A marker that made a line
    /// wider would push table columns and packed gaps out of true, which is the
    /// one thing it must not do, so this is checked on real documents rather
    /// than trusted to the one-byte-each argument.
    #[test]
    fn marking_a_bare_string_never_changes_a_line_width() {
        let Some(base) = corpus() else { return };
        let mut files = Vec::new();
        collect(&base.join("parse/valid"), &mut files);
        collect(&base.join("roundtrip"), &mut files);

        let plain = RenderOptions::default();
        let marked = RenderOptions::default().bare_strings(crate::options::StringStyle::Marked);
        let mut checked = 0usize;

        for file in &files {
            let Ok(source) = std::fs::read_to_string(file) else { continue };
            let Ok(value) = Parser::<Value>::parse_document(&source, ParseOptions::default()) else {
                continue;
            };
            let a = value.to_tjson_with(plain.clone());
            let b = value.to_tjson_with(marked.clone());
            checked += 1;
            assert_eq!(
                a.lines().count(),
                b.lines().count(),
                "{}: marking changed the line count",
                file.display()
            );
            for (n, (left, right)) in a.lines().zip(b.lines()).enumerate() {
                assert_eq!(
                    left.chars().count(),
                    right.chars().count(),
                    "{} line {}: marking changed the width\n  plain : {left:?}\n  marked: {right:?}",
                    file.display(),
                    n + 1
                );
            }
        }
        assert!(checked > 0, "no document in {base:?} parsed under the default reading");
    }

    #[test]
    fn everything_force_markers_writes_is_readable_under_require_forced() {
        let Some(base) = corpus() else { return };
        let mut files = Vec::new();
        collect(&base.join("parse/valid"), &mut files);
        collect(&base.join("roundtrip"), &mut files);
        assert!(!files.is_empty(), "corpus at {base:?} held no .tjson documents");

        let forced = RenderOptions::default().force_markers(true);
        let require = ParseOptions::default().missing_indent_marker(MissingIndentMarker::RequireForced);
        let mut failures = Vec::new();
        let mut checked = 0usize;

        for file in &files {
            let Ok(source) = std::fs::read_to_string(file) else { continue };
            // Documents this corpus keeps for other reasons need not parse under
            // the default reading; only the ones that do are ours to check.
            let Ok(value) = Parser::<Value>::parse_document(&source, ParseOptions::default()) else {
                continue;
            };
            let rendered = value.to_tjson_with(forced.clone());
            checked += 1;
            match Parser::<Value>::parse_document(&rendered, require) {
                Ok(reparsed) => assert_eq!(
                    reparsed,
                    value,
                    "{}: forcing markers changed the data\n{rendered}",
                    file.display()
                ),
                Err(error) => failures.push(format!(
                    "{}\n  rendered:\n{}\n  error: {error}",
                    file.display(),
                    rendered
                )),
            }
        }

        assert!(checked > 0, "no document in {base:?} parsed under the default reading");
        assert!(
            failures.is_empty(),
            "{} of {checked} documents came back unreadable under RequireForced.\n\
             Either force_markers left a level unmarked, or RequireForced is asking for a \
             marker somewhere the specification exempts -- the two must be settled together.\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}
