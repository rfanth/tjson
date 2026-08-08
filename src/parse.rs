use std::marker::PhantomData;

use serde_json::Value as JsonValue;

use crate::number::Number;

use crate::error::ParseError;
use crate::tree::{NodeRef,
    ContainerFacts, EntryFacts, KeyForm, MultilineFlavor, RawComment, ScalarFacts, Span,
    BareForm, StringFacts, StringForm, Tree,
};
use crate::options::{
    ByteOrderMark, CommentPlacementError, MissingIndentMarker, MultilineMinimum, ParseOptions,
    TrailingSpaces,
};
use crate::util::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArrayLineValueContext {
    ArrayLine,
    ObjectValue,
    SingleValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContainerKind {
    Array,
    Object,
}

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
}

/// If `content` looks like an attempted key -- there is a colon on the line --
/// return the first bare key rule its text breaks.
///
/// The split is taken at the real separator, an ASCII colon, not at the first
/// colonlike character -- otherwise `ab\u{02D0}cd:1` would be measured as the
/// key `ab`, which is perfectly valid, and the colonlike that actually caused
/// the rejection would never be named.
fn attempted_bare_key_fault(content: &str, forms: &ParseOptions) -> Option<BareKeyFault> {
    let end = content.find(':')?;
    bare_key_fault(&content[..end], forms)
}

impl MultilineLocalEol {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }

    pub(crate) fn opener_suffix(self) -> &'static str {
        match self {
            Self::Lf => "",
            Self::CrLf => "\\r\\n",
        }
    }
}


pub(crate) struct IndentFrame {
    /// Amount added to raw file indents to get logical (structural) indents.
    offset: usize,
    /// Raw file column where the matching ` />` close glyph must appear.
    close_file_indent: usize,
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

    /// Convert a raw file indent to the logical (structural) indent.
    fn logical(&self, file_indent: usize) -> usize {
        file_indent + self.offset()
    }

    /// Push a glyph context.  `glyph_file_indent` is the raw column of the ` /<` line.
    fn push_glyph(&mut self, glyph_file_indent: usize) {
        self.stack.push(IndentFrame {
            offset: glyph_file_indent + self.offset(),
            close_file_indent: glyph_file_indent,
        });
    }

    /// If `line` is the close glyph ` />` for the current context, pop and return true.
    fn try_pop_close(&mut self, line: &str) -> bool {
        if let Some(f) = self.stack.last()
            && line.len() == f.close_file_indent + 3
            && line[..f.close_file_indent].bytes().all(|b| b == b' ')
            && &line[f.close_file_indent..] == " />"
        {
            self.stack.pop();
            return true;
        }
        false
    }
}

pub(crate) struct Parser<'a, T: Tree> {
    input: &'a str,
    line_offsets: Vec<LineSpan>,
    line: usize,
    /// The caller's reading of the format. Consulted rather than copied apart:
    /// the lookalike sets it carries are what `is_*_like` here means.
    options: ParseOptions,
    idt: IndentTracker,
    /// Comment lines seen but not yet attached to a node. Only populated when
    /// `T::KEEPS_COMMENTS`; drained at the next node-creating site, so a comment
    /// always attaches to the next structural thing after it.
    pending_comments: Vec<RawComment>,
    target: PhantomData<T>,
}

pub(crate) struct LineSpan {
    /// Byte offset of the first character of the line in the original input.
    start: usize,
    /// Byte length of the line content, excluding any line-ending bytes (`\r\n` or `\n`).
    len: usize,
}

pub(crate) fn scan_lines(input: &str) -> std::result::Result<Vec<LineSpan>, ParseError> {
    let mut offsets = Vec::new();
    let mut pos = 0usize;
    for (line_index, raw) in input.split('\n').enumerate() {
        let len = if raw.ends_with('\r') { raw.len() - 1 } else { raw.len() };
        let content = &raw[..len];
        for (col, ch) in content.chars().enumerate() {
            if is_forbidden_literal_tjson_char(ch) {
                return Err(ParseError::new(
                    line_index + 1,
                    col + 1,
                    format!("forbidden character U+{:04X} must be escaped", ch as u32),
                    None,
                ));
            }
        }
        offsets.push(LineSpan { start: pos, len });
        pos += raw.len() + 1; // +1 for the '\n'
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
            return Err(ParseError::new(1, 1, "input larger than 4 GiB is not supported", None));
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
                    1,
                    "this input opens with a byte order mark (U+FEFF), which TJSON has no \
                     place for. It is invisible, so the file looks identical to one that \
                     loads -- save it as UTF-8 without a BOM, which most editors offer as \
                     an encoding choice",
                    None,
                ));
            }
            (None, _) => input,
        };

        let mut parser = Self {
            input,
            line_offsets: scan_lines(input)?,
            line: 0,
            options,
            idt: IndentTracker::new(),
            pending_comments: Vec::new(),
            target: PhantomData,
        };
        parser.skip_ignorable_lines()?;
        if parser.line >= parser.line_offsets.len() {
            return Err(ParseError::new(1, 1, "empty input", None));
        }
        let root_pending = parser.take_pending_comments();
        let mut value = parser.parse_root_value()?;
        if T::KEEPS_COMMENTS && !root_pending.is_empty() {
            T::attach_comments_before(&mut value, root_pending, options.start_indent);
        }
        parser.skip_ignorable_lines()?;
        if T::KEEPS_COMMENTS {
            let trailing = parser.take_pending_comments();
            if !trailing.is_empty() {
                T::attach_trailing_comments(&mut value, trailing);
            }
        }
        if parser.line < parser.line_offsets.len() {
            let current = parser.current_line().unwrap_or("").trim_start();
            let msg = if current.starts_with("/>") {
                "unexpected /> indent offset glyph: no previous matching /< indent offset glyph"
            } else if current.starts_with("/ ") {
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
                 or it is a second value and needs a document of its own"
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
            Some(line) => Span::new(line.start, line.len),
            None => Span::default(),
        }
    }

    fn current_span(&self) -> Span {
        self.line_span(self.line)
    }

    /// Span of `len` bytes at byte column `col` of the current line; the whole current
    /// line when the caller lost column tracking (`col == None`).
    fn span_at(&self, col: Option<usize>, len: usize) -> Span {
        match (col, self.line_offsets.get(self.line)) {
            (Some(col), Some(line)) if col <= line.len => {
                Span::new(line.start + col, len.min(line.len - col))
            }
            _ => self.current_span(),
        }
    }

    fn scalar_facts_at(&self, col: Option<usize>, len: usize) -> ScalarFacts {
        ScalarFacts { span: self.span_at(col, len) }
    }

    fn string_facts_at(&self, form: StringForm, col: Option<usize>, len: usize) -> StringFacts {
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
            .ok_or_else(|| ParseError::new(1, 1, "empty input", None))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let file_indent = count_leading_spaces(&line);
        let indent = self.idt.logical(file_indent);
        let content = &line[file_indent..];

        if indent == self.options.start_indent && starts_with_marker_chain(content) {
            return self.parse_marker_chain_line(content, indent);
        }

        // Standalone root-level start glyph: ` /<` at structural indent start_indent+2.
        // Structural indent is always even; file_indent is structural+1 (the glyph's leading space).
        let root_glyph_struct = (self.options.start_indent + 2).saturating_sub(self.idt.offset());
        if file_indent == root_glyph_struct + 1 && content == "/<" {
            self.idt.push_glyph(root_glyph_struct);
            self.line += 1;
            self.skip_ignorable_lines()?;
            return self.parse_root_value();
        }

        if indent <= self.options.start_indent + 1 {
            return self
                .parse_standalone_scalar_line(&line[self.options.start_indent..], self.options.start_indent);
        }

        if indent >= self.options.start_indent + 2 {
            let child_file_pos = (self.options.start_indent + 2).saturating_sub(self.idt.offset());
            let child_content = &line[child_file_pos..];
            if self.looks_like_object_start(child_content, self.options.start_indent + 2) {
                return self.parse_implicit_object(self.options.start_indent);
            }
            return self.parse_implicit_array(self.options.start_indent);
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
             change the value or move anything deeper"
        ))
    }

    fn parse_implicit_object(
        &mut self,
        parent_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        if self.options.missing_indent_marker == MissingIndentMarker::RequireForced {
            return Err(self.require_marker_error("{ "));
        }
        // Implicit containers have no opener token; their span is the line their first
        // entry starts on, captured before parsing moves past it.
        let open_span = self.current_span();
        let mut entries = Vec::new();
        self.parse_object_tail(parent_indent + 2, &mut entries)?;
        if entries.is_empty() {
            return Err(self.error_current("expected at least one object entry"));
        }
        Ok(T::new_object(entries, self.container_facts_from(open_span)))
    }

    fn parse_implicit_array(
        &mut self,
        parent_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        if self.options.missing_indent_marker == MissingIndentMarker::RequireForced {
            return Err(self.require_marker_error("[ "));
        }
        self.skip_ignorable_lines()?;
        let elem_indent = parent_indent + 2;
        let line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected array contents"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let file_indent = count_leading_spaces(&line);
        let indent = self.idt.logical(file_indent);
        if indent < elem_indent {
            return Err(self.error_current("expected array elements indented by two spaces"));
        }
        let content = &line[file_indent..];
        if content.starts_with('|') {
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
        elem_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        let header_line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected a table header"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let header_file_indent = elem_indent.saturating_sub(self.idt.offset());
        let header = &header_line[header_file_indent..];
        let header_span = self.current_span();
        let columns = self.parse_table_header(header, elem_indent)?;
        self.line += 1;
        let mut rows = Vec::new();
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            if self.idt.try_pop_close(&line) {
                self.line += 1;
                continue;
            }
            self.ensure_line_has_no_tabs(self.line)?;
            let file_indent = count_leading_spaces(&line);
            let indent = self.idt.logical(file_indent);
            if indent < elem_indent {
                break;
            }
            if indent != elem_indent {
                return Err(self.error_current("expected a table row at the array indent"));
            }
            let row = &line[file_indent..];
            if !row.starts_with('|') {
                return Err(self.error_current("table arrays may only contain table rows"));
            }
            // Collect fold continuation lines: `/ ` marker at pair_indent (elem_indent - 2),
            // two characters to the left of the opening `|` per spec.
            // Blank lines and `//` comments between a partial row and its continuation are
            // skipped. A parser would also be within its rights to reject them.
            let pair_indent = elem_indent.saturating_sub(2);
            let mut row_owned = row.to_owned();
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
                    let trimmed = peek.trim_start_matches(' ');
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
                    let next_file_indent = count_leading_spaces(next_line);
                    let next_indent = self.idt.logical(next_file_indent);
                    if next_indent != pair_indent {
                        break;
                    }
                    let next_content = &next_line[next_file_indent..];
                    if !next_content.starts_with("/ ") {
                        break;
                    }
                    next_content[2..].to_owned()
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
            }
            let pending = self.take_pending_comments();
            let mut parsed_row = self.parse_table_row(&columns, &row_owned, elem_indent)?;
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

    fn parse_table_header(&self, row: &str, indent: usize) -> std::result::Result<Vec<(String, KeyForm)>, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_line(self.line, indent + 1, "invalid table header"))?;
        if cells.first().is_some_and(String::is_empty) {
            cells.remove(0);
        }
        if !cells.last().is_some_and(String::is_empty) {
            return Err(self.error_at_line(self.line, indent + row.len() + 1, "table header must end with \"  |\" (two spaces of padding then pipe)"));
        }
        cells.pop();
        if cells.is_empty() {
            return Err(self.error_at_line(self.line, 1, "table headers must list columns"));
        }
        let mut col = indent + 2; // skip leading |
        cells
            .into_iter()
            .map(|cell| {
                let cell_col = col;
                col += cell.len() + 1; // +1 for the | separator
                self.parse_table_header_key(cell.trim_end(), cell_col)
            })
            .collect()
    }

    fn parse_table_header_key(&self, cell: &str, col: usize) -> std::result::Result<(String, KeyForm), ParseError> {
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
        if let Some(fault) = bare_key_fault(cell, &self.options) {
            return Err(self.error_at_line(
                self.line,
                col,
                format!("invalid table header key: {}", fault.describe()),
            ));
        }
        Err(self.error_at_line(self.line, col, "invalid table header key"))
    }

    fn parse_table_row(
        &self,
        columns: &[(String, KeyForm)],
        row: &str,
        indent: usize,
    ) -> std::result::Result<T, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_line(self.line, indent + 1, "invalid table row"))?;
        if cells.first().is_some_and(String::is_empty) {
            cells.remove(0);
        }
        if !cells.last().is_some_and(String::is_empty) {
            return Err(self.error_at_line(self.line, indent + row.len() + 1, "table row must end with \"  |\" (two spaces of padding then pipe)"));
        }
        cells.pop();
        if cells.len() != columns.len() {
            return Err(self.error_at_line(
                self.line,
                indent + row.len() + 1,
                "table row has wrong number of cells",
            ));
        }
        let mut entries = Vec::new();
        for (index, (key, key_form)) in columns.iter().enumerate() {
            let cell = cells[index].trim_end();
            if cell.is_empty() {
                continue;
            }
            let value = self.parse_table_cell_value(cell)?;
            entries.push(T::new_entry(key.clone(), value, self.entry_facts(*key_form)));
        }
        Ok(T::new_object(entries, self.container_facts()))
    }

    fn parse_table_cell_value(&self, cell: &str) -> std::result::Result<T, ParseError> {
        if cell.is_empty() {
            return Err(self.error_at_line(
                self.line,
                1,
                "empty table cells mean the key is absent",
            ));
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
                return Err(self.error_at_line(
                    self.line,
                    1,
                    format!(
                        "a table cell is padded on the right, not the left. The first \
                         space after the `|` is the bare string's opening quote, so the \
                         second one starts a second value -- write `|{padded}|` rather \
                         than `|{cell}|`, or `|{content}` with no space at all for a \
                         number or a boolean"
                    ),
                ));
            }
            if let Some(fault) = bare_string_fault(value, &self.options) {
                return Err(self.error_at_line(
                    self.line,
                    1,
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
        Err(self.error_at_line(self.line, 1, "invalid table cell value"))
    }

    fn parse_object_tail(
        &mut self,
        pair_indent: usize,
        entries: &mut Vec<T::Entry>,
    ) -> std::result::Result<(), ParseError> {
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            // Close glyph: pop offset and continue so the loop re-evaluates indent.
            if self.idt.try_pop_close(&line) {
                self.line += 1;
                continue;
            }
            let file_indent = count_leading_spaces(&line);
            let indent = self.idt.logical(file_indent);
            if indent < pair_indent {
                break;
            }
            if indent != pair_indent {
                let content = line[file_indent..].to_owned();
                let msg = if content.starts_with("/>") {
                    format!("misplaced /> indent offset glyph: found at column {}, expected at column {}", indent + 1, pair_indent + 1)
                } else if content.starts_with("/ ") {
                    format!("misplaced fold marker: found at column {}, expected at column {}", indent + 1, pair_indent + 1)
                } else {
                    "expected an object entry at this indent".to_owned()
                };
                return Err(self.error_current(msg));
            }
            let content = &line[file_indent..];
            if content.is_empty() {
                return Err(self.error_current("blank lines are not valid inside objects"));
            }
            // Comments preceding this line attach to the line's first entry; comments
            // captured while parsing nested values drain at deeper sites.
            let pending = self.take_pending_comments();
            let mut line_entries =
                self.parse_object_line_content(content, pair_indent, Some(file_indent))?;
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
        pair_indent: usize,
        col0: Option<usize>,
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
            if self.line != key_line {
                col = None;
            }
            // Raw source extent of the key: everything before the colon, quotes included.
            let key_raw_len = prev_len - after_colon.len() - 1;
            let key_facts = EntryFacts { key_form, key_span: self.span_at(col, key_raw_len) };
            rest = after_colon;
            col = col.map(|c| c + key_raw_len + 1);

            if rest.is_empty() {
                self.line += 1;
                let value = self.parse_value_after_key(pair_indent)?;
                entries.push(T::new_entry(key, value, key_facts));
                return Ok(entries);
            }

            // Inline indent glyph: `key: /<` — value follows on next lines at shifted indent.
            if rest == " /<" {
                let glyph_file_indent = pair_indent.saturating_sub(self.idt.offset());
                self.idt.push_glyph(glyph_file_indent);
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
            col = col.map(|c| c + consumed + space_count);
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
        pair_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        self.skip_ignorable_lines()?;
        let child_indent = pair_indent + 2;
        let line = self
            .current_line()
            .ok_or_else(|| self.error_at_line(self.line, 1, "expected a nested value"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let file_indent = count_leading_spaces(&line);
        let indent = self.idt.logical(file_indent);
        let content = &line[file_indent..];
        if starts_with_marker_chain(content) && (indent == pair_indent || indent == child_indent) {
            return self.parse_marker_chain_line(content, indent);
        }
        // Fold after colon: value starts on a "/ " continuation line at pair_indent.
        // Spec: key and basic value are folded as a single unit; fold marker is allowed
        // immediately after the ":" (preferred), treating the junction at pair_indent+2 indent.
        if indent == pair_indent && content.starts_with("/ ") {
            let continuation_content = &content[2..];
            let (value, consumed) = self.parse_inline_value(
                continuation_content,
                pair_indent,
                ArrayLineValueContext::ObjectValue,
                Some(file_indent + 2),
            )?;
            if consumed.is_some() {
                self.line += 1;
            }
            return Ok(value);
        }
        // Own-line indent glyph: ` /<` at pair_indent (file_indent + 1 with content "/<").
        // The glyph's leading space sits at position pair_indent - offset in the file.
        if indent == pair_indent + 1 && content == "/<" {
            let glyph_file_indent = pair_indent.saturating_sub(self.idt.offset());
            self.idt.push_glyph(glyph_file_indent);
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
        // Halving rounds an odd column down on purpose. A bare string sits one
        // column right of its structural position, that column being its
        // one-sided opening quote, so a gap of 3 is one level plus a quote and
        // not a level and a half. The recursion then sees the same odd gap it
        // would have seen without any jump, and reads the quote as it always
        // does.
        let extra_levels = (indent - child_indent) / 2;
        if extra_levels > 0 && self.options.missing_indent_marker == MissingIndentMarker::Infer {
            // The synthesized arrays have no line of their own, so they take the
            // span of the line their one element starts on -- which is what every
            // other implicit container here does.
            let open_span = self.current_span();
            let mut value = self.parse_value_after_key(child_indent + 2 * extra_levels - 2)?;
            for _ in 0..extra_levels {
                value = T::new_array(vec![value], self.container_facts_from(open_span));
            }
            return Ok(value);
        }
        let child_file_indent = child_indent.saturating_sub(self.idt.offset());
        let content = &line[child_file_indent..];
        // `content` sits at `child_indent`, so that is where its fold continuations
        // are too. Passing `pair_indent` here looked for them two columns to the
        // left and never found them, so a folded key on a nested line was not
        // recognised as opening an object.
        if self.looks_like_object_start(content, child_indent) {
            self.parse_implicit_object(pair_indent)
        } else {
            self.parse_implicit_array(pair_indent)
        }
    }

    fn parse_standalone_scalar_line(
        &mut self,
        content: &str,
        line_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        // Spec: MINIMAL JSON "must be on a line by itself ... nothing may come after
        // it on that line". So a candidate takes the whole line; if it does not parse
        // as such, that is an error rather than a packed element.
        if is_minimal_json_candidate(content) {
            let span = self.span_at(Some(self.options.start_indent), content.len());
            let value = self.parse_minimal_json_line(content, span)?;
            self.line += 1;
            return Ok(value);
        }
        let (value, consumed) = self.parse_inline_value(
            content,
            line_indent,
            ArrayLineValueContext::SingleValue,
            Some(self.options.start_indent),
        )?;
        if let Some(consumed) = consumed {
            if consumed != content.len() {
                return Err(self.error_current("only one value may appear here"));
            }
            self.line += 1;
        }
        Ok(value)
    }

    fn parse_array_tail(
        &mut self,
        parent_indent: usize,
        elements: &mut Vec<T>,
    ) -> std::result::Result<(), ParseError> {
        let elem_indent = parent_indent + 2;
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            // Close glyph: pop offset and continue.
            if self.idt.try_pop_close(&line) {
                self.line += 1;
                continue;
            }
            let file_indent = count_leading_spaces(&line);
            let indent = self.idt.logical(file_indent);
            let content = &line[file_indent..];
            if indent < parent_indent {
                break;
            }
            if starts_with_marker_chain(content) && indent == elem_indent {
                elements.push(self.parse_marker_chain_line(content, indent)?);
                continue;
            }
            if indent < elem_indent {
                break;
            }
            // Structural indents are always even; an odd file_indent means the extra space is part
            // of the content (glyph leading space or bare string leading space).
            let elem_struct_pos = elem_indent.saturating_sub(self.idt.offset());
            if file_indent == elem_struct_pos + 1 {
                // Bare strings can never start with `/`, so content=="/<" is unambiguously a glyph.
                if content == "/<" {
                    self.idt.push_glyph(elem_struct_pos);
                    self.line += 1;
                    continue;
                }
                let pending = self.take_pending_comments();
                let first_new = elements.len();
                self.parse_array_line_content(
                    &line[elem_struct_pos..],
                    elem_indent,
                    elements,
                    Some(elem_struct_pos),
                )?;
                if T::KEEPS_COMMENTS
                    && !pending.is_empty()
                    && let Some(first) = elements.get_mut(first_new)
                {
                    T::attach_comments_before(first, pending, elem_indent);
                }
                continue;
            }
            // Standalone glyph at structural indent elem_indent+2: introduces a nested sub-array.
            let sub_glyph_struct = (elem_indent + 2).saturating_sub(self.idt.offset());
            if file_indent == sub_glyph_struct + 1 && content == "/<" {
                self.idt.push_glyph(sub_glyph_struct);
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
            if indent == elem_indent + 2 && !elements.is_empty() {
                let nested_content = &line[file_indent..];
                let pending = self.take_pending_comments();
                let mut nested = if self.looks_like_object_start(nested_content, indent) {
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
            let content = &line[file_indent..];
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
                let span = self.span_at(Some(file_indent), content.len());
                elements.push(self.parse_minimal_json_line(content, span)?);
                self.line += 1;
            } else {
                self.parse_array_line_content(content, elem_indent, elements, Some(file_indent))?;
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

    fn parse_array_line_content(
        &mut self,
        content: &str,
        elem_indent: usize,
        elements: &mut Vec<T>,
        col0: Option<usize>,
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
        loop {
            // `rest` is always a suffix of `content`, so the element's byte column is
            // recoverable from how much has been consumed.
            let col = col0.map(|c| c + (content.len() - rest.len()));
            let element_is_bare = rest.starts_with(' ') || rest.starts_with('_');
            let (value, consumed) =
                self.parse_inline_value(rest, elem_indent, ArrayLineValueContext::ArrayLine, col)?;
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
            if rest == "," {
                self.line += 1;
                return Ok(());
            }
            if let Some(next) = rest.strip_prefix(", ") {
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
                if rest.is_empty() {
                    return Err(self.error_current("array lines cannot end with a separator"));
                }
                continue;
            }
            if let Some(next) = rest.strip_prefix("  ") {
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
                if rest.is_empty() {
                    return Err(self.error_current("array lines cannot end with a separator"));
                }
                continue;
            }
            return Err(self.error_current(
                "array elements on the same line are separated by ', ' or by two spaces in \
                 all-bare-string arrays",
            ));
        }
    }

    fn parse_marker_chain_line(
        &mut self,
        content: &str,
        line_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        // Every container introduced by this marker line carries the marker line's span.
        let open_span = self.current_span();
        // Comments preceding a marker line attach to the container it introduces.
        let pending = self.take_pending_comments();
        // `line_indent` is logical; spans need the raw byte column of `content`'s start.
        let base_col = line_indent.saturating_sub(self.idt.offset());
        let mut rest = content;
        let mut markers = Vec::new();
        // Which levels the writer typed and which this read in. An explicit
        // marker is a fact about the document and is never revised; an inferred
        // one is a level the indentation already established, whose only
        // missing part is the glyph that would have told a reader about it.
        let mut inferred = Vec::new();
        loop {
            if let Some(next) = rest.strip_prefix("[ ") {
                markers.push(ContainerKind::Array);
                inferred.push(false);
                rest = next;
                continue;
            }
            if let Some(next) = rest.strip_prefix("{ ") {
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
                        base_col + (content.len() - rest.len()),
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
        // Only the deepest level can be an object, and an inferred one has no
        // glyph saying which it is, so it answers the same question an ordinary
        // one-level nesting answers: a key and a colon make it an object, and
        // anything else leaves it the array it was assumed to be.
        if *inferred.last().unwrap()
            && self.looks_like_object_start(rest, line_indent + 2 * markers.len())
        {
            *markers.last_mut().unwrap() = ContainerKind::Object;
        }
        if markers[..markers.len().saturating_sub(1)]
            .iter()
            .any(|kind| *kind != ContainerKind::Array)
        {
            return Err(
                self.error_current("only the final explicit nesting marker on a line may be '{'")
            );
        }
        let deepest_parent_indent = line_indent + 2 * markers.len().saturating_sub(1);

        // Indent glyph after markers: `[ [ /<` — content follows on next lines at shifted indent.
        if rest == " /<" {
            let glyph_file_indent = (deepest_parent_indent + 2).saturating_sub(self.idt.offset());
            self.idt.push_glyph(glyph_file_indent);
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
                    let pair_indent = deepest_parent_indent + 2;
                    let mut entries = Vec::new();
                    self.parse_object_tail(pair_indent, &mut entries)?;
                    if entries.is_empty() {
                        return Err(self.error_current("expected at least one object entry after indent glyph"));
                    }
                    T::new_object(entries, self.container_facts_from(open_span))
                }
            };
            for level in (0..markers.len().saturating_sub(1)).rev() {
                let parent_indent = line_indent + 2 * level;
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
                        base_col + (content.len() - rest.len()) + 1,
                        "a table header is one space right of the level it belongs to. A \
                         single space before a value opens a bare string, and a bare string \
                         cannot begin with a pipe, so nothing here explains the space -- \
                         delete it to put the header at its level, or add one more to put \
                         the table a level deeper",
                    ));
                }
                let table_elem_indent = deepest_parent_indent + 2 + leading_spaces;
                let mut value = self.parse_table_array(table_elem_indent)?;
                for level in (0..markers.len().saturating_sub(1)).rev() {
                    let parent_indent = line_indent + 2 * level;
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

        let rest_col = base_col + (content.len() - rest.len());
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
                        deepest_parent_indent + 2,
                        &mut elements,
                        Some(rest_col),
                    )?;
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                }
                T::new_array(elements, self.container_facts_from(open_span))
            }
            ContainerKind::Object => {
                let pair_indent = line_indent + 2 * markers.len();
                let mut entries =
                    self.parse_object_line_content(rest, pair_indent, Some(rest_col))?;
                self.parse_object_tail(pair_indent, &mut entries)?;
                T::new_object(entries, self.container_facts_from(open_span))
            }
        };
        for level in (0..markers.len().saturating_sub(1)).rev() {
            let parent_indent = line_indent + 2 * level;
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
        fold_indent: usize,
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
                                 whole key, or below its value",
                            )?;
                            next += 1;
                            continue;
                        }
                        FoldNext::Ends => break,
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
                             key, or below its value",
                        )?;
                        next += 1;
                        continue;
                    }
                    FoldNext::Ends => break,
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
            return Err(self.error_at_line(
                self.line,
                fold_indent + 1,
                "a bare key cannot begin with `_` or a character shaped like one. Keys \
                 follow the bare string rules, and this column is where a bare string's \
                 opening marker goes -- so a key starting here would be read as a marked \
                 string rather than a key. Double quote it",
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
            return Err(self.error_at_line(
                self.line,
                fold_indent + 1,
                "a nesting marker here says a container starts at this column, but this is \
                 where this object's keys go and a container cannot be an entry without a \
                 key. If this is meant to continue a value from the line above, a marker \
                 cannot do that -- it can only start something new; indent the continuation \
                 instead. If it is meant to be a new entry, give it a key",
            ));
        }
        Err(self.error_at_line(self.line, fold_indent + 1, "invalid object key"))
    }

    fn parse_inline_value(
        &mut self,
        content: &str,
        line_indent: usize,
        context: ArrayLineValueContext,
        col: Option<usize>,
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
                    && let Some(rest) = content.strip_prefix("  ")
                {
                    let value = self.parse_inline_array(rest, line_indent, col.map(|c| c + 2))?;
                    return Ok((value, None));
                }
                if content.starts_with(" `") {
                    // Opener facts are captured before the body parse moves past it.
                    let opener_span = self.span_at(col.map(|c| c + 1), content.len().saturating_sub(1));
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
                             spaces, or put the value on its own line below the key",
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
                if let Some(fault) = bare_string_fault(complete, &self.options) {
                    return Err(self.error_at_col(col, fault.describe()));
                }
                if let Some((acc, next)) = folded {
                    // Facts before the line advance so the span lands on the opener line.
                    let facts = self.string_facts_at(
                        StringForm::Bare(bare_form),
                        col.map(|c| c + 1),
                        end.saturating_sub(1),
                    );
                    self.line = next;
                    return Ok((T::new_string(acc, facts), None));
                }
                Ok((
                    T::new_string(
                        value.to_owned(),
                        self.string_facts_at(StringForm::Bare(bare_form), col.map(|c| c + 1), end.saturating_sub(1)),
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
                    && let Some(rest) = content.strip_prefix("[ ")
                {
                    let value = self.parse_inline_array(rest, line_indent, col.map(|c| c + 2))?;
                    return Ok((value, None));
                }
                if is_minimal_json_candidate(content) {
                    // Spec: MINIMAL JSON "MUST NEVER be packed in a TJSON line with
                    // any other value", with one exception -- "a non folded bare or
                    // quoted key immediately before [it] on its same line". So it is
                    // allowed as an object value or alone, never as an element of a
                    // packed array.
                    if context == ArrayLineValueContext::ArrayLine {
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
                    if context == ArrayLineValueContext::ArrayLine {
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
                let end = simple_token_end(content, context);
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
                let end = simple_token_end(content, context);
                let token = &content[..end];
                Err(self.error_at_col(col, format!("invalid JSON number: \"{token}\" (numbers must start with a digit)")))
            }
            _ => {
                // Nothing here starts a value. If a colon appears later on the
                // line the writer probably meant a key, so report what is wrong
                // with the key rather than a generic value fault that points at
                // the wrong construct and names no rule.
                if let Some(fault) = attempted_bare_key_fault(content, &self.options) {
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
                if content.starts_with("/ ") || content == "/" {
                    return Err(self.error_at_col(
                        col,
                        "a `/ ` line continues a value from the line above it, and \
                         nothing above this line is left open. Remove the `/ `, or \
                         put the value it was meant to continue on the line above",
                    ));
                }
                // No colon anywhere on the line: an entry needs one, and that is a
                // likelier reading than anything about spaces. Both ways out are
                // worth naming, because which was meant is not knowable here.
                // The whole physical line, not this value fragment: a fragment
                // after a colon naturally has none of its own, and reading that
                // as "no colon on this line" hijacks every value fault.
                if self.current_line().is_some_and(|line| !line.contains(':'))
                    && content.starts_with(|c: char| is_unicode_letter_or_number(c))
                {
                    return Err(self.error_at_col(
                        col,
                        "there is no colon on this line, so it is not a key and a \
                         value. An object entry is written `key: value`. If the whole \
                         line was meant as a string, it needs a space in front of it \
                         -- the space is what opens a bare string",
                    ));
                }
                if bare_string_fault(content, &self.options).is_none() {
                    let ladder = match context {
                        // Deliberately not "add a space": inside a *comma* packed
                        // array that produces a bare string among non-bare
                        // elements, which the all-or-none rule then rejects. The
                        // advice would trade this error for another one, and a
                        // suggestion that does not work is worse than none.
                        ArrayLineValueContext::ArrayLine =>
                            "A comma packed array cannot hold a bare string at all: the \
                             space that opens one makes the comma after it part of the \
                             string rather than a separator, so there is no spacing that \
                             works here. Double quote this element, or put the array on \
                             multiple lines",
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
                }
                Err(self.error_at_col(col, "invalid value start"))
            }
        }
    }

    fn parse_inline_array(
        &mut self,
        content: &str,
        parent_indent: usize,
        col0: Option<usize>,
    ) -> std::result::Result<T, ParseError> {
        let open_span = self.span_at(col0, content.len());
        let mut values = Vec::new();
        self.parse_array_line_content(content, parent_indent + 2, &mut values, col0)?;
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
                1,
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
                1,
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
        line_indent: usize,
    ) -> std::result::Result<(String, MultilineFlavor), ParseError> {
        let (glyph, suffix) = if let Some(rest) = content.strip_prefix(" ```") {
            ("```", rest)
        } else if let Some(rest) = content.strip_prefix(" ``") {
            ("``", rest)
        } else if let Some(rest) = content.strip_prefix(" `") {
            ("`", rest)
        } else {
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

        // Closer must exactly match opener glyph including any explicit suffix
        let closer = format!("{} {}{}", spaces(line_indent), glyph, suffix);
        let opener_line = self.line;
        self.line += 1;

        let (body, flavor) = match glyph {
            "```" => (
                self.parse_triple_backtick_body(local_eol, &closer, opener_line)?,
                MultilineFlavor::Triple,
            ),
            "``" => (
                self.parse_double_backtick_body(local_eol, &closer, opener_line)?,
                MultilineFlavor::Double,
            ),
            "`" => (
                self.parse_single_backtick_body(line_indent, local_eol, &closer, opener_line)?,
                MultilineFlavor::Single,
            ),
            _ => unreachable!(),
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
            let Some(line) = self.current_line().map(str::to_owned) else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line == closer {
                self.line += 1;
                break;
            }
            if line_count > 0 {
                value.push_str(local_eol.as_str());
            }
            value.push_str(&line);
            line_count += 1;
            self.line += 1;
        }
        self.check_multiline_minimum(&value, "```", line_count)?;
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
        loop {
            let Some(line) = self.current_line().map(str::to_owned) else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line == closer {
                self.line += 1;
                break;
            }
            let trimmed = line.trim_start_matches(' ');
            if let Some(content_part) = trimmed.strip_prefix("| ") {
                if line_count > 0 {
                    value.push_str(local_eol.as_str());
                }
                value.push_str(content_part);
                line_count += 1;
            } else if let Some(cont_part) = trimmed.strip_prefix("/ ") {
                if line_count == 0 {
                    return Err(self.error_current(
                        "fold continuation cannot appear before any content in a `` multiline string",
                    ));
                }
                value.push_str(cont_part);
            } else if let Some(detail) = self.closer_misindent(&line, closer) {
                return Err(self.error_current(detail));
            } else {
                return Err(self.error_current(
                    "`` multiline string body lines must start with '| ' or '/ '",
                ));
            }
            self.line += 1;
        }
        self.check_multiline_minimum(&value, "``", line_count)?;
        Ok(value)
    }

    fn parse_single_backtick_body(
        &mut self,
        n: usize,
        local_eol: MultilineLocalEol,
        closer: &str,
        opener_line: usize,
    ) -> std::result::Result<String, ParseError> {
        let content_indent = n + 2;
        let fold_marker = format!("{}{}", spaces(n), "/ ");
        let mut value = String::new();
        let mut line_count = 0usize;
        loop {
            let Some(line) = self.current_line().map(str::to_owned) else {
                return Err(self.unterminated_multiline(opener_line, closer));
            };
            if line == closer {
                self.line += 1;
                break;
            }
            if line.starts_with(&fold_marker) {
                if line_count == 0 {
                    return Err(self.error_current(
                        "fold continuation cannot appear before any content in a ` multiline string",
                    ));
                }
                value.push_str(&line[content_indent..]);
                self.line += 1;
                continue;
            }
            if count_leading_spaces(&line) < content_indent {
                return Err(self.error_current(
                    "` multiline string content lines must be indented at n+2 spaces",
                ));
            }
            if line_count > 0 {
                value.push_str(local_eol.as_str());
            }
            value.push_str(&line[content_indent..]);
            line_count += 1;
            self.line += 1;
        }
        self.check_multiline_minimum(&value, "`", line_count)?;
        Ok(value)
    }

    fn parse_folded_json_string(
        &mut self,
        content: &str,
        fold_indent: usize,
    ) -> std::result::Result<String, ParseError> {
        let mut json = content.to_owned();
        let start_line = self.line;
        self.line += 1;
        loop {
            let line = self
                .current_line()
                .ok_or_else(|| self.error_at_line(start_line, fold_indent + 1, "unterminated JSON string"))?
                .to_owned();
            self.ensure_line_has_no_tabs(self.line)?;
            // Spec: "A comment may not be within a fold." Checked before the
            // indent test, because a comment may sit at any indentation and
            // would otherwise be reported as an unterminated string, blaming
            // the line where the string opened rather than the comment.
            if line.trim_start_matches(' ').starts_with("//") {
                self.comment_in_fold(
                    self.line,
                    "this comment sits in the middle of a quoted string that \
                     continues below it -- move the comment above the whole value, \
                     or below it",
                )?;
                self.line += 1;
                continue;
            }
            let raw_fi = count_leading_spaces(&line);
            if self.idt.logical(raw_fi) != fold_indent {
                return Err(self.error_at_line(start_line, fold_indent + 1, "unterminated JSON string"));
            }
            let rest = &line[raw_fi..];
            if !rest.starts_with("/ ") {
                return Err(self.error_at_line(start_line, fold_indent + 1, "unterminated JSON string"));
            }
            json.push_str(&rest[2..]);
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
        if let Err(col) = is_valid_minimal_json(content) {
            return Err(self.error_at_line(
                self.line,
                col + 1,
                "invalid MINIMAL JSON (whitespace outside strings is forbidden)",
            ));
        }
        let value: JsonValue = serde_json::from_str(content).map_err(|error| {
            let col = error.column();
            self.error_at_line(self.line, col, format!("minimal JSON error: {error}"))
        })?;
        // The target decides how source facts apply to the fragment's interior —
        // e.g. an annotated tree marks interior strings Quoted, since that is how
        // JSON spells strings.
        Ok(T::from_minimal_json(value, ContainerFacts { span, table: false }))
    }

    fn line_str(&self, index: usize) -> Option<&str> {
        self.line_offsets.get(index).map(|s| &self.input[s.start..s.start + s.len])
    }

    fn current_line(&self) -> Option<&str> {
        self.line_str(self.line)
    }

    fn skip_ignorable_lines(&mut self) -> std::result::Result<(), ParseError> {
        let mut first_comment: Option<usize> = None;
        while let Some(line) = self.current_line() {
            self.ensure_line_has_no_tabs(self.line)?;
            let trimmed = line.trim_start_matches(' ');
            if trimmed.starts_with("//") {
                if first_comment.is_none() {
                    first_comment = Some(self.line);
                }
                if T::KEEPS_COMMENTS {
                    let comment = RawComment {
                        col: line.len() - trimmed.len(),
                        text: trimmed.to_owned(),
                    };
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
                        1,
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
                    1,
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
            && next.trim_start_matches(' ').starts_with("/ ")
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
            .all(|line| line.trim_start_matches(' ').is_empty())
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
                1,
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
                    let trimmed = line.trim_start_matches(' ');
                    let comment = RawComment {
                        col: line.len() - trimmed.len(),
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
        col: Option<usize>,
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
        let token = &content[..simple_token_end(content, context)];
        if next == ' ' || (next == ',' && context == ArrayLineValueContext::ArrayLine) {
            return Ok(());
        }
        Err(self.error_at_col(
            col.map(|c| c + literal.len()),
            format!(
                "`{literal}` is followed by `{next}`, and nothing may follow it. \
                 `k:{literal}` writes the {} itself, so it has to end there. If you \
                 meant the text `{token}`, write it as a bare string with a space \
                 after the colon -- `k: {token}` -- since the space is what opens a \
                 string",
                if literal == "null" { "null" } else { "boolean" }
            ),
        ))
    }

    /// Did this line close the string, only at the wrong indentation?
    ///
    /// A misindented closer otherwise reports as a malformed body line, which is
    /// true and useless: the writer did close the string, they closed it a
    /// column off, and nothing in "body lines must start with `| `" says where
    /// it belonged. The glyph carries no indentation cue of its own and the
    /// miss is usually one space, so it is invisible on the page -- the only
    /// thing that helps is naming both columns.
    fn closer_misindent(&self, line: &str, closer: &str) -> Option<String> {
        let glyph = closer.trim_start_matches(' ');
        if line.trim_start_matches(' ') != glyph {
            return None;
        }
        let expected = closer.len() - glyph.len() + 1;
        let found = count_leading_spaces(line) + 1;
        Some(format!(
            "the closing {glyph} glyph is at column {found} but belongs at column \
             {expected}, one space further in than the key that opened the string. \
             Where it is, it reads as a body line -- and a body line has to start \
             with '| ' or '/ '"
        ))
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
        let column = closer.len() - glyph.len() + 1;

        let mut probe = opener_line + 1;
        while let Some(line) = self.line_str(probe) {
            if line.trim_start_matches(' ').trim_end() == glyph {
                let found = count_leading_spaces(line) + 1;
                let reason = if found == column {
                    "has trailing spaces after it, and a closing glyph has to be the \
                     whole line"
                        .to_owned()
                } else {
                    format!("is at column {found}, not column {column}")
                };
                return self.error_at_line(
                    probe,
                    1,
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
            1,
            format!(
                "unterminated multiline string: no closing {glyph} was found. It must be \
                 {glyph} alone on its own line at column {column}, one space further in \
                 than the key that opened the string"
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
    fn bare_comma_error(&self, col: Option<usize>) -> ParseError {
        self.error_at_col(
            col,
            "a bare string in a comma separated packed array may not contain a comma; \
             double quote it, or consider unpacking this line onto multiple lines",
        )
    }

    fn mixed_pack_error(&self, col: Option<usize>, bare_scalar: Option<&str>) -> ParseError {
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
        let indent_end = line.len() - line.trim_start_matches([' ', '\t']).len();
        if let Some(column) = line[..indent_end].find('\t') {
            return Err(self.error_at_line(
                line_index,
                column + 1,
                "tab characters are not allowed as indentation",
            ));
        }
        Ok(())
    }

    fn looks_like_object_start(&self, content: &str, fold_indent: usize) -> bool {
        if content.starts_with('|') || starts_with_marker_chain(content) {
            return false;
        }
        if let Some(end) = parse_bare_key_prefix(content, &self.options) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return true;
            }
            // Bare run fills the whole line and continues with `/ `. That is
            // either a folded key whose colon lands on a later line, or a
            // folded scalar (an array element, or the whole root). Only the
            // colon tells them apart, so reassemble and look for it rather
            // than assuming. Assuming "key" made a folded number in an array
            // fail to parse as `invalid object key`.
            if only_held_back_tail(content, end, &self.options)
                && self.folded_bare_has_colon(content, fold_indent)
            {
                return true;
            }
        }
        if let Some((_, end)) = parse_json_string_prefix(content) {
            return content.get(end..).is_some_and(|rest| rest.starts_with(':'));
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
        false
    }

    /// What follows a fold, decided in one place.
    ///
    /// Every fold walker needs the same three-way answer, and each used to
    /// inline its own copy of the indent-and-`/ ` test. Keeping one definition
    /// means they cannot drift, and gives the comment case somewhere to live:
    /// the spec says a comment may not be within a fold, so it is neither a
    /// continuation nor a clean end.
    fn classify_fold_next(&self, line_no: usize, fold_indent: usize) -> FoldNext<'_> {
        let Some(line) = self.line_str(line_no) else {
            return FoldNext::Ends;
        };
        let raw_fi = count_leading_spaces(line);
        let rest = &line[raw_fi..];
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
                if peek.trim_start_matches(' ').starts_with("//") {
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
        if self.idt.logical(raw_fi) != fold_indent || !rest.starts_with("/ ") {
            return FoldNext::Ends;
        }
        FoldNext::Continues(&rest[2..])
    }

    /// Reassemble a quoted string across its `/ ` fold continuations and report
    /// whether a `:` follows it. Read-only lookahead: the caller re-walks the
    /// same lines once it has decided how to interpret them.
    fn folded_json_string_has_colon(&self, content: &str, fold_indent: usize) -> bool {
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
            };
            acc.push_str(rest);
            next += 1;
            if let Some((_, end)) = parse_json_string_prefix(&acc) {
                return acc.get(end..).is_some_and(|r| r.starts_with(':'));
            }
        }
        false
    }

    /// Reassemble a bare run across its `/ ` fold continuations and report
    /// whether a `:` follows it. The bare twin of `folded_json_string_has_colon`,
    /// and it exists for the same reason: at a given indent a folded key and a
    /// folded scalar look identical until the colon shows up, or fails to.
    ///
    /// Read-only lookahead; the caller re-walks the same lines once it has
    /// decided how to read them.
    fn folded_bare_has_colon(&self, content: &str, fold_indent: usize) -> bool {
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
            };
            acc.push_str(rest);
            next += 1;
            if let Some(end) = parse_bare_key_prefix(&acc, &self.options)
                && acc.get(end..).is_some_and(|r| r.starts_with(':'))
            {
                return true;
            }
        }
        false
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
    fn missing_marker_error(&self, col: usize, depth: usize) -> ParseError {
        self.error_at_line(
            self.line,
            col + 1,
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

    fn error_at_col(&self, col: Option<usize>, message: impl Into<String>) -> ParseError {
        match col {
            Some(c) => self.error_at_line(self.line, c + 1, message),
            None => self.error_current(message),
        }
    }

    fn error_current(&self, message: impl Into<String>) -> ParseError {
        let column = self
            .current_line()
            .map(|line| count_leading_spaces(line) + 1)
            .unwrap_or(1);
        self.error_at_line(self.line, column, message)
    }

    fn error_at_line(
        &self,
        line_index: usize,
        column: usize,
        message: impl Into<String>,
    ) -> ParseError {
        // Callers count columns in bytes, because that is what parsing works in.
        // A reader counts characters, and so does the caret the error prints. The
        // two agree only for ASCII, so the conversion happens here -- the one
        // funnel every error passes through -- rather than at each of the callers
        // that would each have to remember.
        let source = self.line_str(line_index);
        let column = match source {
            Some(line) => {
                let mut byte = column.saturating_sub(1).min(line.len());
                while byte > 0 && !line.is_char_boundary(byte) {
                    byte -= 1;
                }
                line[..byte].chars().count() + 1
            }
            None => column,
        };
        ParseError::new(line_index + 1, column, message, source.map(str::to_owned))
    }
}


fn bare_string_end(content: &str, context: ArrayLineValueContext) -> usize {
    match context {
        ArrayLineValueContext::ArrayLine | ArrayLineValueContext::ObjectValue => {
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

/// Is everything past `end` just the tail a key/string run holds back?
///
/// A run stops short of trailing spaces and commas because it may not end on one.
/// When a fold continuation follows, the value does not end there at all, so that
/// tail is interior content and the run does reach the end of the line.
fn only_held_back_tail(content: &str, end: usize, forms: &ParseOptions) -> bool {
    content[end..]
        .chars()
        .all(|c| c == ' ' || forms.is_comma_like(c) || forms.is_quote_like(c))
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
/// is a rule about the finished value, so it belongs to `bare_string_fault` and
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
fn simple_token_end(content: &str, context: ArrayLineValueContext) -> usize {
    match context {
        ArrayLineValueContext::ArrayLine => {
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
