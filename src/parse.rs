use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::number::Number;

use crate::error::ParseError;
use crate::tree::{
    ContainerFacts, EntryFacts, KeyForm, MultilineFlavor, RawComment, ScalarFacts, Span,
    StringFacts, StringForm, Tree,
};
use crate::util::*;

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ParseOptions {
    pub(crate) start_indent: usize,
}

/// Options controlling how TJSON is rendered. Use [`RenderOptions::default`] for sensible

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
    start_indent: usize,
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
        start_indent: usize,
    ) -> std::result::Result<T, ParseError> {
        // Span offsets are stored as u32 (see tree::Span); bound the input before any
        // are produced so an oversized document fails loudly instead of mis-spanning.
        if input.len() > u32::MAX as usize {
            return Err(ParseError::new(1, 1, "input larger than 4 GiB is not supported", None));
        }
        let mut parser = Self {
            input,
            line_offsets: scan_lines(input)?,
            line: 0,
            start_indent,
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
            T::attach_comments_before(&mut value, root_pending, start_indent);
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
                "unexpected trailing content"
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

        if indent == self.start_indent && starts_with_marker_chain(content) {
            return self.parse_marker_chain_line(content, indent);
        }

        // Standalone root-level start glyph: ` /<` at structural indent start_indent+2.
        // Structural indent is always even; file_indent is structural+1 (the glyph's leading space).
        let root_glyph_struct = (self.start_indent + 2).saturating_sub(self.idt.offset());
        if file_indent == root_glyph_struct + 1 && content == "/<" {
            self.idt.push_glyph(root_glyph_struct);
            self.line += 1;
            self.skip_ignorable_lines()?;
            return self.parse_root_value();
        }

        if indent <= self.start_indent + 1 {
            return self
                .parse_standalone_scalar_line(&line[self.start_indent..], self.start_indent);
        }

        if indent >= self.start_indent + 2 {
            let child_file_pos = (self.start_indent + 2).saturating_sub(self.idt.offset());
            let child_content = &line[child_file_pos..];
            if self.looks_like_object_start(child_content, self.start_indent + 2) {
                return self.parse_implicit_object(self.start_indent);
            }
            return self.parse_implicit_array(self.start_indent);
        }

        Err(self.error_current("expected a value at the starting indent"))
    }

    fn parse_implicit_object(
        &mut self,
        parent_indent: usize,
    ) -> std::result::Result<T, ParseError> {
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
                // Peek past ignorable lines to find the next meaningful line.
                let mut offset = 1usize;
                while let Some(peek) = self.line_str(self.line + offset) {
                    let trimmed = peek.trim_start_matches(' ');
                    if trimmed.starts_with("//") {
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
        if let Some(end) = parse_bare_key_prefix(cell)
            && end == cell.len() {
                return Ok((cell.to_owned(), KeyForm::Bare));
            }
        if let Some((value, end)) = parse_json_string_prefix(cell)
            && end == cell.len() {
                return Ok((value, KeyForm::Quoted));
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
        if let Some(value) = cell.strip_prefix(' ') {
            if !is_allowed_bare_string(value) {
                return Err(self.error_at_line(self.line, 1, "invalid bare string in table cell"));
            }
            let facts = self.string_facts_at(StringForm::Bare, None, 0);
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
                return Err(self.error_current("object lines cannot end with a separator"));
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
        let child_file_indent = child_indent.saturating_sub(self.idt.offset());
        let content = &line[child_file_indent..];
        if self.looks_like_object_start(content, pair_indent) {
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
        if is_minimal_json_candidate(content) {
            let span = self.span_at(Some(self.start_indent), content.len());
            let value = self.parse_minimal_json_line(content, span)?;
            self.line += 1;
            return Ok(value);
        }
        let (value, consumed) = self.parse_inline_value(
            content,
            line_indent,
            ArrayLineValueContext::SingleValue,
            Some(self.start_indent),
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
        let mut string_only_mode = false;
        loop {
            // `rest` is always a suffix of `content`, so the element's byte column is
            // recoverable from how much has been consumed.
            let col = col0.map(|c| c + (content.len() - rest.len()));
            let (value, consumed) =
                self.parse_inline_value(rest, elem_indent, ArrayLineValueContext::ArrayLine, col)?;
            let is_string = value.is_string();
            if string_only_mode && !is_string {
                return Err(self.error_current(
                    "two-space array packing is only allowed when all values are strings",
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
                string_only_mode = false;
                if rest.is_empty() {
                    return Err(self.error_current("array lines cannot end with a separator"));
                }
                continue;
            }
            if let Some(next) = rest.strip_prefix("  ") {
                rest = next;
                string_only_mode = true;
                if rest.is_empty() {
                    return Err(self.error_current("array lines cannot end with a separator"));
                }
                continue;
            }
            return Err(self.error_current(
                "array elements on the same line are separated by ', ' or by two spaces in string-only arrays",
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
        loop {
            if let Some(next) = rest.strip_prefix("[ ") {
                markers.push(ContainerKind::Array);
                rest = next;
                continue;
            }
            if let Some(next) = rest.strip_prefix("{ ") {
                markers.push(ContainerKind::Object);
                rest = next;
                break;
            }
            break;
        }
        if markers.is_empty() {
            return Err(self.error_current("expected an explicit nesting marker"));
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
    fn parse_key(
        &mut self,
        content: &str,
        fold_indent: usize,
    ) -> std::result::Result<(String, KeyForm, String), ParseError> {
        // Bare key on this line
        if let Some(end) = parse_bare_key_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((
                    content[..end].to_owned(),
                    KeyForm::Bare,
                    content[end + ':'.len_utf8()..].to_owned(),
                ));
            }
            // Bare key fills the whole line — look for fold continuations
            if end == content.len() {
                let mut key_acc = content[..end].to_owned();
                let mut next = self.line + 1;
                loop {
                    let (colon_pos, cont_owned) = {
                        let Some(fold_line) = self.line_str(next) else { break; };
                        let raw_fi = count_leading_spaces(fold_line);
                        if self.idt.logical(raw_fi) != fold_indent { break; }
                        let rest = &fold_line[raw_fi..];
                        if !rest.starts_with("/ ") { break; }
                        let cont = &rest[2..];
                        (cont.find(':'), cont.to_owned())
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
            while let Some(fold_line) = self.line_str(next) {
                let fi = count_leading_spaces(fold_line);
                if fi != fold_indent {
                    break;
                }
                let rest = &fold_line[fi..];
                if !rest.starts_with("/ ") {
                    break;
                }
                json_acc.push_str(&rest[2..]);
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
            ' ' => {
                if context == ArrayLineValueContext::ObjectValue {
                    if content.starts_with(" []") {
                        let facts = ContainerFacts { span: self.span_at(col.map(|c| c + 1), 2), table: false };
                        return Ok((T::new_array(Vec::new(), facts), Some(3)));
                    }
                    if content.starts_with(" {}") {
                        let facts = ContainerFacts { span: self.span_at(col.map(|c| c + 1), 2), table: false };
                        return Ok((T::new_object(Vec::new(), facts), Some(3)));
                    }
                    if let Some(rest) = content.strip_prefix("  ") {
                        let value = self.parse_inline_array(rest, line_indent, col.map(|c| c + 2))?;
                        return Ok((value, None));
                    }
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
                    return Err(self.error_current("bare strings cannot start with a forbidden character"));
                }
                let value = &content[' '.len_utf8()..end]; // leading space before bare string value
                if !is_allowed_bare_string(value) {
                    return Err(self.error_current("invalid bare string"));
                }
                // Check for fold continuations when the bare string fills the rest of the content
                if end == content.len() {
                    let mut acc = value.to_owned();
                    let mut next = self.line + 1;
                    let mut fold_count = 0usize;
                    while let Some(fold_line) = self.line_str(next) {
                        let raw_fi = count_leading_spaces(fold_line);
                        if self.idt.logical(raw_fi) != line_indent {
                            break;
                        }
                        let rest = &fold_line[raw_fi..];
                        if !rest.starts_with("/ ") {
                            break;
                        }
                        acc.push_str(&rest[2..]);
                        next += 1;
                        fold_count += 1;
                    }
                    if fold_count > 0 {
                        // Facts before the line advance so the span lands on the opener line.
                        let facts = self.string_facts_at(
                            StringForm::Bare,
                            col.map(|c| c + 1),
                            end.saturating_sub(1),
                        );
                        self.line = next;
                        return Ok((T::new_string(acc, facts), None));
                    }
                }
                Ok((
                    T::new_string(
                        value.to_owned(),
                        self.string_facts_at(StringForm::Bare, col.map(|c| c + 1), end.saturating_sub(1)),
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
                if is_minimal_json_candidate(content) {
                    let span = self.span_at(col, content.len());
                    let value = self.parse_minimal_json_line(content, span)?;
                    return Ok((value, Some(content.len())));
                }
                Err(self.error_current("nonempty arrays require container context"))
            }
            '{' => {
                if content.starts_with("{}") {
                    let facts = ContainerFacts { span: self.span_at(col, 2), table: false };
                    return Ok((T::new_object(Vec::new(), facts), Some(2)));
                }
                if is_minimal_json_candidate(content) {
                    let span = self.span_at(col, content.len());
                    let value = self.parse_minimal_json_line(content, span)?;
                    return Ok((value, Some(content.len())));
                }
                Err(self.error_current("nonempty objects require object or array context"))
            }
            't' if content.starts_with("true") => {
                Ok((T::new_bool(true, self.scalar_facts_at(col, 4)), Some(4)))
            }
            'f' if content.starts_with("false") => {
                Ok((T::new_bool(false, self.scalar_facts_at(col, 5)), Some(5)))
            }
            'n' if content.starts_with("null") => {
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
                    while let Some(fold_line) = self.line_str(next) {
                        let raw_fi = count_leading_spaces(fold_line);
                        if self.idt.logical(raw_fi) != line_indent { break; }
                        let rest = &fold_line[raw_fi..];
                        if !rest.starts_with("/ ") { break; }
                        acc.push_str(&rest[2..]);
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
                    .map_err(|_| self.error_current(format!("invalid JSON number: \"{token}\"")))?;
                Ok((T::new_number(n, self.scalar_facts_at(col, end)), Some(end)))
            }
            '.' if content[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                let end = simple_token_end(content, context);
                let token = &content[..end];
                Err(self.error_current(format!("invalid JSON number: \"{token}\" (numbers must start with a digit)")))
            }
            _ => Err(self.error_current("invalid value start")),
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
                return Err(self.error_at_line(
                    opener_line,
                    1,
                    "unterminated multiline string: reached end of file without closing ``` glyph",
                ));
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
        if line_count < 2 {
            return Err(self.error_at_line(
                self.line - 1,
                1,
                "multiline strings must contain at least one real linefeed",
            ));
        }
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
                return Err(self.error_at_line(
                    opener_line,
                    1,
                    "unterminated multiline string: reached end of file without closing `` glyph",
                ));
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
            } else {
                return Err(self.error_current(
                    "`` multiline string body lines must start with '| ' or '/ '",
                ));
            }
            self.line += 1;
        }
        if line_count < 2 {
            return Err(self.error_at_line(
                self.line - 1,
                1,
                "multiline strings must contain at least one real linefeed",
            ));
        }
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
                return Err(self.error_at_line(
                    opener_line,
                    1,
                    "unterminated multiline string: reached end of file without closing ` glyph",
                ));
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
        if line_count < 2 {
            return Err(self.error_at_line(
                self.line - 1,
                1,
                "multiline strings must contain at least one real linefeed",
            ));
        }
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
        while let Some(line) = self.current_line() {
            self.ensure_line_has_no_tabs(self.line)?;
            let trimmed = line.trim_start_matches(' ');
            if line.is_empty() || trimmed.starts_with("//") {
                if T::KEEPS_COMMENTS && trimmed.starts_with("//") {
                    let comment = RawComment {
                        col: line.len() - trimmed.len(),
                        text: trimmed.to_owned(),
                    };
                    self.pending_comments.push(comment);
                }
                self.line += 1;
                continue;
            }
            break;
        }
        Ok(())
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
        // Only reject tabs in the leading indent — tabs inside quoted string values are allowed.
        let indent_end = line.len() - line.trim_start_matches(' ').len();
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
        if let Some(end) = parse_bare_key_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return true;
            }
            // Bare key fills the whole line — a fold continuation may carry the colon
            if end == content.len() && self.next_line_is_fold_continuation(fold_indent) {
                return true;
            }
        }
        if let Some((_, end)) = parse_json_string_prefix(content) {
            return content.get(end..).is_some_and(|rest| rest.starts_with(':'));
        }
        // JSON string that doesn't close on this line — fold continuation may complete it
        if content.starts_with('"')
            && parse_json_string_prefix(content).is_none()
            && self.next_line_is_fold_continuation(fold_indent)
        {
            return true;
        }
        false
    }

    fn next_line_is_fold_continuation(&self, expected_indent: usize) -> bool {
        self.line_str(self.line + 1).is_some_and(|l| {
            let raw_fi = count_leading_spaces(l);
            self.idt.logical(raw_fi) == expected_indent && l[raw_fi..].starts_with("/ ")
        })
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
        ParseError::new(line_index + 1, column, message, self.line_str(line_index).map(str::to_owned))
    }
}


fn bare_string_end(content: &str, context: ArrayLineValueContext) -> usize {
    match context {
        ArrayLineValueContext::ArrayLine => {
            let mut end = content.len();
            if let Some(index) = content.find("  ") {
                end = end.min(index);
            }
            if let Some(index) = content.find(", ") {
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
