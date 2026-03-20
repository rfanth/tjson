use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use unicode_general_category::{GeneralCategory, get_general_category};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParseOptions {
    pub start_indent: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    pub start_indent: usize,
    pub canonical: bool,
    pub force_markers: bool,
    pub bare_strings: BareStyle,
    pub bare_keys: BareStyle,
    pub inline_objects: bool,
    pub inline_arrays: bool,
    pub string_array_style: StringArrayStyle,
    pub tables: bool,
    pub wrap_width: Option<usize>,
    pub table_min_rows: usize,
    pub table_min_cols: usize,
    pub table_similarity: f32,
    pub table_column_max_width: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BareStyle {
    #[default]
    Prefer,
    None,
}

impl FromStr for BareStyle {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "prefer" => Ok(Self::Prefer),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "invalid bare style '{input}' (expected one of: prefer, none)"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StringArrayStyle {
    Spaces,
    PreferSpaces,
    Comma,
    #[default]
    PreferComma,
    None,
}

impl FromStr for StringArrayStyle {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "spaces" => Ok(Self::Spaces),
            "prefer-spaces" => Ok(Self::PreferSpaces),
            "comma" => Ok(Self::Comma),
            "prefer-comma" => Ok(Self::PreferComma),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "invalid string array style '{input}' (expected one of: spaces, prefer-spaces, comma, prefer-comma, none)"
            )),
        }
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            start_indent: 0,
            canonical: false,
            force_markers: false,
            bare_strings: BareStyle::Prefer,
            bare_keys: BareStyle::Prefer,
            inline_objects: true,
            inline_arrays: true,
            string_array_style: StringArrayStyle::PreferComma,
            tables: true,
            wrap_width: Some(80),
            table_min_rows: 3,
            table_min_cols: 3,
            table_similarity: 0.8,
            table_column_max_width: 40,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TjsonValue {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<TjsonValue>),
    Object(Vec<(String, TjsonValue)>),
}

impl TjsonValue {
    pub fn from_json_value(value: JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(value),
            JsonValue::Number(value) => Self::Number(value.to_string()),
            JsonValue::String(value) => Self::String(value),
            JsonValue::Array(values) => {
                Self::Array(values.into_iter().map(Self::from_json_value).collect())
            }
            JsonValue::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| (key, Self::from_json_value(value)))
                    .collect(),
            ),
        }
    }

    pub fn to_json_value_lossy(&self) -> Result<JsonValue, Error> {
        Ok(match self {
            Self::Null => JsonValue::Null,
            Self::Bool(value) => JsonValue::Bool(*value),
            Self::Number(value) => {
                JsonValue::Number(JsonNumber::from_str(value).map_err(Error::Json)?)
            }
            Self::String(value) => JsonValue::String(value.clone()),
            Self::Array(values) => JsonValue::Array(
                values
                    .iter()
                    .map(TjsonValue::to_json_value_lossy)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Object(entries) => {
                let mut map = JsonMap::new();
                for (key, value) in entries {
                    map.insert(key.clone(), value.to_json_value_lossy()?);
                }
                JsonValue::Object(map)
            }
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl ParseError {
    fn new(line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

impl StdError for ParseError {}

#[derive(Debug)]
pub enum Error {
    Parse(ParseError),
    Json(serde_json::Error),
    Render(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Render(message) => write!(f, "{message}"),
        }
    }
}

impl StdError for Error {}

impl From<ParseError> for Error {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn parse_str(input: &str) -> Result<TjsonValue> {
    parse_str_with_options(input, ParseOptions::default())
}

pub fn parse_str_with_options(input: &str, options: ParseOptions) -> Result<TjsonValue> {
    Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
}

pub fn render_string(value: &TjsonValue) -> Result<String> {
    render_string_with_options(value, RenderOptions::default())
}

pub fn render_string_with_options(value: &TjsonValue, options: RenderOptions) -> Result<String> {
    Renderer::render(value, &options)
}

pub fn from_tjson_str<T: DeserializeOwned>(input: &str) -> Result<T> {
    from_tjson_str_with_options(input, ParseOptions::default())
}

pub fn from_tjson_str_with_options<T: DeserializeOwned>(
    input: &str,
    options: ParseOptions,
) -> Result<T> {
    let value = parse_str_with_options(input, options)?;
    let json = value.to_json_value_lossy()?;
    Ok(serde_json::from_value(json)?)
}

pub fn to_tjson_string<T: Serialize>(value: &T) -> Result<String> {
    to_tjson_string_with_options(value, RenderOptions::default())
}

pub fn to_tjson_string_with_options<T: Serialize>(
    value: &T,
    options: RenderOptions,
) -> Result<String> {
    let json = serde_json::to_value(value)?;
    let value = TjsonValue::from_json_value(json);
    render_string_with_options(&value, options)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArrayLineValueContext {
    ArrayLine,
    ObjectValue,
    SingleValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContainerKind {
    Array,
    Object,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MultilineLocalEol {
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

    fn opener_suffix(self) -> &'static str {
        match self {
            Self::Lf => "",
            Self::CrLf => "\\r\\n",
        }
    }
}

struct Parser {
    lines: Vec<String>,
    line: usize,
    start_indent: usize,
}

impl Parser {
    fn parse_document(
        input: &str,
        start_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        let normalized = normalize_input(input)?;
        let mut parser = Self {
            lines: normalized.split('\n').map(str::to_owned).collect(),
            line: 0,
            start_indent,
        };
        parser.skip_ignorable_lines()?;
        if parser.line >= parser.lines.len() {
            return Err(ParseError::new(1, 1, "empty input"));
        }
        let value = parser.parse_root_value()?;
        parser.skip_ignorable_lines()?;
        if parser.line < parser.lines.len() {
            return Err(parser.error_current("unexpected trailing content"));
        }
        Ok(value)
    }

    fn parse_root_value(&mut self) -> std::result::Result<TjsonValue, ParseError> {
        let line = self
            .current_line()
            .ok_or_else(|| ParseError::new(1, 1, "empty input"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let indent = count_leading_spaces(&line);
        let content = &line[indent..];

        if indent == self.start_indent && starts_with_marker_chain(content) {
            return self.parse_marker_chain_line(content, indent);
        }

        if indent <= self.start_indent + 1 {
            return self
                .parse_standalone_scalar_line(&line[self.start_indent..], self.start_indent);
        }

        if indent >= self.start_indent + 2 {
            let child_content = &line[self.start_indent + 2..];
            if self.looks_like_object_start(child_content) {
                return self.parse_implicit_object(self.start_indent);
            }
            return self.parse_implicit_array(self.start_indent);
        }

        Err(self.error_current("expected a value at the starting indent"))
    }

    fn parse_implicit_object(
        &mut self,
        parent_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        let mut entries = Vec::new();
        self.parse_object_tail(parent_indent + 2, &mut entries)?;
        if entries.is_empty() {
            return Err(self.error_current("expected at least one object entry"));
        }
        Ok(TjsonValue::Object(entries))
    }

    fn parse_implicit_array(
        &mut self,
        parent_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        self.skip_ignorable_lines()?;
        let elem_indent = parent_indent + 2;
        let line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected array contents"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let indent = count_leading_spaces(&line);
        if indent < elem_indent {
            return Err(self.error_current("expected array elements indented by two spaces"));
        }
        let content = &line[elem_indent..];
        if content.starts_with('|') {
            return self.parse_table_array(elem_indent);
        }
        let mut elements = Vec::new();
        self.parse_array_tail(parent_indent, &mut elements)?;
        if elements.is_empty() {
            return Err(self.error_current("expected at least one array element"));
        }
        Ok(TjsonValue::Array(elements))
    }

    fn parse_table_array(
        &mut self,
        elem_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        let header_line = self
            .current_line()
            .ok_or_else(|| self.error_current("expected a table header"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let header = &header_line[elem_indent..];
        let columns = self.parse_table_header(header)?;
        self.line += 1;
        let mut rows = Vec::new();
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            let indent = count_leading_spaces(&line);
            if indent < elem_indent {
                break;
            }
            if indent != elem_indent {
                return Err(self.error_current("expected a table row at the array indent"));
            }
            let row = &line[elem_indent..];
            if !row.starts_with('|') {
                return Err(self.error_current("table arrays may only contain table rows"));
            }
            rows.push(self.parse_table_row(&columns, row)?);
            self.line += 1;
        }
        if rows.is_empty() {
            return Err(self.error_current("table arrays must contain at least one row"));
        }
        Ok(TjsonValue::Array(rows))
    }

    fn parse_table_header(&self, row: &str) -> std::result::Result<Vec<String>, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_line(self.line, 1, "invalid table header"))?;
        if cells.first().is_some_and(String::is_empty) {
            cells.remove(0);
        }
        if cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if cells.is_empty() {
            return Err(self.error_at_line(self.line, 1, "table headers must list columns"));
        }
        cells
            .into_iter()
            .map(|cell| self.parse_table_header_key(cell.trim_end()))
            .collect()
    }

    fn parse_table_header_key(&self, cell: &str) -> std::result::Result<String, ParseError> {
        if let Some(end) = parse_bare_key_prefix(cell) {
            if end == cell.len() {
                return Ok(cell.to_owned());
            }
        }
        if let Some((value, end)) = parse_json_string_prefix(cell) {
            if end == cell.len() {
                return Ok(value);
            }
        }
        Err(self.error_at_line(self.line, 1, "invalid table header key"))
    }

    fn parse_table_row(
        &self,
        columns: &[String],
        row: &str,
    ) -> std::result::Result<TjsonValue, ParseError> {
        let mut cells = split_pipe_cells(row)
            .ok_or_else(|| self.error_at_line(self.line, 1, "invalid table row"))?;
        if cells.first().is_some_and(String::is_empty) {
            cells.remove(0);
        }
        if cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        let mut entries = Vec::new();
        for (index, key) in columns.iter().enumerate() {
            let Some(cell) = cells.get(index) else {
                continue;
            };
            let cell = cell.trim_end();
            if cell.is_empty() {
                continue;
            }
            entries.push((key.clone(), self.parse_table_cell_value(cell)?));
        }
        if cells.len() > columns.len()
            && cells[columns.len()..]
                .iter()
                .any(|cell| !cell.trim_end().is_empty())
        {
            return Err(self.error_at_line(
                self.line,
                1,
                "table row has more cells than the header",
            ));
        }
        Ok(TjsonValue::Object(entries))
    }

    fn parse_table_cell_value(&self, cell: &str) -> std::result::Result<TjsonValue, ParseError> {
        if cell.is_empty() {
            return Err(self.error_at_line(
                self.line,
                1,
                "empty table cells mean the key is absent",
            ));
        }
        if cell.starts_with(' ') {
            let value = &cell[1..];
            if !is_allowed_bare_string(value) {
                return Err(self.error_at_line(self.line, 1, "invalid bare string in table cell"));
            }
            return Ok(TjsonValue::String(value.to_owned()));
        }
        if let Some((value, end)) = parse_json_string_prefix(cell) {
            if end == cell.len() {
                return Ok(TjsonValue::String(value));
            }
        }
        if cell == "true" {
            return Ok(TjsonValue::Bool(true));
        }
        if cell == "false" {
            return Ok(TjsonValue::Bool(false));
        }
        if cell == "null" {
            return Ok(TjsonValue::Null);
        }
        if cell == "[]" {
            return Ok(TjsonValue::Array(Vec::new()));
        }
        if cell == "{}" {
            return Ok(TjsonValue::Object(Vec::new()));
        }
        if JsonNumber::from_str(cell).is_ok() {
            return Ok(TjsonValue::Number(cell.to_owned()));
        }
        Err(self.error_at_line(self.line, 1, "invalid table cell value"))
    }

    fn parse_object_tail(
        &mut self,
        pair_indent: usize,
        entries: &mut Vec<(String, TjsonValue)>,
    ) -> std::result::Result<(), ParseError> {
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            let indent = count_leading_spaces(&line);
            if indent < pair_indent {
                break;
            }
            if indent != pair_indent {
                return Err(self.error_current("expected an object entry at this indent"));
            }
            let content = &line[pair_indent..];
            if content.is_empty() {
                return Err(self.error_current("blank lines are not valid inside objects"));
            }
            let line_entries = self.parse_object_line_content(content, pair_indent)?;
            entries.extend(line_entries);
        }
        Ok(())
    }

    fn parse_object_line_content(
        &mut self,
        content: &str,
        pair_indent: usize,
    ) -> std::result::Result<Vec<(String, TjsonValue)>, ParseError> {
        let mut rest = content;
        let mut entries = Vec::new();
        loop {
            let (key, key_len) = self.parse_key(rest)?;
            rest = &rest[key_len..];
            if !rest.starts_with(':') {
                return Err(self.error_current("expected ':' after an object key"));
            }
            rest = &rest[1..];
            if rest.is_empty() {
                self.line += 1;
                let value = self.parse_value_after_key(pair_indent)?;
                entries.push((key, value));
                return Ok(entries);
            }

            let (value, consumed) =
                self.parse_inline_value(rest, pair_indent, ArrayLineValueContext::ObjectValue)?;
            entries.push((key, value));

            let Some(consumed) = consumed else {
                return Ok(entries);
            };

            rest = &rest[consumed..];
            if rest.is_empty() {
                self.line += 1;
                return Ok(entries);
            }
            if !rest.starts_with("  ") {
                return Err(self
                    .error_current("expected two spaces between object entries on the same line"));
            }
            rest = &rest[2..];
            if rest.is_empty() {
                return Err(self.error_current("object lines cannot end with a separator"));
            }
        }
    }

    fn parse_value_after_key(
        &mut self,
        pair_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        self.skip_ignorable_lines()?;
        let child_indent = pair_indent + 2;
        let line = self
            .current_line()
            .ok_or_else(|| self.error_at_line(self.line, 1, "expected a nested value"))?
            .to_owned();
        self.ensure_line_has_no_tabs(self.line)?;
        let indent = count_leading_spaces(&line);
        let content = &line[indent..];
        if starts_with_marker_chain(content) && (indent == pair_indent || indent == child_indent) {
            return self.parse_marker_chain_line(content, indent);
        }
        if indent < child_indent {
            return Err(self.error_current("nested values must be indented by two spaces"));
        }
        let content = &line[child_indent..];
        if is_minimal_json_candidate(content) {
            let value = self.parse_minimal_json_line(content)?;
            self.line += 1;
            return Ok(value);
        }
        if self.looks_like_object_start(content) {
            self.parse_implicit_object(pair_indent)
        } else {
            self.parse_implicit_array(pair_indent)
        }
    }

    fn parse_standalone_scalar_line(
        &mut self,
        content: &str,
        line_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        if is_minimal_json_candidate(content) {
            let value = self.parse_minimal_json_line(content)?;
            self.line += 1;
            return Ok(value);
        }
        let (value, consumed) =
            self.parse_inline_value(content, line_indent, ArrayLineValueContext::SingleValue)?;
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
        elements: &mut Vec<TjsonValue>,
    ) -> std::result::Result<(), ParseError> {
        let elem_indent = parent_indent + 2;
        loop {
            self.skip_ignorable_lines()?;
            let Some(line) = self.current_line().map(str::to_owned) else {
                break;
            };
            self.ensure_line_has_no_tabs(self.line)?;
            let indent = count_leading_spaces(&line);
            let content = &line[indent..];
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
            let content = &line[elem_indent..];
            if content.is_empty() {
                return Err(self.error_current("blank lines are not valid inside arrays"));
            }
            if content.starts_with('|') {
                return Err(self.error_current("table arrays are only valid as the entire array"));
            }
            if is_minimal_json_candidate(content) {
                elements.push(self.parse_minimal_json_line(content)?);
                self.line += 1;
                continue;
            }
            self.parse_array_line_content(content, elem_indent, elements)?;
        }
        Ok(())
    }

    fn parse_array_line_content(
        &mut self,
        content: &str,
        elem_indent: usize,
        elements: &mut Vec<TjsonValue>,
    ) -> std::result::Result<(), ParseError> {
        let mut rest = content;
        let mut string_only_mode = false;
        loop {
            let (value, consumed) =
                self.parse_inline_value(rest, elem_indent, ArrayLineValueContext::ArrayLine)?;
            let is_string = matches!(value, TjsonValue::String(_));
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
    ) -> std::result::Result<TjsonValue, ParseError> {
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
        if rest.is_empty() {
            return Err(self.error_current("a nesting marker must be followed by content"));
        }
        let mut value = match *markers.last().unwrap() {
            ContainerKind::Array => {
                let deepest_parent_indent = line_indent + 2 * markers.len().saturating_sub(1);
                let mut elements = Vec::new();
                if is_minimal_json_candidate(rest) {
                    elements.push(self.parse_minimal_json_line(rest)?);
                    self.line += 1;
                } else {
                    self.parse_array_line_content(rest, deepest_parent_indent + 2, &mut elements)?;
                }
                self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                TjsonValue::Array(elements)
            }
            ContainerKind::Object => {
                let pair_indent = line_indent + 2 * markers.len();
                let mut entries = self.parse_object_line_content(rest, pair_indent)?;
                self.parse_object_tail(pair_indent, &mut entries)?;
                TjsonValue::Object(entries)
            }
        };
        for level in (0..markers.len().saturating_sub(1)).rev() {
            let parent_indent = line_indent + 2 * level;
            let mut wrapped = vec![value];
            self.parse_array_tail(parent_indent, &mut wrapped)?;
            value = TjsonValue::Array(wrapped);
        }
        Ok(value)
    }

    fn parse_key(&self, content: &str) -> std::result::Result<(String, usize), ParseError> {
        if let Some(end) = parse_bare_key_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((content[..end].to_owned(), end));
            }
        }
        if let Some((value, end)) = parse_json_string_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((value, end));
            }
        }
        Err(self.error_current("invalid object key"))
    }

    fn parse_inline_value(
        &mut self,
        content: &str,
        line_indent: usize,
        context: ArrayLineValueContext,
    ) -> std::result::Result<(TjsonValue, Option<usize>), ParseError> {
        let first = content
            .chars()
            .next()
            .ok_or_else(|| self.error_current("expected a value"))?;
        match first {
            ' ' => {
                if context == ArrayLineValueContext::ObjectValue {
                    if content.starts_with(" []") {
                        return Ok((TjsonValue::Array(Vec::new()), Some(3)));
                    }
                    if content.starts_with(" {}") {
                        return Ok((TjsonValue::Object(Vec::new()), Some(3)));
                    }
                    if let Some(rest) = content.strip_prefix("  ") {
                        let value = self.parse_inline_array(rest, line_indent)?;
                        return Ok((value, None));
                    }
                }
                if content.starts_with(" \"\"\"") {
                    let value = self.parse_multiline_string(content, line_indent)?;
                    return Ok((TjsonValue::String(value), None));
                }
                let end = bare_string_end(content, context);
                let value = &content[1..end];
                if !is_allowed_bare_string(value) {
                    return Err(self.error_current("invalid bare string"));
                }
                Ok((TjsonValue::String(value.to_owned()), Some(end)))
            }
            '"' => {
                if let Some((value, end)) = parse_json_string_prefix(content) {
                    return Ok((TjsonValue::String(value), Some(end)));
                }
                let value = self.parse_folded_json_string(content)?;
                Ok((TjsonValue::String(value), None))
            }
            '[' => {
                if content.starts_with("[]") {
                    return Ok((TjsonValue::Array(Vec::new()), Some(2)));
                }
                Err(self.error_current("nonempty arrays require container context"))
            }
            '{' => {
                if content.starts_with("{}") {
                    return Ok((TjsonValue::Object(Vec::new()), Some(2)));
                }
                Err(self.error_current("nonempty objects require object or array context"))
            }
            't' if content.starts_with("true") => Ok((TjsonValue::Bool(true), Some(4))),
            'f' if content.starts_with("false") => Ok((TjsonValue::Bool(false), Some(5))),
            'n' if content.starts_with("null") => Ok((TjsonValue::Null, Some(4))),
            '-' | '0'..='9' => {
                let end = simple_token_end(content, context);
                let token = &content[..end];
                if JsonNumber::from_str(token).is_err() {
                    return Err(self.error_current("invalid JSON number"));
                }
                Ok((TjsonValue::Number(token.to_owned()), Some(end)))
            }
            _ => Err(self.error_current("invalid value start")),
        }
    }

    fn parse_inline_array(
        &mut self,
        content: &str,
        parent_indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
        let mut values = Vec::new();
        self.parse_array_line_content(content, parent_indent + 2, &mut values)?;
        self.parse_array_tail(parent_indent, &mut values)?;
        Ok(TjsonValue::Array(values))
    }

    fn parse_multiline_string(
        &mut self,
        content: &str,
        line_indent: usize,
    ) -> std::result::Result<String, ParseError> {
        let suffix = content
            .strip_prefix(" \"\"\"")
            .ok_or_else(|| self.error_current("invalid multiline string opener"))?;
        let local_eol = match suffix {
            "" | "\\n" => MultilineLocalEol::Lf,
            "\\r\\n" => MultilineLocalEol::CrLf,
            _ => {
                return Err(self.error_current(
                    "multiline string opener only allows literal \\n or \\r\\n after \"\"\"",
                ));
            }
        };

        let mut value = String::new();
        self.line += 1;
        let continuation_indent = line_indent + 2;
        let mut native_line_count = 0usize;
        while let Some(line) = self.current_line().map(str::to_owned) {
            let indent = count_leading_spaces(&line);
            if indent < continuation_indent {
                let trimmed = &line[indent..];
                if indent >= line_indent && trimmed.starts_with("/ ") {
                    value.push_str(&trimmed[2..]);
                    self.line += 1;
                    continue;
                }
                if indent >= line_indent && trimmed.starts_with("\" ") {
                    if native_line_count > 0 {
                        value.push_str(local_eol.as_str());
                    }
                    value.push_str(&trimmed[2..]);
                    native_line_count += 1;
                    self.line += 1;
                    continue;
                }
                break;
            }
            if native_line_count > 0 {
                value.push_str(local_eol.as_str());
            }
            value.push_str(&line[continuation_indent..]);
            native_line_count += 1;
            self.line += 1;
        }
        if native_line_count < 2 {
            return Err(self.error_at_line(
                self.line,
                continuation_indent + 1,
                "multiline strings must contain at least one real linefeed",
            ));
        }
        Ok(value)
    }

    fn parse_folded_json_string(
        &mut self,
        content: &str,
    ) -> std::result::Result<String, ParseError> {
        let mut json = content.to_owned();
        self.line += 1;
        loop {
            let line = self
                .current_line()
                .ok_or_else(|| self.error_current("unterminated folded JSON string"))?
                .to_owned();
            self.ensure_line_has_no_tabs(self.line)?;
            let trimmed = line.trim_start_matches(' ');
            if !trimmed.starts_with("/ ") {
                return Err(self.error_current("folded JSON strings continue with '/ '"));
            }
            json.push_str(&trimmed[2..]);
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
    ) -> std::result::Result<TjsonValue, ParseError> {
        if !is_valid_minimal_json(content) {
            return Err(self.error_at_line(
                self.line,
                1,
                "invalid MINIMAL JSON (whitespace outside strings is forbidden)",
            ));
        }
        let value: JsonValue = serde_json::from_str(content)
            .map_err(|error| self.error_at_line(self.line, 1, error.to_string()))?;
        Ok(TjsonValue::from_json_value(value))
    }

    fn current_line(&self) -> Option<&str> {
        self.lines.get(self.line).map(String::as_str)
    }

    fn skip_ignorable_lines(&mut self) -> std::result::Result<(), ParseError> {
        while let Some(line) = self.current_line() {
            self.ensure_line_has_no_tabs(self.line)?;
            let trimmed = line.trim_start_matches(' ');
            if line.is_empty() || trimmed.starts_with("//") {
                self.line += 1;
                continue;
            }
            break;
        }
        Ok(())
    }

    fn ensure_line_has_no_tabs(&self, line_index: usize) -> std::result::Result<(), ParseError> {
        let Some(line) = self.lines.get(line_index) else {
            return Ok(());
        };
        if let Some(column) = line.find('\t') {
            return Err(self.error_at_line(
                line_index,
                column + 1,
                "tab characters are only allowed inside multiline strings",
            ));
        }
        Ok(())
    }

    fn looks_like_object_start(&self, content: &str) -> bool {
        if content.starts_with('|') || starts_with_marker_chain(content) {
            return false;
        }
        if let Some(end) = parse_bare_key_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return true;
            }
        }
        if let Some((_, end)) = parse_json_string_prefix(content) {
            return content.get(end..).is_some_and(|rest| rest.starts_with(':'));
        }
        false
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
        ParseError::new(line_index + 1, column, message)
    }
}

struct Renderer;

impl Renderer {
    fn render(value: &TjsonValue, options: &RenderOptions) -> Result<String> {
        let lines = Self::render_root(value, options, options.start_indent)?;
        Ok(lines.join("\n"))
    }

    fn render_root(
        value: &TjsonValue,
        options: &RenderOptions,
        start_indent: usize,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Null
            | TjsonValue::Bool(_)
            | TjsonValue::Number(_)
            | TjsonValue::String(_) => Ok(Self::render_scalar_lines(value, start_indent, options)?),
            TjsonValue::Array(values) if values.is_empty() => {
                Ok(Self::render_scalar_lines(value, start_indent, options)?)
            }
            TjsonValue::Object(entries) if entries.is_empty() => {
                Ok(Self::render_scalar_lines(value, start_indent, options)?)
            }
            TjsonValue::Array(values) if effective_force_markers(options) => {
                Self::render_explicit_array(values, start_indent, options)
            }
            TjsonValue::Array(values) => Self::render_implicit_array(values, start_indent, options),
            TjsonValue::Object(entries) if effective_force_markers(options) => {
                Self::render_explicit_object(entries, start_indent, options)
            }
            TjsonValue::Object(entries) => {
                Self::render_implicit_object(entries, start_indent, options)
            }
        }
    }

    fn render_implicit_object(
        entries: &[(String, TjsonValue)],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let pair_indent = parent_indent + 2;
        let mut lines = Vec::new();
        let mut packed_line = String::new();

        for (key, value) in entries {
            if effective_inline_objects(options) {
                if let Some(token) = Self::render_inline_object_token(key, value, options)? {
                    let candidate = if packed_line.is_empty() {
                        format!("{}{}", spaces(pair_indent), token)
                    } else {
                        format!("{packed_line}  {token}")
                    };
                    if packed_line.is_empty() || fits_wrap(options, &candidate) {
                        packed_line = candidate;
                        continue;
                    }
                    lines.push(std::mem::take(&mut packed_line));
                    packed_line = format!("{}{}", spaces(pair_indent), token);
                    continue;
                }
            }

            if !packed_line.is_empty() {
                lines.push(std::mem::take(&mut packed_line));
            }
            lines.extend(Self::render_object_entry(key, value, pair_indent, options)?);
        }

        if !packed_line.is_empty() {
            lines.push(packed_line);
        }
        Ok(lines)
    }

    fn render_object_entry(
        key: &str,
        value: &TjsonValue,
        pair_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let key_text = render_key(key, options);
        match value {
            TjsonValue::Array(values) if !values.is_empty() => {
                if effective_force_markers(options) {
                    let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                    lines.extend(Self::render_explicit_array(values, pair_indent, options)?);
                    return Ok(lines);
                }

                if effective_tables(options) {
                    if let Some(table_lines) = Self::render_table(values, pair_indent, options)? {
                        let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                        lines.extend(table_lines);
                        return Ok(lines);
                    }
                }

                if effective_inline_arrays(options) {
                    if let Some(lines) = Self::render_packed_array_lines(
                        values,
                        format!("{}{}:  ", spaces(pair_indent), key_text),
                        pair_indent + 2,
                        options,
                    )? {
                        return Ok(lines);
                    }
                }

                let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                if values.first().is_some_and(needs_explicit_array_marker) {
                    lines.extend(Self::render_explicit_array(
                        values,
                        pair_indent + 2,
                        options,
                    )?);
                } else {
                    lines.extend(Self::render_array_children(
                        values,
                        pair_indent + 2,
                        options,
                    )?);
                }
                Ok(lines)
            }
            TjsonValue::Object(entries) if !entries.is_empty() => {
                if effective_force_markers(options) {
                    let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                    lines.extend(Self::render_explicit_object(entries, pair_indent, options)?);
                    return Ok(lines);
                }

                let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                lines.extend(Self::render_implicit_object(entries, pair_indent, options)?);
                Ok(lines)
            }
            _ => {
                let scalar_lines = Self::render_scalar_lines(value, pair_indent, options)?;
                let mut iter = scalar_lines.into_iter();
                let first = iter
                    .next()
                    .ok_or_else(|| Error::Render("expected at least one scalar line".to_owned()))?;
                let mut lines = vec![format!(
                    "{}{}:{}",
                    spaces(pair_indent),
                    key_text,
                    &first[pair_indent..]
                )];
                lines.extend(iter);
                Ok(lines)
            }
        }
    }

    fn render_implicit_array(
        values: &[TjsonValue],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if effective_tables(options) {
            if let Some(lines) = Self::render_table(values, parent_indent, options)? {
                return Ok(lines);
            }
        }

        if effective_inline_arrays(options) {
            if let Some(lines) = Self::render_packed_array_lines(
                values,
                spaces(parent_indent + 2),
                parent_indent + 2,
                options,
            )? {
                return Ok(lines);
            }
        }

        let elem_indent = parent_indent + 2;
        let element_lines = values
            .iter()
            .map(|value| Self::render_array_element(value, elem_indent, options))
            .collect::<Result<Vec<_>>>()?;
        if values.first().is_some_and(needs_explicit_array_marker) {
            let mut lines = Vec::new();
            let first = &element_lines[0];
            let first_line = first.first().ok_or_else(|| {
                Error::Render("expected at least one array element line".to_owned())
            })?;
            let stripped = first_line.get(elem_indent..).ok_or_else(|| {
                Error::Render("failed to align the explicit outer array marker".to_owned())
            })?;
            lines.push(format!("{}[ {}", spaces(parent_indent), stripped));
            lines.extend(first.iter().skip(1).cloned());
            for extra in element_lines.iter().skip(1) {
                lines.extend(extra.clone());
            }
            Ok(lines)
        } else {
            Ok(element_lines.into_iter().flatten().collect())
        }
    }

    fn render_array_children(
        values: &[TjsonValue],
        elem_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        for value in values {
            lines.extend(Self::render_array_element(value, elem_indent, options)?);
        }
        Ok(lines)
    }

    fn render_explicit_array(
        values: &[TjsonValue],
        marker_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if effective_inline_arrays(options) {
            if let Some(lines) = Self::render_packed_array_lines(
                values,
                format!("{}[ ", spaces(marker_indent)),
                marker_indent + 2,
                options,
            )? {
                return Ok(lines);
            }
        }

        let elem_indent = marker_indent + 2;
        let mut element_lines = Vec::new();
        for value in values {
            element_lines.push(Self::render_array_element(value, elem_indent, options)?);
        }
        let first = element_lines
            .first()
            .ok_or_else(|| Error::Render("explicit arrays must be nonempty".to_owned()))?;
        let first_line = first
            .first()
            .ok_or_else(|| Error::Render("expected at least one explicit array line".to_owned()))?;
        let stripped = first_line
            .get(elem_indent..)
            .ok_or_else(|| Error::Render("failed to align an explicit array marker".to_owned()))?;
        let mut lines = vec![format!("{}[ {}", spaces(marker_indent), stripped)];
        lines.extend(first.iter().skip(1).cloned());
        for extra in element_lines.iter().skip(1) {
            lines.extend(extra.clone());
        }
        Ok(lines)
    }

    fn render_explicit_object(
        entries: &[(String, TjsonValue)],
        marker_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let pair_indent = marker_indent + 2;
        let implicit_lines = Self::render_implicit_object(entries, marker_indent, options)?;
        let first_line = implicit_lines.first().ok_or_else(|| {
            Error::Render("expected at least one explicit object line".to_owned())
        })?;
        let stripped = first_line
            .get(pair_indent..)
            .ok_or_else(|| Error::Render("failed to align an explicit object marker".to_owned()))?;
        let mut lines = vec![format!("{}{{ {}", spaces(marker_indent), stripped)];
        lines.extend(implicit_lines.into_iter().skip(1));
        Ok(lines)
    }

    fn render_array_element(
        value: &TjsonValue,
        elem_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Array(values) if !values.is_empty() => {
                Self::render_explicit_array(values, elem_indent, options)
            }
            TjsonValue::Object(entries) if !entries.is_empty() => {
                Self::render_explicit_object(entries, elem_indent, options)
            }
            _ => Self::render_scalar_lines(value, elem_indent, options),
        }
    }

    fn render_scalar_lines(
        value: &TjsonValue,
        indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Null => Ok(vec![format!("{}null", spaces(indent))]),
            TjsonValue::Bool(value) => Ok(vec![format!(
                "{}{}",
                spaces(indent),
                if *value { "true" } else { "false" }
            )]),
            TjsonValue::Number(value) => {
                if JsonNumber::from_str(value).is_err() {
                    return Err(Error::Render(format!("invalid JSON number: {value}")));
                }
                Ok(vec![format!("{}{}", spaces(indent), value)])
            }
            TjsonValue::String(value) => Self::render_string_lines(value, indent, options),
            TjsonValue::Array(values) => {
                if values.is_empty() {
                    Ok(vec![format!("{}[]", spaces(indent))])
                } else {
                    Err(Error::Render(
                        "nonempty arrays must be rendered through array context".to_owned(),
                    ))
                }
            }
            TjsonValue::Object(entries) => {
                if entries.is_empty() {
                    Ok(vec![format!("{}{{}}", spaces(indent))])
                } else {
                    Err(Error::Render(
                        "nonempty objects must be rendered through object or array context"
                            .to_owned(),
                    ))
                }
            }
        }
    }

    fn render_string_lines(
        value: &str,
        indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if value.is_empty() {
            return Ok(vec![format!("{}\"\"", spaces(indent))]);
        }
        if !options.canonical
            && !value.chars().any(is_forbidden_literal_tjson_char)
            && let Some(local_eol) = detect_multiline_local_eol(value)
        {
            let parts: Vec<&str> = match local_eol {
                MultilineLocalEol::Lf => value.split('\n').collect(),
                MultilineLocalEol::CrLf => value.split("\r\n").collect(),
            };
            let mut lines = vec![format!(
                "{} \"\"\"{}",
                spaces(indent),
                local_eol.opener_suffix()
            )];
            let continuation_indent = indent + 2;
            for part in parts {
                lines.push(format!("{}{}", spaces(continuation_indent), part));
            }
            if lines.len() < 3 {
                return Err(Error::Render(
                    "multiline strings must contain at least one real linefeed".to_owned(),
                ));
            }
            return Ok(lines);
        }
        if options.bare_strings == BareStyle::Prefer && is_allowed_bare_string(value) {
            return Ok(vec![format!("{} {}", spaces(indent), value)]);
        }
        Ok(vec![format!(
            "{}{}",
            spaces(indent),
            render_json_string(value)
        )])
    }

    fn render_inline_object_token(
        key: &str,
        value: &TjsonValue,
        options: &RenderOptions,
    ) -> Result<Option<String>> {
        let Some(value_text) = Self::render_scalar_token(value, options)? else {
            return Ok(None);
        };
        Ok(Some(format!("{}:{}", render_key(key, options), value_text)))
    }

    fn render_scalar_token(value: &TjsonValue, options: &RenderOptions) -> Result<Option<String>> {
        let rendered = match value {
            TjsonValue::Null => "null".to_owned(),
            TjsonValue::Bool(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            TjsonValue::Number(value) => {
                if JsonNumber::from_str(value).is_err() {
                    return Err(Error::Render(format!("invalid JSON number: {value}")));
                }
                value.clone()
            }
            TjsonValue::String(value) => {
                if value.contains('\n') || value.contains('\r') {
                    return Ok(None);
                }
                if options.bare_strings == BareStyle::Prefer && is_allowed_bare_string(value) {
                    format!(" {}", value)
                } else {
                    render_json_string(value)
                }
            }
            TjsonValue::Array(values) if values.is_empty() => "[]".to_owned(),
            TjsonValue::Object(entries) if entries.is_empty() => "{}".to_owned(),
            TjsonValue::Array(_) | TjsonValue::Object(_) => return Ok(None),
        };

        if options.canonical && rendered.contains('\n') {
            return Ok(None);
        }

        Ok(Some(rendered))
    }

    fn render_packed_array_lines(
        values: &[TjsonValue],
        first_prefix: String,
        continuation_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if values.is_empty() {
            return Ok(Some(vec![format!("{first_prefix}[]")]));
        }

        if values
            .iter()
            .all(|value| matches!(value, TjsonValue::String(_)))
        {
            return Self::render_string_array_lines(
                values,
                first_prefix,
                continuation_indent,
                options,
            );
        }

        let Some(tokens) = Self::render_packed_array_tokens(values, false, options)? else {
            return Ok(None);
        };
        Self::render_packed_token_lines(tokens, first_prefix, continuation_indent, false, options)
    }

    fn render_string_array_lines(
        values: &[TjsonValue],
        first_prefix: String,
        continuation_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        match options.string_array_style {
            StringArrayStyle::None => Ok(None),
            StringArrayStyle::Spaces => {
                let Some(tokens) = Self::render_packed_array_tokens(values, true, options)? else {
                    return Ok(None);
                };
                Self::render_packed_token_lines(
                    tokens,
                    first_prefix,
                    continuation_indent,
                    true,
                    options,
                )
            }
            StringArrayStyle::PreferSpaces => {
                let preferred = match Self::render_packed_array_tokens(values, true, options)? {
                    Some(tokens) => Self::render_packed_token_lines(
                        tokens,
                        first_prefix.clone(),
                        continuation_indent,
                        true,
                        options,
                    )?,
                    None => None,
                };
                let fallback = match Self::render_packed_array_tokens(values, false, options)? {
                    Some(tokens) => Self::render_packed_token_lines(
                        tokens,
                        first_prefix,
                        continuation_indent,
                        false,
                        options,
                    )?,
                    None => None,
                };
                Ok(pick_preferred_string_array_layout(
                    preferred, fallback, options,
                ))
            }
            StringArrayStyle::Comma => {
                let Some(tokens) = Self::render_packed_array_tokens(values, false, options)? else {
                    return Ok(None);
                };
                Self::render_packed_token_lines(
                    tokens,
                    first_prefix,
                    continuation_indent,
                    false,
                    options,
                )
            }
            StringArrayStyle::PreferComma => {
                let preferred = match Self::render_packed_array_tokens(values, false, options)? {
                    Some(tokens) => Self::render_packed_token_lines(
                        tokens,
                        first_prefix.clone(),
                        continuation_indent,
                        false,
                        options,
                    )?,
                    None => None,
                };
                let fallback = match Self::render_packed_array_tokens(values, true, options)? {
                    Some(tokens) => Self::render_packed_token_lines(
                        tokens,
                        first_prefix,
                        continuation_indent,
                        true,
                        options,
                    )?,
                    None => None,
                };
                Ok(pick_preferred_string_array_layout(
                    preferred, fallback, options,
                ))
            }
        }
    }

    fn render_packed_array_tokens(
        values: &[TjsonValue],
        string_spaces_mode: bool,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        let mut tokens = Vec::new();
        for value in values {
            let token = match value {
                TjsonValue::String(text) => {
                    if text.chars().any(is_comma_like) {
                        render_json_string(text)
                    } else {
                        let Some(token) = Self::render_scalar_token(value, options)? else {
                            return Ok(None);
                        };
                        token
                    }
                }
                _ => {
                    let Some(token) = Self::render_scalar_token(value, options)? else {
                        return Ok(None);
                    };
                    token
                }
            };
            if string_spaces_mode && !matches!(value, TjsonValue::String(_)) {
                return Ok(None);
            }
            tokens.push(token);
        }
        Ok(Some(tokens))
    }

    fn render_packed_token_lines(
        tokens: Vec<String>,
        first_prefix: String,
        continuation_indent: usize,
        string_spaces_mode: bool,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if tokens.is_empty() {
            return Ok(Some(vec![first_prefix]));
        }

        let separator = if string_spaces_mode { "  " } else { ", " };
        let continuation_prefix = spaces(continuation_indent);
        let mut iter = tokens.into_iter();
        let first = iter
            .next()
            .ok_or_else(|| Error::Render("expected at least one packed token".to_owned()))?;
        let mut current = format!("{first_prefix}{first}");
        let mut lines = Vec::new();

        for token in iter {
            let candidate = format!("{current}{separator}{token}");
            if fits_wrap(options, &candidate) {
                current = candidate;
                continue;
            }

            if !string_spaces_mode {
                current.push(',');
            }
            lines.push(current);
            current = format!("{continuation_prefix}{token}");
        }

        lines.push(current);
        Ok(Some(lines))
    }

    fn render_table(
        values: &[TjsonValue],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if values.len() < options.table_min_rows {
            return Ok(None);
        }

        let mut columns = Vec::<String>::new();
        let mut present_cells = 0usize;

        for value in values {
            let TjsonValue::Object(entries) = value else {
                return Ok(None);
            };
            present_cells += entries.len();
            for (key, cell) in entries {
                if matches!(cell, TjsonValue::Array(inner) if !inner.is_empty())
                    || matches!(cell, TjsonValue::Object(inner) if !inner.is_empty())
                    || matches!(cell, TjsonValue::String(text) if text.contains('\n') || text.contains('\r'))
                {
                    return Ok(None);
                }
                if !columns.iter().any(|column| column == key) {
                    columns.push(key.clone());
                }
            }
        }

        if columns.len() < options.table_min_cols {
            return Ok(None);
        }

        let similarity = present_cells as f32 / (values.len() * columns.len()) as f32;
        if similarity < options.table_similarity {
            return Ok(None);
        }

        let mut header_cells = Vec::new();
        let mut rows = Vec::new();
        for column in &columns {
            header_cells.push(render_key(column, options));
        }

        for value in values {
            let TjsonValue::Object(entries) = value else {
                return Ok(None);
            };
            let mut row: Vec<String> = Vec::new();
            for column in &columns {
                let token = if let Some((_, value)) = entries.iter().find(|(key, _)| key == column)
                {
                    Self::render_table_cell_token(value, options)?
                } else {
                    None
                };
                row.push(token.unwrap_or_default());
            }
            rows.push(row);
        }

        let mut widths = vec![0usize; columns.len()];
        for (index, header) in header_cells.iter().enumerate() {
            widths[index] = header.len();
        }
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(cell.len());
            }
        }
        for width in &mut widths {
            *width += 2;
        }

        let indent = spaces(parent_indent + 2);
        let mut lines = Vec::new();
        lines.push(format!(
            "{}{}",
            indent,
            header_cells
                .iter()
                .zip(widths.iter())
                .map(|(cell, width)| format!("|{cell:<width$}", width = *width))
                .collect::<String>()
                + "|"
        ));
        for row in rows {
            lines.push(format!(
                "{}{}",
                indent,
                row.iter()
                    .zip(widths.iter())
                    .map(|(cell, width)| format!("|{cell:<width$}", width = *width))
                    .collect::<String>()
                    + "|"
            ));
        }

        Ok(Some(lines))
    }

    fn render_table_cell_token(
        value: &TjsonValue,
        options: &RenderOptions,
    ) -> Result<Option<String>> {
        Ok(match value {
            TjsonValue::Null => Some("null".to_owned()),
            TjsonValue::Bool(value) => Some(if *value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }),
            TjsonValue::Number(value) => Some(value.clone()),
            TjsonValue::String(value) => {
                if value.contains('\n') || value.contains('\r') {
                    None
                } else if options.bare_strings == BareStyle::Prefer
                    && is_allowed_bare_string(value)
                    && !matches!(value.as_str(), "true" | "false" | "null")
                    && !value.contains('|')
                {
                    Some(format!(" {}", value))
                } else {
                    Some(render_json_string(value))
                }
            }
            TjsonValue::Array(values) if values.is_empty() => Some("[]".to_owned()),
            TjsonValue::Object(entries) if entries.is_empty() => Some("{}".to_owned()),
            _ => None,
        })
    }
}

fn normalize_input(input: &str) -> std::result::Result<String, ParseError> {
    let mut normalized = String::with_capacity(input.len());
    let mut line = 1;
    let mut column = 1;
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
                normalized.push('\n');
                line += 1;
                column = 1;
                continue;
            }
            return Err(ParseError::new(
                line,
                column,
                "bare carriage returns are not valid",
            ));
        }
        if is_forbidden_literal_tjson_char(ch) {
            return Err(ParseError::new(
                line,
                column,
                format!("forbidden character U+{:04X} must be escaped", ch as u32),
            ));
        }
        normalized.push(ch);
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Ok(normalized)
}

fn count_leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn spaces(count: usize) -> String {
    " ".repeat(count)
}

fn effective_inline_objects(options: &RenderOptions) -> bool {
    !options.canonical && options.inline_objects
}

fn effective_inline_arrays(options: &RenderOptions) -> bool {
    !options.canonical && options.inline_arrays
}

fn effective_force_markers(options: &RenderOptions) -> bool {
    !options.canonical && options.force_markers
}

fn effective_tables(options: &RenderOptions) -> bool {
    !options.canonical && options.tables
}

fn fits_wrap(options: &RenderOptions, line: &str) -> bool {
    match options.wrap_width {
        Some(0) | None => true,
        Some(width) => line.chars().count() <= width,
    }
}

fn pick_preferred_string_array_layout(
    preferred: Option<Vec<String>>,
    fallback: Option<Vec<String>>,
    options: &RenderOptions,
) -> Option<Vec<String>> {
    match (preferred, fallback) {
        (Some(preferred), Some(fallback))
            if string_array_layout_score(&fallback, options)
                < string_array_layout_score(&preferred, options) =>
        {
            Some(fallback)
        }
        (Some(preferred), _) => Some(preferred),
        (None, fallback) => fallback,
    }
}

fn string_array_layout_score(lines: &[String], options: &RenderOptions) -> (usize, usize, usize) {
    let overflow = match options.wrap_width {
        Some(0) | None => 0,
        Some(width) => lines
            .iter()
            .map(|line| line.chars().count().saturating_sub(width))
            .sum(),
    };
    let max_width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    (overflow, lines.len(), max_width)
}

fn starts_with_marker_chain(content: &str) -> bool {
    content.starts_with("[ ") || content.starts_with("{ ")
}

fn parse_json_string_prefix(content: &str) -> Option<(String, usize)> {
    if !content.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    let mut end = None;
    for (index, ch) in content.char_indices().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => {
                end = Some(index + 1);
                break;
            }
            '\n' | '\r' => return None,
            _ => {}
        }
    }
    let end = end?;
    let parsed = serde_json::from_str(&content[..end]).ok()?;
    Some((parsed, end))
}

fn split_pipe_cells(row: &str) -> Option<Vec<String>> {
    if !row.starts_with('|') {
        return None;
    }
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut escaped = false;

    for ch in row.chars() {
        if in_string {
            current.push(ch);
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                current.push(ch);
            }
            '|' => {
                cells.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }

    if in_string || escaped {
        return None;
    }

    cells.push(current);
    Some(cells)
}

fn is_minimal_json_candidate(content: &str) -> bool {
    let bytes = content.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    (bytes[0] == b'{' && bytes[1] != b'}' && bytes[1] != b' ')
        || (bytes[0] == b'[' && bytes[1] != b']' && bytes[1] != b' ')
}

fn is_valid_minimal_json(content: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;

    for ch in content.chars() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            ch if ch.is_whitespace() => return false,
            _ => {}
        }
    }

    !in_string && !escaped
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

fn detect_multiline_local_eol(value: &str) -> Option<MultilineLocalEol> {
    let bytes = value.as_bytes();
    let mut index = 0usize;
    let mut saw_lf = false;
    let mut saw_crlf = false;

    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    saw_crlf = true;
                    index += 2;
                } else {
                    return None;
                }
            }
            b'\n' => {
                saw_lf = true;
                index += 1;
            }
            _ => index += 1,
        }
    }

    match (saw_lf, saw_crlf) {
        (false, false) => None,
        (true, false) => Some(MultilineLocalEol::Lf),
        (false, true) => Some(MultilineLocalEol::CrLf),
        (true, true) => None,
    }
}

fn parse_bare_key_prefix(content: &str) -> Option<usize> {
    let mut chars = content.char_indices().peekable();
    let (_, first) = chars.next()?;
    if !is_unicode_letter_or_number(first) {
        return None;
    }
    let mut end = first.len_utf8();

    let mut previous_space = false;
    for (index, ch) in chars {
        if is_unicode_letter_or_number(ch)
            || matches!(
                ch,
                '_' | '(' | ')' | '/' | '\'' | '.' | '!' | '%' | '&' | ',' | '-'
            )
        {
            previous_space = false;
            end = index + ch.len_utf8();
            continue;
        }
        if ch == ' ' && !previous_space {
            previous_space = true;
            end = index + ch.len_utf8();
            continue;
        }
        break;
    }

    let candidate = &content[..end];
    let last = candidate.chars().next_back()?;
    if last == ' ' || is_comma_like(last) || is_quote_like(last) {
        return None;
    }
    Some(end)
}

fn render_key(key: &str, options: &RenderOptions) -> String {
    if options.bare_keys == BareStyle::Prefer
        && parse_bare_key_prefix(key).is_some_and(|end| end == key.len())
    {
        key.to_owned()
    } else {
        render_json_string(key)
    }
}

fn is_allowed_bare_string(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let first = value.chars().next().unwrap();
    let last = value.chars().next_back().unwrap();
    if first == ' '
        || last == ' '
        || first == '/'
        || is_quote_like(first)
        || is_quote_like(last)
        || is_comma_like(first)
        || is_comma_like(last)
    {
        return false;
    }
    let mut previous_space = false;
    for ch in value.chars() {
        if ch != ' ' && is_forbidden_bare_char(ch) {
            return false;
        }
        if ch == ' ' {
            if previous_space {
                return false;
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
    }
    true
}

fn needs_explicit_array_marker(value: &TjsonValue) -> bool {
    matches!(value, TjsonValue::Array(values) if !values.is_empty())
        || matches!(value, TjsonValue::Object(entries) if !entries.is_empty())
}

fn is_unicode_letter_or_number(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
    )
}

fn is_forbidden_literal_tjson_char(ch: char) -> bool {
    is_forbidden_control_char(ch)
        || is_default_ignorable_code_point(ch)
        || is_private_use_code_point(ch)
        || is_noncharacter_code_point(ch)
        || matches!(ch, '\u{2028}' | '\u{2029}')
}

fn is_forbidden_bare_char(ch: char) -> bool {
    if is_forbidden_literal_tjson_char(ch) {
        return true;
    }
    matches!(
        get_general_category(ch),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Unassigned
            | GeneralCategory::SpaceSeparator
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
            | GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

fn is_forbidden_control_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000B}'..='\u{000C}'
            | '\u{000E}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
    )
}

fn is_default_ignorable_code_point(ch: char) -> bool {
    matches!(get_general_category(ch), GeneralCategory::Format)
        || matches!(
            ch,
            '\u{034F}'
                | '\u{115F}'..='\u{1160}'
                | '\u{17B4}'..='\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FFA0}'
                | '\u{1BCA0}'..='\u{1BCA3}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'
                | '\u{E0001}'
                | '\u{E0020}'..='\u{E007F}'
                | '\u{E0100}'..='\u{E01EF}'
        )
}

fn is_private_use_code_point(ch: char) -> bool {
    matches!(get_general_category(ch), GeneralCategory::PrivateUse)
}

fn is_noncharacter_code_point(ch: char) -> bool {
    let code_point = ch as u32;
    (0xFDD0..=0xFDEF).contains(&code_point)
        || (code_point <= 0x10FFFF && (code_point & 0xFFFE) == 0xFFFE)
}

fn render_json_string(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for ch in value.chars() {
        match ch {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            '\u{0008}' => rendered.push_str("\\b"),
            '\u{000C}' => rendered.push_str("\\f"),
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            ch if ch <= '\u{001F}' || is_forbidden_literal_tjson_char(ch) => {
                push_json_unicode_escape(&mut rendered, ch);
            }
            _ => rendered.push(ch),
        }
    }
    rendered.push('"');
    rendered
}

fn push_json_unicode_escape(rendered: &mut String, ch: char) {
    let code_point = ch as u32;
    if code_point <= 0xFFFF {
        rendered.push_str(&format!("\\u{:04x}", code_point));
        return;
    }

    let scalar = code_point - 0x1_0000;
    let high = 0xD800 + ((scalar >> 10) & 0x3FF);
    let low = 0xDC00 + (scalar & 0x3FF);
    rendered.push_str(&format!("\\u{:04x}\\u{:04x}", high, low));
}

fn is_comma_like(ch: char) -> bool {
    matches!(ch, ',' | '\u{FF0C}' | '\u{FE50}')
}

fn is_quote_like(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::InitialPunctuation | GeneralCategory::FinalPunctuation
    ) || matches!(ch, '"' | '\'' | '`')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(input: &str) -> JsonValue {
        serde_json::from_str(input).unwrap()
    }

    fn tjson_value(input: &str) -> TjsonValue {
        TjsonValue::from_json_value(json(input))
    }

    #[test]
    fn parses_basic_scalar_examples() {
        assert_eq!(
            parse_str("null").unwrap().to_json_value_lossy().unwrap(),
            json("null")
        );
        assert_eq!(
            parse_str("5").unwrap().to_json_value_lossy().unwrap(),
            json("5")
        );
        assert_eq!(
            parse_str(" a").unwrap().to_json_value_lossy().unwrap(),
            json("\"a\"")
        );
        assert_eq!(
            parse_str("[]").unwrap().to_json_value_lossy().unwrap(),
            json("[]")
        );
        assert_eq!(
            parse_str("{}").unwrap().to_json_value_lossy().unwrap(),
            json("{}")
        );
    }

    #[test]
    fn parses_comments_and_marker_examples() {
        let input = "// comment\n  a:5\n// comment\n  x:\n    [ [ 1\n      { b: text";
        let expected = json("{\"a\":5,\"x\":[[1],{\"b\":\"text\"}]}");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_json_string_example() {
        let input =
            "\"foldingat\n/ onlyafew\\r\\n\n/ characters\n/ hereusing\n/ somejson\n/ escapes\\\\\"";
        let expected = json("\"foldingatonlyafew\\r\\ncharactershereusingsomejsonescapes\\\\\"");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_multiline_string_example() {
        let input = "  note: \"\"\"\n    first\n    second\n      indented";
        let expected = json("{\"note\":\"first\\nsecond\\n  indented\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_marked_multiline_string_with_explicit_lf_indicator() {
        let input = " \"\"\"\\n\n\" first\n\" second";
        let expected = json("\"first\\nsecond\"");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_crlf_multiline_string_example() {
        let input = "  note: \"\"\"\\r\\n\n    first\n    second\n      indented";
        let expected = json("{\"note\":\"first\\r\\nsecond\\r\\n  indented\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_table_array_example() {
        let input = "  |a  |b   |c      |\n  |1  | x  |true   |\n  |2  | y  |false  |\n  |3  | z  |null   |";
        let expected = json(
            "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
        );
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_minimal_json_inside_array_example() {
        let input = "  [{\"a\":{\"b\":null},\"c\":3}]";
        let expected = json("[[{\"a\":{\"b\":null},\"c\":3}]]");
        assert_eq!(
            parse_str(input).unwrap().to_json_value_lossy().unwrap(),
            expected
        );
    }

    #[test]
    fn renders_basic_scalar_examples() {
        assert_eq!(render_string(&tjson_value("null")).unwrap(), "null");
        assert_eq!(render_string(&tjson_value("5")).unwrap(), "5");
        assert_eq!(render_string(&tjson_value("\"a\"")).unwrap(), " a");
        assert_eq!(render_string(&tjson_value("[]")).unwrap(), "[]");
        assert_eq!(render_string(&tjson_value("{}")).unwrap(), "{}");
    }

    #[test]
    fn renders_multiline_string_example() {
        let rendered =
            render_string(&tjson_value("{\"note\":\"first\\nsecond\\n  indented\"}")).unwrap();
        assert_eq!(
            rendered,
            "  note: \"\"\"\n    first\n    second\n      indented"
        );
    }

    #[test]
    fn renders_crlf_multiline_string_example() {
        let rendered = render_string(&tjson_value(
            "{\"note\":\"first\\r\\nsecond\\r\\n  indented\"}",
        ))
        .unwrap();
        assert_eq!(
            rendered,
            "  note: \"\"\"\\r\\n\n    first\n    second\n      indented"
        );
    }

    #[test]
    fn mixed_newlines_fall_back_to_json_string() {
        let rendered =
            render_string(&tjson_value("{\"note\":\"first\\r\\nsecond\\nthird\"}")).unwrap();
        assert_eq!(rendered, "  note:\"first\\r\\nsecond\\nthird\"");
    }

    #[test]
    fn escapes_forbidden_characters_in_json_strings() {
        let rendered = render_string(&tjson_value("{\"note\":\"a\\u200Db\"}")).unwrap();
        assert_eq!(rendered, "  note:\"a\\u200db\"");
    }

    #[test]
    fn forbidden_characters_force_multiline_fallback_to_json_string() {
        let rendered = render_string(&tjson_value("{\"lines\":\"x\\ny\\u200Dz\"}")).unwrap();
        assert_eq!(rendered, "  lines:\"x\\ny\\u200dz\"");
    }

    #[test]
    fn rejects_raw_forbidden_characters() {
        let input = format!("  note:\"a{}b\"", '\u{200D}');
        let error = parse_str(&input).unwrap_err();
        assert!(error.to_string().contains("U+200D"));
    }

    #[test]
    fn renders_table_when_eligible() {
        let value = tjson_value(
            "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
        );
        let rendered = render_string(&value).unwrap();
        assert_eq!(
            rendered,
            "  |a  |b   |c      |\n  |1  | x  |true   |\n  |2  | y  |false  |\n  |3  | z  |null   |"
        );
    }

    #[test]
    fn renders_table_for_array_object_values() {
        let value = tjson_value(
            "{\"people\":[{\"name\":\"Alice\",\"age\":30,\"active\":true},{\"name\":\"Bob\",\"age\":25,\"active\":false},{\"name\":\"Carol\",\"age\":35,\"active\":true}]}",
        );
        let rendered = render_string(&value).unwrap();
        assert_eq!(
            rendered,
            "  people:\n    |name    |age  |active  |\n    | Alice  |30   |true    |\n    | Bob    |25   |false   |\n    | Carol  |35   |true    |"
        );
    }

    #[test]
    fn packs_explicit_nested_arrays_and_objects() {
        let value = tjson_value(
            "{\"nested\":[[1,2],[3,4]],\"rows\":[{\"a\":1,\"b\":2},{\"c\":3,\"d\":4}]}",
        );
        let rendered = render_string(&value).unwrap();
        assert_eq!(
            rendered,
            "  nested:\n    [ [ 1, 2\n      [ 3, 4\n  rows:\n    [ { a:1  b:2\n      { c:3  d:4"
        );
    }

    #[test]
    fn wraps_long_packed_arrays_before_falling_back_to_multiline() {
        let value =
            tjson_value("{\"data\":[100,200,300,400,500,600,700,800,900,1000,1100,1200,1300]}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                wrap_width: Some(40),
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "  data:  100, 200, 300, 400, 500, 600,\n    700, 800, 900, 1000, 1100, 1200,\n    1300"
        );
    }

    #[test]
    fn default_string_array_style_is_prefer_comma() {
        let value = tjson_value("{\"items\":[\"alpha\",\"beta\",\"gamma\"]}");
        let rendered = render_string(&value).unwrap();
        assert_eq!(rendered, "  items:   alpha,  beta,  gamma");
    }

    #[test]
    fn bare_strings_none_quotes_single_line_strings() {
        let value = tjson_value("{\"greeting\":\"hello world\",\"items\":[\"alpha\",\"beta\"]}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                bare_strings: BareStyle::None,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "  greeting:\"hello world\"\n  items:  \"alpha\", \"beta\""
        );
        let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
        assert_eq!(reparsed, value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn bare_keys_none_quotes_keys_in_objects_and_tables() {
        let object_value = tjson_value("{\"alpha\":1,\"beta key\":2}");
        let rendered_object = render_string_with_options(
            &object_value,
            RenderOptions {
                bare_keys: BareStyle::None,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered_object, "  \"alpha\":1  \"beta key\":2");

        let table_value = tjson_value(
            "{\"rows\":[{\"alpha\":1,\"beta\":2},{\"alpha\":3,\"beta\":4},{\"alpha\":5,\"beta\":6}]}",
        );
        let rendered_table = render_string_with_options(
            &table_value,
            RenderOptions {
                bare_keys: BareStyle::None,
                table_min_cols: 2,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered_table,
            "  \"rows\":\n    |\"alpha\"  |\"beta\"  |\n    |1        |2       |\n    |3        |4       |\n    |5        |6       |"
        );
        let reparsed = parse_str(&rendered_table)
            .unwrap()
            .to_json_value_lossy()
            .unwrap();
        assert_eq!(reparsed, table_value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn force_markers_applies_to_root_and_key_nested_single_levels() {
        let value =
            tjson_value("{\"a\":5,\"6\":\"fred\",\"xy\":[],\"de\":{},\"e\":[1],\"o\":{\"k\":2}}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                force_markers: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "{ a:5  6: fred  xy:[]  de:{}\n  e:\n  [ 1\n  o:\n  { k:2"
        );
        let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
        assert_eq!(reparsed, value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn force_markers_applies_to_root_arrays() {
        let value = tjson_value("[1,2,3]");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                force_markers: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "[ 1, 2, 3");
        let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
        assert_eq!(reparsed, value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn force_markers_suppresses_table_rendering_for_array_containers() {
        let value = tjson_value("[{\"a\":1,\"b\":2},{\"a\":3,\"b\":4},{\"a\":5,\"b\":6}]");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                force_markers: true,
                table_min_cols: 2,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "[ { a:1  b:2\n  { a:3  b:4\n  { a:5  b:6");
        assert!(!rendered.contains('|'));
    }

    #[test]
    fn string_array_style_spaces_forces_space_packing() {
        let value = tjson_value("{\"items\":[\"alpha\",\"beta\",\"gamma\"]}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                string_array_style: StringArrayStyle::Spaces,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "  items:   alpha   beta   gamma");
    }

    #[test]
    fn string_array_style_none_disables_string_array_packing() {
        let value = tjson_value("{\"items\":[\"alpha\",\"beta\",\"gamma\"]}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                string_array_style: StringArrayStyle::None,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "  items:\n     alpha\n     beta\n     gamma");
    }

    #[test]
    fn prefer_comma_can_fall_back_to_spaces_when_wrap_is_cleaner() {
        let value = tjson_value("{\"items\":[\"aa\",\"bb\",\"cc\"]}");
        let comma = render_string_with_options(
            &value,
            RenderOptions {
                string_array_style: StringArrayStyle::Comma,
                wrap_width: Some(18),
                ..RenderOptions::default()
            },
        )
        .unwrap();
        let prefer_comma = render_string_with_options(
            &value,
            RenderOptions {
                string_array_style: StringArrayStyle::PreferComma,
                wrap_width: Some(18),
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(comma, "  items:   aa,  bb,\n     cc");
        assert_eq!(prefer_comma, "  items:   aa   bb\n     cc");
    }

    #[test]
    fn quotes_comma_strings_in_packed_arrays_so_they_round_trip() {
        let value = tjson_value("{\"items\":[\"apples, oranges\",\"pears, plums\",\"grapes\"]}");
        let rendered = render_string(&value).unwrap();
        assert_eq!(
            rendered,
            "  items:  \"apples, oranges\", \"pears, plums\",  grapes"
        );
        let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
        assert_eq!(reparsed, value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn spaces_style_quotes_comma_strings_and_round_trips() {
        let value = tjson_value("{\"items\":[\"apples, oranges\",\"pears, plums\"]}");
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                string_array_style: StringArrayStyle::Spaces,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "  items:  \"apples, oranges\"  \"pears, plums\"");
        let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
        assert_eq!(reparsed, value.to_json_value_lossy().unwrap());
    }

    #[test]
    fn canonical_rendering_disables_tables_and_inline_packing() {
        let value = tjson_value(
            "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
        );
        let rendered = render_string_with_options(
            &value,
            RenderOptions {
                canonical: true,
                ..RenderOptions::default()
            },
        )
        .unwrap();
        assert!(!rendered.contains('|'));
        assert!(!rendered.contains(", "));
    }

    #[test]
    fn round_trips_generated_examples() {
        let values = [
            json("{\"a\":5,\"6\":\"fred\",\"xy\":[],\"de\":{},\"e\":[1]}"),
            json("{\"nested\":[[1],[2,3],{\"x\":\"y\"}],\"empty\":[],\"text\":\"plain english\"}"),
            json("{\"note\":\"first\\nsecond\\n  indented\"}"),
            json(
                "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
            ),
        ];
        for value in values {
            let rendered = render_string(&TjsonValue::from_json_value(value.clone())).unwrap();
            let reparsed = parse_str(&rendered).unwrap().to_json_value_lossy().unwrap();
            assert_eq!(reparsed, value);
        }
    }

    #[test]
    fn keeps_key_order_at_the_ast_and_json_boundary() {
        let input = "  first:1\n  second:2\n  third:3";
        let value = parse_str(input).unwrap();
        match &value {
            TjsonValue::Object(entries) => {
                let keys = entries
                    .iter()
                    .map(|(key, _)| key.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(keys, vec!["first", "second", "third"]);
            }
            other => panic!("expected an object, found {other:?}"),
        }
        let json = value.to_json_value_lossy().unwrap();
        let keys = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["first", "second", "third"]);
    }

    #[test]
    fn duplicate_keys_are_localized_to_the_json_boundary() {
        let input = "  dup:1\n  dup:2\n  keep:3";
        let value = parse_str(input).unwrap();
        match &value {
            TjsonValue::Object(entries) => assert_eq!(entries.len(), 3),
            other => panic!("expected an object, found {other:?}"),
        }
        let json_value = value.to_json_value_lossy().unwrap();
        assert_eq!(json_value, json("{\"dup\":2,\"keep\":3}"));
    }
}
