//! Positions on a line, and the frames they can be in.
//!
//! # Space carries the meaning; markers draw attention to it
//!
//! The principle the rest of this module is an expression of. Depth, type and
//! structure are held by *where things are*. A marker does not carry any of
//! that -- it points at space that already meant what it means, which is why
//! omitting one changes nothing about how a document reads.
//!
//! The mechanical consequence, and the reason this module has the shape it has,
//! is that a marker occupies the very space it annotates. It cannot insert a
//! column, because a column it inserted would be one the space did not already
//! have, and then the marker would be carrying meaning instead of pointing at
//! it:
//!
//! | marker | replaces |
//! |---|---|
//! | `[ ` `{ ` beginning a line | two columns of the indent |
//! | `[ ` after a key's colon | the two spaces of the inline array starter |
//! | `/ ` | the last two columns of the indent |
//! | `| ` | the multiline body's indent |
//! | `_` | a bare string's one-column opening space |
//!
//! That is why every [`Marker`] is exactly [`LEVEL`] wide, why an [`Opener`] is
//! one column, and why writing a marker or omitting it produces the same layout
//! and the same reading.
//!
//! There are exceptions -- [`Glyph::IndentOpen`] and its close deliberately shift
//! the frame, which is the one construct here that changes what space means
//! rather than pointing at it, and it is why [`FileIndent`] and [`LogicalIndent`]
//! have to be separate types at all. Everything else annotates.
//!
//! `tests/fuzz.rs` checks one instance of this as `overlay_invariance` -- that
//! `_` and a plain space put the text in the same place. The general law, that
//! *no* marker moves anything, is the same law at a larger scale and is not
//! tested yet.
//!
//! Four things here share the shape of "a count" and none of them are
//! interchangeable. Two are structural positions:
//!
//! - A [`FileIndent`] is **where a value's structure sits in the physical file**.
//!   Always even.
//! - A [`LogicalIndent`] is that same position with any active ` /<` offset
//!   applied, which is what decides parent and child. Also always even.
//!
//! The reason for the split is that a line's leading run of spaces is not the
//! same thing as its indent. Two constructs begin with a space that is part of
//! the *value*, not part of the indentation: a bare string's one-sided opening
//! quote, and the space that begins a ` /<` glyph. Counting leading spaces
//! therefore yields a number that sometimes means "where structure sits" and
//! sometimes means "where structure sits, plus one column of content", with the
//! difference recoverable only from its parity.
//!
//! [`Leading`] is the one place that decomposition happens. Everything
//! downstream gets an indent that is only ever an indent, and an [`Opener`] that
//! says what the extra column was, so no later code has to read a low bit to
//! find out.
//!
//! The other two are where a position sits *along* a line, and they are a
//! different pair entirely:
//!
//! - A [`ByteOffset`] is an index into the line's bytes. What slicing uses.
//! - A [`Column`] is what a human is told: the 1-based place their caret should
//!   land.
//!
//! Adding one to a byte offset and calling the result a column is only correct
//! while the line is ASCII, which is the assumption behind every caret that has
//! ever pointed at the wrong character.
//!
//! So none of these types can reach another by arithmetic. What relates a count
//! of characters to an index into bytes is the text itself, and [`Line`] is the
//! type that holds it — which is why every crossing is a method there and why
//! each one takes the line. [`Line::column_at`] goes bytes to characters,
//! [`Line::byte_offset_of`] goes back, and what a column *is* is decided in
//! those two places and nowhere else.
//!
//! Emission is the exception, and it belongs on the indent rather than the line:
//! [`FileIndent::spaces`] needs no text because there is none yet — it is making
//! it, and a column of indentation is written as one space by definition.

use std::ops::{Index, Range, RangeFrom, RangeFull, RangeTo};

use crate::util::count_leading_spaces;

/// One nesting level, in columns. Fixed by the specification: the indent step is
/// always two, which is what lets a reader see a one-level discrepancy by eye.
pub(crate) const LEVEL: usize = 2;

/// Where a value's structure sits in the physical file, in columns.
///
/// Always even: structure advances by [`LEVEL`] and starts at an even origin.
/// An odd leading-space count means the last space belongs to the value, not to
/// the indentation — see [`Leading`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct FileIndent(usize);

impl FileIndent {
    /// The left margin.
    pub(crate) const ROOT: Self = Self(0);

    /// A structural position that arrived as a plain number.
    ///
    /// For the renderer, which computes where it is about to write rather than
    /// reading a position off a line. Reading is [`Leading::of`]'s job and stays
    /// there; this is the writing side, where the number is one the caller just
    /// worked out and is about to spend on spaces.
    pub(crate) fn new(columns: usize) -> Self {
        debug_assert!(columns % LEVEL == 0, "structural indent is always even, got {columns}");
        Self(columns)
    }

    /// `levels` levels further out, saturating at the margin.
    pub(crate) fn shallower(self, levels: usize) -> Self {
        Self(self.0.saturating_sub(levels * LEVEL))
    }

    /// The rest of `line` after this indent.
    ///
    /// For lines the renderer wrote at this indent: it spells an indent as ASCII
    /// spaces, so the byte index and the column agree -- but only for that reason,
    /// and only until something writes an indent some other way. Going through
    /// [`Columns::spent_in`] means the agreement is established rather than
    /// assumed, and a caller cannot slice a line by a structural position at all.
    pub(crate) fn strip(self, line: &str) -> Option<&str> {
        line.get(self.width().spent_in(line)..)
    }

    /// How wide this indent is, for budget arithmetic against a width.
    ///
    /// An indent is columns spent before the content starts, so a budget that has
    /// to account for it asks here rather than reaching for the number.
    pub(crate) fn width(self) -> Columns {
        Columns(self.0)
    }

    /// This position seen in the structural frame, `shift` columns to the right.
    ///
    /// The two frames differ only by the active ` /<` shift, so crossing between
    /// them is one addition — but one that nobody outside this pair may perform,
    /// which is why neither type will hand out its column count. Reaching a byte
    /// position is a different kind of crossing entirely and needs the line: see
    /// [`Line::byte_offset_of`].
    pub(crate) fn shifted_right(self, shift: usize) -> LogicalIndent {
        LogicalIndent(self.0 + shift)
    }

    /// The spaces this indent is written as.
    ///
    /// The counterpart of [`Line::byte_offset_of`], and the reason the two live
    /// on different types: reading an indent needs the line, because the text
    /// already exists and its characters may be any width. Writing one does not,
    /// because there is no text yet and a column of indentation *is* one space.
    /// Emission is the easy direction, so it stays with the indent.
    ///
    /// Yields a [`Spaces`] rather than a count, so no number escapes that a
    /// caller could use as a byte position by mistake.
    pub(crate) fn spaces(self) -> Spaces {
        Spaces(self.0)
    }

    /// `levels` levels further in, in the same frame.
    pub(crate) fn deeper(self, levels: usize) -> Self {
        Self(self.0 + levels * LEVEL)
    }
}

/// A run of indentation, ready to be written.
///
/// Carries a count but does not expose one: it exists so [`FileIndent::spaces`]
/// can hand back something that goes straight into a `format!` or a `push_str`
/// without a number escaping on the way. Writing it costs no allocation — the
/// formatter's own padding does the work, where `" ".repeat(n)` would build a
/// buffer solely to copy it into the next one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Spaces(usize);

impl Spaces {
    /// A run of `count` spaces.
    ///
    /// For code that holds a column count as a bare number and has not yet been
    /// given a [`FileIndent`] to ask instead — the renderer, mostly. Prefer
    /// [`FileIndent::spaces`] wherever an indent is in hand.
    pub(crate) fn new(count: usize) -> Self {
        Self(count)
    }
}

impl std::fmt::Display for Spaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:width$}", "", width = self.0)
    }
}

/// A structural position with any active ` /<` offset applied: the frame in
/// which parent and child are decided. Always even, for the same reason
/// [`FileIndent`] is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct LogicalIndent(usize);

impl LogicalIndent {
    /// The outermost structural level: the left margin.
    pub(crate) const ROOT: Self = Self(0);

    pub(crate) fn new(columns: usize) -> Self {
        debug_assert!(columns % LEVEL == 0, "structural indent is always even, got {columns}");
        Self(columns)
    }

    /// Saturating inverse of [`FileIndent::shifted_right`].
    ///
    /// Saturates because a logical position shallower than the active shift has
    /// no file column of its own; callers read that as the left margin. An
    /// underflow here would wrap to an enormous indent, so the clamp is a
    /// decision made in the open rather than arithmetic nobody watched.
    pub(crate) fn shifted_left(self, shift: usize) -> FileIndent {
        FileIndent(self.0.saturating_sub(shift))
    }

    /// This position used as the shift for frames nested inside it.
    ///
    /// A ` /<` glyph shifts everything below it by its own logical column, so a
    /// position becomes a distance exactly here and nowhere else.
    pub(crate) fn as_shift(self) -> usize {
        self.0
    }

    /// `levels` levels further in, in the same frame.
    pub(crate) fn deeper(self, levels: usize) -> Self {
        Self(self.0 + levels * LEVEL)
    }

    /// `levels` levels further out, in the same frame.
    ///
    /// Saturates at the left margin: nothing sits outside the document, so a
    /// caller asking for the parent of the outermost level is asking for the
    /// outermost level. An underflow here would wrap to an enormous indent that
    /// no line could ever match, which reads downstream as "this construct is
    /// nowhere" rather than as the arithmetic mistake it is.
    pub(crate) fn shallower(self, levels: usize) -> Self {
        Self(self.0.saturating_sub(levels * LEVEL))
    }

    /// How many levels `self` sits below `shallower`, saturating at zero.
    ///
    /// Exact rather than rounding: both operands are even, so the gap is a whole
    /// number of levels and there is no remainder to discard.
    pub(crate) fn levels_below(self, shallower: Self) -> usize {
        self.0.saturating_sub(shallower.0) / LEVEL
    }
}

/// What the extra, non-structural leading space on a line belongs to.
///
/// Both variants are one column wide and are the first character of the value
/// rather than part of the indentation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Opener {
    /// The leading run was entirely indentation.
    None,
    /// A bare string's one-sided opening quote.
    BareString,
    /// The space that begins a ` /<` or ` />` indent glyph.
    Glyph,
}

impl Opener {
    /// Width this opener occupies, which the indent does not account for.
    ///
    /// Use this rather than a literal wherever code steps over an opener: the
    /// width and the step are one fact, and writing the step as a number is how
    /// they come apart.
    pub(crate) fn width(self) -> usize {
        match self {
            Self::None => 0,
            Self::BareString | Self::Glyph => 1,
        }
    }

    pub(crate) fn is_present(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// A marker written at a column no marker may begin at.
///
/// A marker stands in the indent it points at, so it begins where an indent
/// level ends and therefore at an even column. A line whose leading run is odd
/// has spent its last space on an [`Opener`] — a column of the *value* — and a
/// marker cannot follow one, because a marker is not part of any value.
///
/// This is what [`Leading::of`] has to say instead of guessing, and the reason
/// it is fallible. Deciding the run's parity and then choosing an opener
/// without checking what follows is how a `/ ` one column right of its pairing
/// column was read as "indent, then a bare string's opening quote", which put
/// the marker at exactly the column the caller went on to look for it at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct OffColumnMarker {
    /// Which marker it is, so the message can name it rather than describe it.
    pub(crate) marker: Marker,
    /// Where the marker begins — one column right of where the indent ended.
    pub(crate) at: ByteOffset,
}

/// How a line's leading run of spaces decomposes.
///
/// The invariant that makes this worth a struct: `indent` and `opener` together
/// account for the whole run, so nothing measured is silently dropped and
/// nothing has to be recovered from parity later.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Leading {
    pub(crate) indent: FileIndent,
    pub(crate) opener: Opener,
}

impl Leading {
    /// Markers whose column this checks.
    ///
    /// *Every* marker sits in the indent and so at an even column, which means
    /// any of them following an odd run proves the extra space is not an opener.
    /// Only [`Marker::Fold`] is acted on here, because it is the only one that
    /// reaches this function at a wrong column:
    ///
    /// - [`Marker::Body`] belongs to a multiline body, whose lines are verbatim
    ///   text read by [`crate::parse::Parser::parse_triple_backtick_body`] and its
    ///   neighbours. None of them measure a [`Leading`], so a `| ` never arrives.
    ///   Listing it would catch only a *table* row's leading `|`, which is a cell
    ///   edge and not this marker at all — see [`Marker`] — and would name it
    ///   wrongly in the message. The table walk asks [`Self::unopened`] instead.
    /// - [`Marker::CHAIN`] is held out because a BARE STRING may legally open
    ///   with `[` or `{` today: [`crate::util::check_bare_string`] bars `/`, `_`,
    ///   `|`, `"` and `,` at the start of one, and not the brackets. Refusing
    ///   `   [ 3` would change what the language accepts rather than close a hole
    ///   in it.
    ///
    /// Whether the brackets *should* be refused is a specification question,
    /// recorded as G4 in `local/fuzzer-found-breakage.md`. Adding
    /// [`Marker::CHAIN`] here is the whole change on this side if it is ever
    /// settled that way, which is why this is a list and not one hand-written
    /// test.
    const OFF_COLUMN_CHECKED: [Marker; 1] = [Marker::Fold];

    /// The only place a measured column becomes a structural indent.
    ///
    /// An odd count means its last space is the value's opening column, because
    /// structure is always even. Which kind is settled by what follows: a bare
    /// string can never start with `/`, so `/<` after an odd run is
    /// unambiguously a glyph.
    ///
    /// That same rule is why this is fallible rather than total. "A bare string
    /// can never start with `/`" is not only how a glyph is recognised, it is a
    /// fact about the odd run itself: if what follows cannot open a value, then
    /// the last space did not open one either and the run does not decompose at
    /// all. Answering [`Opener::BareString`] there is a claim contradicted by
    /// [`crate::util::check_bare_string`] four hundred lines later, and it is
    /// what let a marker one column right of its column be found exactly where
    /// it was expected. Pinned by `a_marker_one_column_right_is_not_an_opener`.
    ///
    /// Spaces are ASCII, so the run's byte length is also its column count and
    /// also its character count. That coincidence is confined to this function:
    /// what leaves is a [`FileIndent`] and an [`Opener`], and reaching a byte
    /// position from either takes [`Line::byte_offset_of`] like anything else.
    pub(crate) fn of(line: Line<'_>) -> Result<Self, OffColumnMarker> {
        let run = count_leading_spaces(line.text());
        if run % LEVEL == 0 {
            return Ok(Self { indent: FileIndent(run), opener: Opener::None });
        }
        let rest = &line[ByteOffset(run)..];
        if rest == Glyph::IndentOpen.body() || rest == Glyph::IndentClose.body() {
            return Ok(Self { indent: FileIndent(run - 1), opener: Opener::Glyph });
        }
        if let Some(marker) = Self::OFF_COLUMN_CHECKED.into_iter().find(|marker| marker.opens(rest))
        {
            return Err(OffColumnMarker { marker, at: ByteOffset(run) });
        }
        Ok(Self { indent: FileIndent(run - 1), opener: Opener::BareString })
    }

    /// Where the value starts, opener included. Slicing a line here keeps a bare
    /// string's opening quote attached to the string it opens.
    ///
    /// Takes the line because the answer is a position in bytes and this is a
    /// count of columns; nothing converts the one to the other without the text.
    pub(crate) fn content_start(self, line: Line<'_>) -> ByteOffset {
        line.byte_at(self.indent.0)
    }

    /// Where the value's first non-opener character sits.
    ///
    /// Steps over the opener in *characters*, not bytes — the opener is one
    /// column wide and its width is [`Opener::width`], so a wider opening
    /// character would move this without any other change.
    ///
    /// **This is a position for carets and spans, not a place to look for
    /// structure.** Stepping over the opener spends a column the value owns, so
    /// a caller that reads a key, a marker or a table row here is reading one
    /// column right of where any of them may begin — and will find it, because
    /// what it finds is the value's own text shifted into the structural
    /// position. Ask [`Self::unopened`] instead, which cannot answer at all on a
    /// line that opened a value.
    pub(crate) fn text_start(self, line: Line<'_>) -> ByteOffset {
        line.byte_at(self.indent.0 + self.opener.width())
    }

    /// The line's content when the whole leading run was indentation.
    ///
    /// `None` when a column went to an [`Opener`], which is the entire point:
    /// structure begins where indentation ends, so on a line that opened a value
    /// there is no structure to find and the honest answer is that the question
    /// does not apply. Callers looking for a key, a marker, a table row or a
    /// container start ask here, and the `None` arm is where they say what a
    /// value sitting at that column means to them.
    ///
    /// Borrows the input rather than the `Line`, since the answer is usually
    /// handed onwards — see [`Line::from`].
    pub(crate) fn unopened<'a>(self, line: Line<'a>) -> Option<&'a str> {
        match self.opener {
            Opener::None => Some(line.from(self.content_start(line))),
            Opener::BareString | Opener::Glyph => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_marker_fills_exactly_one_indent_slot() {
        for marker in Marker::ALL {
            assert_eq!(
                marker.width(),
                LEVEL,
                "{marker:?} must occupy one indent slot, or a level stops reading the \
                 same with and without it",
            );
        }
    }

    #[test]
    fn an_even_run_is_all_indent() {
        let line = Line::new("    alpha: 1");
        let leading = line.leading().expect("an even run always decomposes");
        assert_eq!(leading.indent, FileIndent(4));
        assert_eq!(leading.opener, Opener::None);
        assert_eq!(leading.content_start(line), ByteOffset(4));
        assert_eq!(leading.text_start(line), ByteOffset(4));
        assert_eq!(leading.unopened(line), Some("alpha: 1"));
    }

    #[test]
    fn an_odd_run_ends_in_a_bare_string_quote() {
        // Two levels of structure, then the one-sided opening quote.
        let line = Line::new("     alpha");
        let leading = line.leading().expect("a bare string may open here");
        assert_eq!(leading.indent, FileIndent(4));
        assert_eq!(leading.opener, Opener::BareString);
        assert_eq!(leading.content_start(line), ByteOffset(4));
        assert_eq!(leading.text_start(line), ByteOffset(5));
        // The whole reason the accessor exists: the text at `text_start` reads
        // as structure and is not any, so there is nothing to hand back.
        assert_eq!(leading.unopened(line), None);
    }

    #[test]
    fn an_odd_run_before_a_glyph_is_a_glyph_opener() {
        let leading = Line::new("     /<").leading().expect("a glyph opens with its own space");
        assert_eq!(leading.indent, FileIndent(4));
        assert_eq!(leading.opener, Opener::Glyph);
    }

    /// A marker's column is the only thing pairing it to what it points at, so a
    /// marker one column off pairs with nothing and the run it follows is not an
    /// opener. Named by [`Leading::of`].
    #[test]
    fn a_marker_one_column_right_is_not_an_opener() {
        for text in ["   / tail", " / tail", "     / tail"] {
            let fault = Line::new(text)
                .leading()
                .expect_err("a fold marker cannot follow a bare string's opening quote");
            assert_eq!(fault.marker, Marker::Fold, "{text:?}");
            assert_eq!(fault.at, ByteOffset(count_leading_spaces(text)), "{text:?}");
        }
        // The even column is the whole difference, and it still decomposes.
        for text in ["  / tail", "/ tail", "    / tail"] {
            let leading = Line::new(text).leading().expect("an even run always decomposes");
            assert_eq!(leading.opener, Opener::None, "{text:?}");
        }
        // A solidus that is not the marker's spelling stays a bare string, and
        // is refused later as one — `check_bare_string` bars a leading `/`, and
        // says so in the language of bare strings rather than of columns.
        let leading = Line::new(" /notamarker").leading().expect("not a marker spelling");
        assert_eq!(leading.opener, Opener::BareString);
        // Held out, and deliberately. `[` and `{` because a bare string may open
        // with them today (G4 in `local/fuzzer-found-breakage.md`); `|` because
        // the only one reaching here is a table's cell edge, which this marker is
        // not. See `Leading::OFF_COLUMN_CHECKED`.
        for text in ["   [ 3", "   { 3", "   | Alice |"] {
            let leading = Line::new(text).leading().expect("held out of the column check");
            assert_eq!(leading.opener, Opener::BareString, "{text:?}");
        }
    }

    #[test]
    fn the_parts_account_for_the_whole_run() {
        for text in ["x", " x", "  x", "   x", "    x", "     /<", "   /<"] {
            let line = Line::new(text);
            let leading = line.leading().expect("none of these carry an off-column marker");
            assert_eq!(
                leading.text_start(line),
                ByteOffset(count_leading_spaces(text)),
                "indent plus opener must account for the whole leading run of {text:?}",
            );
        }
    }

    /// The property the file is built on: a column count and a byte index are
    /// the same number only while the line is ASCII, and crossing between them
    /// happens in one place that knows the difference.
    ///
    /// `字` is three bytes and one column, so an indent of two columns lands on
    /// byte 6 here and byte 2 in the ASCII case. Code that added the indent to a
    /// byte position — which is what [`FileIndent`] used to offer — would slice
    /// this line in the middle of a character.
    #[test]
    fn crossing_to_bytes_counts_characters_not_bytes() {
        let ascii = Line::new("  ab");
        assert_eq!(ascii.byte_offset_of(FileIndent(2)), Some(ByteOffset(2)));

        let wide = Line::new("字字ab");
        assert_eq!(wide.byte_offset_of(FileIndent(2)), Some(ByteOffset(6)));
        assert_eq!(&wide[wide.byte_offset_of(FileIndent(2)).unwrap()..], "ab");
    }

    /// The inverse crossing, and the reason a caret needs the line.
    #[test]
    fn a_column_counts_characters_and_never_splits_one() {
        let line = Line::new("字字ab");
        assert_eq!(line.column_at(ByteOffset(0)), Column::FIRST);
        assert_eq!(line.column_at(ByteOffset(6)), Column(3));
        // Inside the second character: report where that character starts
        // rather than a column no reader can see.
        assert_eq!(line.column_at(ByteOffset(4)), Column(2));
        // Past the end clamps to one past the last character.
        assert_eq!(line.column_at(ByteOffset(999)), Column(5));
    }

    /// A line with fewer characters than an indent claims does not reach it, and
    /// says so rather than answering with its own length.
    ///
    /// The distinction this protects: `"  "` clamped to the end would compare
    /// equal to its own leading-space count, so a two-space line would report as
    /// sitting at indent four. Callers that ask a question get `None`; callers
    /// that want the empty tail say so themselves.
    #[test]
    fn a_line_that_does_not_reach_an_indent_has_no_position_there() {
        let line = Line::new("  ");
        assert_eq!(line.byte_offset_of(FileIndent(4)), None);
        // Reaching it exactly is a different fact, and does have a position.
        assert_eq!(line.byte_offset_of(FileIndent(2)), Some(line.end()));
    }

    #[test]
    fn levels_are_exact_because_both_ends_are_even() {
        let outer = LogicalIndent::new(2);
        let inner = LogicalIndent::new(8);
        assert_eq!(inner.levels_below(outer), 3);
        assert_eq!(outer.levels_below(inner), 0);
    }
}

/// A width or a budget along a line, in columns.
///
/// The other half of the pair [`Column`] belongs to: that one is *where* a
/// position is, this one is *how much room* there is. Neither is a byte length,
/// and a byte length is the thing they are most often confused with -- a row of
/// CJK is three times as long in bytes as in columns, so a budget weighed
/// against `str::len` folds it at a third of the intended width.
///
/// Arithmetic is defined here so budget code never has to reach for the number:
/// subtraction saturates, because a line already wider than its budget has no
/// room left rather than a negative amount of it, and an underflow would wrap to
/// a budget nothing could exceed.
///
/// **What a column is, this type does not decide** -- [`columns_of`] does, and
/// today it counts characters. That is a lie for CJK, which occupies two
/// terminal cells per character, and it is a deliberate one: the door left open
/// for a `VisibleWidth` is [`Self::pad_width`], the one place a count escapes to
/// pad text out. Nothing else needs the number.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub(crate) struct Columns(usize);

impl Columns {
    pub(crate) const ZERO: Self = Self(0);

    /// How much room `text` takes.
    pub(crate) fn of(text: &str) -> Self {
        Self(columns_of(text))
    }

    /// A width that arrived as a plain number: a configured wrap width, a
    /// literal, a count of something whose columns are known by construction.
    pub(crate) fn new(count: usize) -> Self {
        Self(count)
    }

    /// [`Self::new`] in a `const` context.
    pub(crate) const fn new_const(count: usize) -> Self {
        Self(count)
    }

    /// This budget with `other` spent out of it, or nothing left.
    pub(crate) fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// This width as a plain count of columns.
    ///
    /// Not a seam like [`Self::pad`] and [`Self::spent_in`] -- it answers no
    /// question about text. It is a stopgap for the parts of the renderer whose
    /// *indents* are still bare `usize`, where a width has to become one to be
    /// used as a position. When those carry a type, this goes.
    pub(crate) fn columns(self) -> usize {
        self.0
    }

    /// Where this budget runs out in `text`, as an index into its bytes.
    ///
    /// Takes the text for the same reason [`Line::byte_offset_of`] takes the
    /// line: a count of columns and an index into bytes are different units, and
    /// nothing relates them except the text itself.
    pub(crate) fn spent_in(self, text: &str) -> usize {
        text.char_indices().nth(self.0).map_or(text.len(), |(at, _)| at)
    }

    /// This width written as a run of spaces.
    ///
    /// The counterpart of [`FileIndent::spaces`], for a width that is not a
    /// structural indent: the leading run a continuation lines up under, the gap
    /// a packed array starts after. Hands back a [`Spaces`] for the same reason —
    /// it goes straight into a `format!` without a number escaping, and without
    /// building a buffer solely to copy it into the next one.
    pub(crate) fn spaces(self) -> Spaces {
        Spaces(self.0)
    }

    /// `text` padded out to this width, ready to be written.
    ///
    /// Mirrors [`FileIndent::spaces`]: it hands back something that goes straight
    /// into a `format!` rather than a number a caller could use as a byte length
    /// by mistake, and the formatter's own padding does the work.
    pub(crate) fn pad(self, text: &str) -> Padded<'_> {
        Padded { text, width: self.0 }
    }
}

/// Text padded out to a width. See [`Columns::pad`].
///
/// **This and [`Columns::spent_in`] are the marked entry points.** They are the
/// only two places a column count meets actual text -- one measuring text out,
/// one measuring a budget along it -- so they are what a `VisibleWidth` would
/// have to change. Every other use of a width goes through the arithmetic on
/// [`Columns`] and never sees a number at all. Rust's fill-and-align counts
/// characters, which is what [`columns_of`] counts, so the two agree today by
/// construction rather than by coincidence.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Padded<'a> {
    text: &'a str,
    width: usize,
}

impl std::fmt::Display for Padded<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:<width$}", self.text, width = self.width)
    }
}

impl std::ops::Add for Columns {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }
}

/// How many columns `text` occupies on a line.
///
/// Private, and reached only through [`Columns::of`]: a bare count is the untyped
/// quantity this module exists to keep out of circulation, so what a column *is*
/// can be answered here without the answer escaping as a number.
///
/// The counterpart to [`Column::at`], which gives a *position*; this gives a
/// *width*. Both answer "what is a column" and must answer it the same way, so
/// the definition lives here -- change the counting and every budget, every
/// margin check and every caret move together.
///
/// A byte length is not this. They coincide only for ASCII, and using one where
/// the other belongs makes a twelve-column CJK key look thirty-six columns wide,
/// which the renderer then leaves as wasted line.
fn columns_of(text: &str) -> usize {
    text.chars().count()
}

/// A byte offset into the whole input, as opposed to a position within one line.
///
/// The two are different things and mixing them is how a caret ends up pointing
/// at a character in a different line. Subtracting two of these yields a
/// [`ByteOffset`] -- a position inside the line the later one falls in -- and
/// only in that direction, because an offset earlier than the line said to
/// contain it is not a small number, it is a caller that has paired an offset
/// with the wrong line. [`Self::within`] returns `None` there rather than
/// clamping, so the impossibility is a case somebody handles instead of a zero
/// nobody sees.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct DocumentOffset(usize);

impl DocumentOffset {
    pub(crate) fn new(bytes: usize) -> Self {
        Self(bytes)
    }

    pub(crate) fn bytes(self) -> usize {
        self.0
    }

    /// `n` bytes further into the input.
    pub(crate) fn plus(self, bytes: usize) -> Self {
        Self(self.0 + bytes)
    }

    /// Where this offset falls within the line starting at `line_start`.
    ///
    /// `None` when it falls before that line, which is never a position -- it
    /// means the offset and the line do not belong together.
    pub(crate) fn within(self, line_start: Self) -> Option<ByteOffset> {
        self.0.checked_sub(line_start.0).map(ByteOffset)
    }
}

/// An index into a line's bytes. What slicing uses, and what `str` lengths are.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct ByteOffset(usize);

impl ByteOffset {
    /// The start of the line.
    pub(crate) const START: Self = Self(0);

    /// For a value that is already a position in the line's bytes: a scan
    /// result, a slice length, an index returned by a search.
    ///
    /// Not a way to convert from another frame. A structural position reaches a
    /// byte offset through [`FileIndent::byte_offset`], which cannot be got to
    /// from a [`LogicalIndent`] without the tracker.
    pub(crate) fn new(bytes: usize) -> Self {
        Self(bytes)
    }

    /// `n` bytes further along the same line.
    pub(crate) fn plus(self, bytes: usize) -> Self {
        Self(self.0 + bytes)
    }

    pub(crate) fn bytes(self) -> usize {
        self.0
    }
}

/// A 1-based column, as reported to a person: where their caret should land.
///
/// Reachable from a [`ByteOffset`] only through [`Column::at`], which needs the
/// line, so no caller can produce one by arithmetic on an offset alone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Column(usize);

impl Column {
    /// The first column. 1-based, so this is 1.
    pub(crate) const FIRST: Self = Self(1);

    /// The column an offset falls at when the line is not available.
    ///
    /// Only for errors that have no source line to point into — end of input,
    /// and failures raised before any line was read. Counts bytes, which is all
    /// there is to count without the text.
    pub(crate) fn at_unknown_line(offset: ByteOffset) -> Self {
        Self(offset.bytes() + 1)
    }

    /// The column a structural indent reads as, for a message that names one.
    ///
    /// Indents are counted from zero and columns from one; doing that by hand at
    /// a call site is how a message comes to name a column nobody can find. Takes
    /// a [`FileIndent`] rather than any integer, because a *logical* indent is
    /// not a column on the page at all when a ` /<` is active.
    pub(crate) fn of_indent(indent: FileIndent) -> Self {
        Self(indent.0 + 1)
    }

    /// A column already known in 1-based terms — a literal position in a message,
    /// or one derived from another column rather than from an offset.
    pub(crate) fn one_based(column: usize) -> Self {
        debug_assert!(column >= 1, "columns are 1-based, got {column}");
        Self(column)
    }

    pub(crate) fn number(self) -> usize {
        self.0
    }
}

/// One physical line of the input, and the only place the frames may be crossed.
///
/// The types above deliberately cannot reach one another: a [`Column`] counts
/// characters, a [`ByteOffset`] indexes bytes, a [`FileIndent`] counts columns of
/// structure, and no arithmetic relates them. What relates them is the text — so
/// the type that holds the text is where every crossing lives.
/// [`Self::column_at`] and [`Self::byte_offset_of`] are the whole set, and each
/// needs the line because neither question has an answer without it.
///
/// That is also what keeps the bytes-are-characters assumption in one place.
/// Indentation is spaces today, so a column of it is one byte, and code that
/// added a column count to a byte index was right by accident. Here the
/// conversion counts characters, so the assumption is not written down at all and
/// admitting a non-ASCII indent character later changes nothing outside this
/// type.
///
/// Slicing goes through [`Index`], implemented for ranges of [`ByteOffset`] and
/// nothing else: `&line[a..b]` reads as it would on a `&str`, and a [`Column`] or
/// a bare `usize` is a compile error. That is the whole difference between this
/// and the `&str` it wraps.
///
/// **Never implement [`Deref`](std::ops::Deref) for this type.** It would hand
/// over every `str` method for free and, with them, `&line[0..2]` by auto-deref —
/// restoring the untyped slice silently, with no diagnostic anywhere to notice
/// it. [`Self::text`] is the one way out, and it is a name a reader can grep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Line<'a> {
    text: &'a str,
}

impl<'a> Line<'a> {
    /// Wrap one line's content. The line ending is not part of it.
    pub(crate) fn new(text: &'a str) -> Self {
        debug_assert!(
            !text.contains('\n'),
            "a Line is one physical line; {text:?} spans more than one",
        );
        Self { text }
    }

    /// The underlying text, for the many `str` operations that are frame-neutral
    /// — `starts_with`, `trim_end`, `contains`. The one escape from the type, and
    /// named so that looking for places the guarantee is set aside is one grep.
    pub(crate) fn text(self) -> &'a str {
        self.text
    }

    /// One past the last byte: where a slice running to the end of the line stops.
    pub(crate) fn end(self) -> ByteOffset {
        ByteOffset(self.text.len())
    }

    pub(crate) fn is_empty(self) -> bool {
        self.text.is_empty()
    }

    /// The column a byte offset in *this* line falls at.
    ///
    /// **This decides what a column is.** The single seam: change the counting
    /// here and every caret in the program moves together.
    ///
    /// Parsing works in bytes; a reader counts characters, and so does the caret
    /// an error prints. The two agree only for ASCII, so the conversion happens
    /// here rather than at callers who would each have to remember. An offset
    /// past the end of the line, or inside a character, resolves to the start of
    /// the character it lands in — a caret one glyph early beats a caret in the
    /// middle of a codepoint.
    pub(crate) fn column_at(self, at: ByteOffset) -> Column {
        let mut byte = at.0.min(self.text.len());
        while byte > 0 && !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        Column(self.text[..byte].chars().count() + 1)
    }

    /// Where a structural indent ends in this line's bytes.
    ///
    /// The mirror of [`Self::column_at`]: that one goes bytes to characters, this
    /// one characters to bytes. An indent is a count of columns, so this counts
    /// that many characters and reports where the next one starts; a line with
    /// fewer characters than the indent claims has no such position and reports
    /// the end of the line, which is what slicing there yields anyway.
    ///
    /// Takes a [`FileIndent`] and not a [`LogicalIndent`] because only the file
    /// frame names columns that exist on the page — under an active ` /<` a
    /// logical indent names a column no line has.
    ///
    /// `None` when the line has fewer characters than the indent claims. That is
    /// not a small number and not the end of the line: it is a line that does not
    /// reach the indent at all, which is a different fact from reaching it and
    /// having nothing after it. Same reasoning as [`DocumentOffset::within`] —
    /// clamping here would hand back a position the line does not have, and every
    /// caller that then compared against it would be comparing against the length
    /// by accident.
    pub(crate) fn byte_offset_of(self, indent: FileIndent) -> Option<ByteOffset> {
        self.positions().nth(indent.0)
    }

    /// Every position in this line, in column order: one before each character,
    /// then one past the last.
    ///
    /// A line of `n` characters has `n + 1` positions, not `n` — the end is a
    /// place a slice may start, and a line whose text stops exactly at an indent
    /// has still reached it. Counting only the characters loses that last one and
    /// makes "ends here" indistinguishable from "never got here", which is the
    /// distinction [`Self::byte_offset_of`] exists to report.
    fn positions(self) -> impl Iterator<Item = ByteOffset> {
        self.text
            .char_indices()
            .map(|(byte, _)| ByteOffset(byte))
            .chain(std::iter::once(self.end()))
    }

    /// Where the character `columns` columns into this line begins.
    ///
    /// Total, unlike [`Self::byte_offset_of`], and only safely so because its
    /// callers are [`Leading`]'s, whose column counts were measured on *this*
    /// line and therefore always name a position it has. The end-of-line fallback
    /// is unreachable for them; it exists so the function has an answer at all.
    ///
    /// Private: a bare column count is exactly the untyped quantity the rest of
    /// the file exists to prevent, so it may only be produced by a neighbour that
    /// knows what its number means.
    fn byte_at(self, columns: usize) -> ByteOffset {
        self.positions().nth(columns).unwrap_or_else(|| self.end())
    }

    /// How this line's leading run of spaces decomposes. See [`Leading`].
    pub(crate) fn leading(self) -> Result<Leading, OffColumnMarker> {
        Leading::of(self)
    }

    /// The rest of the line from `at`, borrowed from the *input* rather than
    /// from this `Line`.
    ///
    /// `&line[at..]` is the same slice and is what to reach for. The difference
    /// is lifetime, not bytes: [`Index`] is defined as `index(&self)`, so the
    /// slice it returns borrows the `Line` value it came from and cannot outlive
    /// it — which is fine in a function body and fails the moment a caller has to
    /// hand the slice onwards. This is that escape and its only reason to exist.
    pub(crate) fn from(self, at: ByteOffset) -> &'a str {
        &self.text[at.0..]
    }
}

impl Index<Range<ByteOffset>> for Line<'_> {
    type Output = str;

    fn index(&self, at: Range<ByteOffset>) -> &str {
        &self.text[at.start.0..at.end.0]
    }
}

impl Index<RangeFrom<ByteOffset>> for Line<'_> {
    type Output = str;

    fn index(&self, at: RangeFrom<ByteOffset>) -> &str {
        &self.text[at.start.0..]
    }
}

impl Index<RangeTo<ByteOffset>> for Line<'_> {
    type Output = str;

    fn index(&self, at: RangeTo<ByteOffset>) -> &str {
        &self.text[..at.end.0]
    }
}

impl Index<RangeFull> for Line<'_> {
    type Output = str;

    fn index(&self, _: RangeFull) -> &str {
        self.text
    }
}

/// A glyph whose leading space belongs to it rather than to the indentation.
///
/// TJSON has two constructs that begin with a space: a bare string, whose space
/// is its one-sided opening quote, and these. [`Leading`] already treats both
/// that way when reading. This is the writing half — without it the space is
/// spelled by hand at every emission site, and a renderer that writes it one way
/// while a parser expects another loses the document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Glyph {
    /// ` /<` — shifts the indent frame right.
    IndentOpen,
    /// ` />` — closes the shift.
    IndentClose,
    /// `` ` `` — multiline string, plain flavour.
    MultilineSingle,
    /// ` ``` — multiline string, bold flavour.
    MultilineDouble,
    /// ` ``` ` — multiline string, fenced flavour.
    MultilineTriple,
}

impl Glyph {
    /// The glyph including its leading space, which is its first character.
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::IndentOpen => " /<",
            Self::IndentClose => " />",
            Self::MultilineSingle => " `",
            Self::MultilineDouble => " ``",
            Self::MultilineTriple => " ```",
        }
    }

    /// Written at a structural position: the leading space lands on `indent`,
    /// so the glyph's own text begins one column right of it. That offset is
    /// here and nowhere else.
    pub(crate) fn at(self, indent: Spaces) -> String {
        format!("{indent}{}", self.text())
    }

    /// The token without its opener.
    pub(crate) fn body(self) -> &'static str {
        self.text().trim_start_matches(' ')
    }

    /// Split `content` into the bytes this glyph's opener occupies and the bytes
    /// remaining, when `content` begins with the glyph.
    ///
    /// Nothing here states how wide an opener is: it is whatever separates the
    /// token from its body, which is a fact about how the glyph is spelled and
    /// stays true if that ever changes.
    pub(crate) fn split_opener(self, content: &str) -> Option<(usize, usize)> {
        let lead = self.text().len() - self.body().len();
        content.starts_with(self.text()).then(|| (lead, content.len() - lead))
    }

    /// Written at a structural position with an explicit end-of-line suffix,
    /// which only the multiline openers carry.
    pub(crate) fn at_with_suffix(self, indent: Spaces, suffix: &str) -> String {
        format!("{indent}{}{suffix}", self.text())
    }
}

/// An indent of `indent` columns whose last level is written as `marker` rather
/// than as the two spaces the marker points at.
///
/// The choice is either-or, never additive: a level's slot is spelled one way or
/// the other and the result is the same width and the same bytes, since both
/// spellings are ASCII. Written as one call because the alternative -- subtract a
/// level, emit spaces, add the token back -- is two operations that have to agree
/// about a number, and callers holding the shallower indent is how they stop
/// agreeing.
///
/// Panics in debug if `indent` has no level to spell, which would mean a marker
/// with nothing to point at.
pub(crate) fn indent_marked(indent: FileIndent, marker: Marker) -> String {
    debug_assert!(
        indent >= FileIndent::new(marker.width()),
        "a marker points at a level that exists; {indent:?} has no room for {marker:?}",
    );
    // A marker is exactly one level wide, so the spaces before it are this indent
    // one level out -- which is the same fact the type already knows, rather than
    // a subtraction repeated here. `Spaces` also writes them without the buffer
    // `" ".repeat` would build only to copy.
    format!("{}{}", indent.shallower(1).spaces(), marker.text())
}

/// A marker whose trailing space is part of it, not padding after it.
///
/// The mirror of [`Glyph`], whose space leads instead. In both cases the space is
/// not decoration around the token, it *is* the token's own character.
///
/// Each is exactly [`LEVEL`] wide, and that is the point rather than a
/// coincidence: **at the start of a line** a marker stands in the room one
/// nesting level occupies, so a level reads the same whether or not anyone wrote
/// it. The invariant is checked in the tests below, so the spelling and the
/// indent step cannot drift apart.
///
/// All of that is about a marker at the start of a line, which is what the
/// specification means by "sits inside the indent". [`ARRAY_STARTER`] describes
/// the one other place these two characters may appear, which is not the same
/// thing and does not generalise.
///
/// A table's `|` is *not* one of these. It carries no space, separates cells
/// rather than opening a level, and is not [`LEVEL`] wide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Marker {
    /// `[ ` — opens one array level.
    Array,
    /// `{ ` — opens one object level.
    Object,
    /// `/ ` — continues a folded value on the next line.
    Fold,
    /// `| ` — begins a body line of a bold multiline string.
    Body,
}

impl Marker {
    /// Markers that open a nesting level, in the order a chain may use them.
    ///
    /// Enumerated so that code asking "does a marker chain start here" cannot
    /// silently miss one that gets added later.
    pub(crate) const CHAIN: [Self; 2] = [Self::Array, Self::Object];

    /// The marker including its trailing space, which is part of it.
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Array => "[ ",
            Self::Object => "{ ",
            Self::Fold => "/ ",
            Self::Body => "| ",
        }
    }

    /// Width the marker occupies -- one indent slot. Derived from the
    /// spelling, never quoted.
    pub(crate) fn width(self) -> usize {
        self.text().len()
    }

    /// Every marker. Exists so the one-indent-slot invariant below is checked
    /// against all of them by construction: a variant added without a width of
    /// [`LEVEL`] fails the test rather than passing unnoticed.
    #[cfg(test)]
    pub(crate) const ALL: [Self; 4] = [Self::Array, Self::Object, Self::Fold, Self::Body];

    /// Strip this marker from the front of `content`, if it is there.
    pub(crate) fn strip(self, content: &str) -> Option<&str> {
        content.strip_prefix(self.text())
    }

    /// Does `content` begin with this marker?
    pub(crate) fn opens(self, content: &str) -> bool {
        content.starts_with(self.text())
    }
}

/// The inline array starter: two spaces after a colon open a packed array.
///
/// `key:  9` and `key:[ 9` are the same document. The two spaces *are* the
/// marker -- an implicit one -- and [`Marker::Array`] written in their place is
/// the same two character slots with the writer highlighting them. The parser
/// accepts either; the generator writes only the spaces today, the specification
/// calling the explicit form ugly, though it may come to write it later.
///
/// **This does not generalise, and nothing should be built on it.** The explicit
/// form is legal only immediately after a key's colon, exactly once, never
/// chained -- the parser admits it solely in an object-value context, so
/// `key:  1, [ 2` and `key:[ [ 9` are both refused.
///
/// It is also the one place a marker's column says nothing. Every other marker
/// sits in the indent and therefore at an even column; this one begins wherever
/// the key ended, so in `  ab:[ 9` it starts at column 5. Depth here comes from
/// the line's indent and the count of markers, never from where the marker sits.
pub(crate) const ARRAY_STARTER: &str = "  ";
