use crate::error::{Error, Result};
use crate::options::{BareStyle, FoldStyle, IndentGlyphMarkerStyle, IndentGlyphMode, MultilineStyle, StringArrayStyle, TableUnindentStyle, RenderOptions, MIN_FOLD_CONTINUATION, indent_glyph_mode};
use crate::value::{Entry, Value};
use crate::util::*;
use crate::parse::{MultilineLocalEol, detect_multiline_local_eol};

fn effective_inline_objects(options: &RenderOptions) -> bool {
    options.inline_objects
}

fn effective_inline_arrays(options: &RenderOptions) -> bool {
    options.inline_arrays
}

fn effective_force_markers(options: &RenderOptions) -> bool {
    options.force_markers
}

fn effective_tables(options: &RenderOptions) -> bool {
    options.tables
}

// Returns the target parent_indent to re-render the table at when /< /> glyphs should be
// used, or None if no unindenting should occur.
//
// `natural_lines` are the table lines as rendered at pair_indent (spaces(pair_indent+2) prefix).
fn table_unindent_target(pair_indent: usize, natural_lines: &[String], options: &RenderOptions) -> Option<usize> {
    let n = pair_indent;
    let max_natural = natural_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    // data_width: widest line with the natural indent stripped
    let data_width = max_natural.saturating_sub(n + 2);

    match options.table_unindent_style {
        TableUnindentStyle::None => None,

        TableUnindentStyle::Left => {
            // Always push to indent 0, unless already there.
            if n == 0 { None } else {
                // Check it fits at 0 (data_width <= w, or unlimited width).
                let fits = options.wrap_width.map(|w| data_width <= w).unwrap_or(true);
                if fits { Some(0) } else { None }
            }
        }

        TableUnindentStyle::Auto => {
            // Push to indent 0 only when table overflows at natural indent.
            // With unlimited width, never unindent.
            let w = options.wrap_width?;
            let overflows_natural = max_natural > w;
            let fits_at_zero = data_width <= w;
            if overflows_natural && fits_at_zero { Some(0) } else { None }
        }

        TableUnindentStyle::Floating => {
            // Push left by the minimum amount needed to fit within wrap_width.
            // With unlimited width, never unindent.
            let w = options.wrap_width?;
            if max_natural <= w {
                return None; // already fits, no need to move
            }
            // Find the minimum parent_indent such that data_width + (parent_indent + 2) <= w.
            // data_width is fixed; we need parent_indent + 2 + data_width <= w.
            // minimum parent_indent = 0 if data_width + 2 <= w, else can't help.
            if data_width + 2 <= w {
                // Find smallest parent_indent that makes table fit.
                let target = w.saturating_sub(data_width + 2);
                // Only unindent if it actually reduces the indent.
                if target < n { Some(target) } else { None }
            } else {
                None // table too wide even at indent 0
            }
        }
    }
}

/// Approximate number of output lines a value will produce. Used for glyph volume estimation.
/// Empty arrays and objects count as 1 (simple values); non-empty containers recurse.
fn subtree_line_count(value: &Value) -> usize {
    match value {
        Value::Array(v) if !v.is_empty() => v.iter().map(subtree_line_count).sum::<usize>() + 1,
        Value::Object(e) if !e.is_empty() => {
            e.iter().map(|entry| subtree_line_count(&entry.value) + 1).sum()
        }
        _ => 1,
    }
}

/// Rough count of content bytes in a subtree. Used to weight volume in `ByteWeighted` mode.
fn subtree_byte_count(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Number(n) => n.to_string().len(),
        Value::Bool(b) => if *b { 4 } else { 5 },
        Value::Null => 4,
        Value::Array(v) => v.iter().map(subtree_byte_count).sum(),
        Value::Object(e) => e.iter().map(|entry| entry.key.len() + subtree_byte_count(&entry.value)).sum(),
    }
}

/// Maximum nesting depth of non-empty containers below this value.
/// Empty arrays/objects count as 0 (simple values).
fn subtree_max_depth(value: &Value) -> usize {
    match value {
        Value::Array(v) if !v.is_empty() => {
            1 + v.iter().map(subtree_max_depth).max().unwrap_or(0)
        }
        Value::Object(e) if !e.is_empty() => {
            1 + e.iter().map(|entry| subtree_max_depth(&entry.value)).max().unwrap_or(0)
        }
        _ => 0,
    }
}

/// Returns true if a `/<` indent-offset glyph should be emitted for `value` at `pair_indent`.
fn should_use_indent_glyph(value: &Value, pair_indent: usize, options: &RenderOptions) -> bool {
    let Some(w) = options.wrap_width else { return false; };
    let fold_floor = || {
        let max_depth = subtree_max_depth(value);
        pair_indent + max_depth * 2 >= w.saturating_sub(MIN_FOLD_CONTINUATION + 2)
    };
    match indent_glyph_mode(options) {
        IndentGlyphMode::None => false,
        IndentGlyphMode::Fixed => pair_indent >= w / 2,
        IndentGlyphMode::IndentWeighted(threshold) => {
            if fold_floor() { return true; }
            let line_count = subtree_line_count(value);
            (pair_indent * line_count) as f64 >= threshold * (w * w) as f64
        }
        IndentGlyphMode::ByteWeighted(threshold) => {
            if fold_floor() { return true; }
            let byte_count = subtree_byte_count(value);
            (pair_indent * byte_count) as f64 >= threshold * (w * w) as f64
        }
    }
}

/// Build the opening glyph line(s) for an indent-offset block.
/// Returns either `["key: /<"]` or `["key:", "INDENT /<"]` depending on options.
fn indent_glyph_open_lines(key_line: &str, pair_indent: usize, options: &RenderOptions) -> Vec<String> {
    match options.indent_glyph_marker_style {
        IndentGlyphMarkerStyle::Compact => vec![format!("{}: /<", key_line)],
        IndentGlyphMarkerStyle::Separate /*| IndentGlyphMarkerStyle::Marked*/ => vec![
            format!("{}:", key_line),
            format!("{} /<", spaces(pair_indent)),
        ],
    }
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

struct StringArrayLayoutScore {
    overflow: usize,
    line_count: usize,
    max_width: usize,
}

impl PartialOrd for StringArrayLayoutScore {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StringArrayLayoutScore {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.overflow, self.line_count, self.max_width)
            .cmp(&(other.overflow, other.line_count, other.max_width))
    }
}

impl PartialEq for StringArrayLayoutScore {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for StringArrayLayoutScore {}

fn string_array_layout_score(lines: &[String], options: &RenderOptions) -> StringArrayLayoutScore {
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
    StringArrayLayoutScore { overflow, line_count: lines.len(), max_width }
}


pub(crate) fn render_key(key: &str, options: &RenderOptions) -> String {
    if options.bare_keys == BareStyle::Prefer
        && parse_bare_key_prefix(key).is_some_and(|end| end == key.len())
    {
        key.to_owned()
    } else {
        render_json_string(key)
    }
}


pub(crate) fn needs_explicit_array_marker(value: &Value) -> bool {
    matches!(value, Value::Array(values) if !values.is_empty())
        || matches!(value, Value::Object(entries) if !entries.is_empty())
}


fn split_multiline_fold(text: &str, avail: usize, style: FoldStyle) -> Vec<&str> {
    if text.len() <= avail || avail == 0 {
        return vec![text];
    }
    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        if rest.len() <= avail {
            segments.push(rest);
            break;
        }
        let split_at = match style {
            FoldStyle::Auto => {
                // Find the last space before avail that is not a single consecutive space
                // (spec: bare strings may not fold immediately after a single space, but
                // multiline folds are within the body text so we just prefer spaces).
                let candidate = &rest[..avail.min(rest.len())];
                // Find last space boundary
                if let Some(pos) = candidate.rfind(' ') {
                    if pos > 0 { pos } else { avail.min(rest.len()) }
                } else {
                    avail.min(rest.len())
                }
            }
            FoldStyle::Fixed | FoldStyle::None => avail.min(rest.len()),
        };
        // Don't split mid-escape-sequence (keep `\x` pairs together)
        // Find the actual safe split point: walk back if we're in the middle of `\x`
        let safe = safe_json_split(rest, split_at);
        segments.push(&rest[..safe]);
        rest = &rest[safe..];
        if rest.is_empty() {
            break;
        }
    }
    segments
}

/// Find the last safe byte position to split a JSON-encoded string, not mid-escape.
/// `split_at` is the desired split position. May return a smaller value if `split_at`
/// would land in the middle of a `\uXXXX` or `\X` escape.
fn fold_bare_string(
    value: &str,
    indent: usize,
    first_line_extra: usize,
    style: FoldStyle,
    wrap_width: Option<usize>,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    // First-line budget: indent + 1 (space before bare string) + first_line_extra + content
    // first_line_extra accounts for any key+colon prefix on the same line.
    let first_avail = w.saturating_sub(indent + 1 + first_line_extra);
    if value.len() <= first_avail {
        return None; // fits on one line, no fold needed
    }
    // Continuation budget: indent + 2 (`/ ` prefix) + content
    let cont_avail = w.saturating_sub(indent + 2);
    if cont_avail < MIN_FOLD_CONTINUATION {
        return None; // too little room for useful continuation content
    }
    let mut lines = Vec::new();
    let mut rest = value;
    let mut first = true;
    let avail = if first { first_avail } else { cont_avail };
    let _ = avail;
    let mut current_avail = first_avail;
    loop {
        if rest.is_empty() {
            break;
        }
        if rest.len() <= current_avail {
            if first {
                lines.push(format!("{} {}", spaces(indent), rest));
            } else {
                lines.push(format!("{}/ {}", spaces(indent), rest));
            }
            break;
        }
        // Find a fold point
        let split_at = match style {
            FoldStyle::Auto => {
                // Spec: "a bare string may never be folded immediately after a single
                // consecutive space." Find last space boundary that isn't after a lone space.
                let candidate = &rest[..current_avail.min(rest.len())];
                let lookahead = rest[candidate.len()..].chars().next();
                find_bare_fold_point(candidate, lookahead)
            }
            FoldStyle::Fixed | FoldStyle::None => current_avail.min(rest.len()),
        };
        let split_at = if split_at == 0 && !first && matches!(style, FoldStyle::Auto) {
            // No good boundary found on a continuation line — fall back to a hard cut.
            current_avail.min(rest.len())
        } else if split_at == 0 {
            // No fold point on the first line, or Fixed/None style — emit remainder as-is.
            if first {
                lines.push(format!("{} {}", spaces(indent), rest));
            } else {
                lines.push(format!("{}/ {}", spaces(indent), rest));
            }
            break;
        } else {
            split_at
        };
        let segment = &rest[..split_at];
        if first {
            lines.push(format!("{} {}", spaces(indent), segment));
            first = false;
        } else {
            lines.push(format!("{}/ {}", spaces(indent), segment));
        }
        rest = &rest[split_at..];
        current_avail = cont_avail;
    }
    if lines.len() <= 1 {
        None // only produced one line, no actual fold
    } else {
        Some(lines)
    }
}

/// Fold a bare key (no leading space) into multiple continuation lines.
/// The caller must append `:` to the last returned line.
/// Returns None if no fold is needed, impossible, or style is None.
fn fold_bare_key(
    key: &str,
    pair_indent: usize,
    style: FoldStyle,
    wrap_width: Option<usize>,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    if matches!(style, FoldStyle::None) { return None; }
    // key + colon fits — no fold needed
    if key.len() < w.saturating_sub(pair_indent) { return None; }
    let first_avail = w.saturating_sub(pair_indent);
    let cont_avail = w.saturating_sub(pair_indent + 2); // `/ ` prefix
    if cont_avail < MIN_FOLD_CONTINUATION { return None; }
    let ind = spaces(pair_indent);
    let mut lines: Vec<String> = Vec::new();
    let mut rest = key;
    let mut first = true;
    let mut current_avail = first_avail;
    loop {
        if rest.is_empty() { break; }
        if rest.len() <= current_avail {
            lines.push(if first { format!("{}{}", ind, rest) } else { format!("{}/ {}", ind, rest) });
            break;
        }
        let split_at = match style {
            FoldStyle::Auto => {
                let candidate = &rest[..current_avail.min(rest.len())];
                let lookahead = rest[candidate.len()..].chars().next();
                find_bare_fold_point(candidate, lookahead)
            }
            FoldStyle::Fixed | FoldStyle::None => current_avail.min(rest.len()),
        };
        if split_at == 0 {
            lines.push(if first { format!("{}{}", ind, rest) } else { format!("{}/ {}", ind, rest) });
            break;
        }
        lines.push(if first { format!("{}{}", ind, &rest[..split_at]) } else { format!("{}/ {}", ind, &rest[..split_at]) });
        rest = &rest[split_at..];
        first = false;
        current_avail = cont_avail;
    }
    if lines.len() <= 1 { None } else { Some(lines) }
}

/// Find a fold point in a number string at or before `avail` bytes.
/// Auto mode: prefers splitting before `.` or `e`/`E` (keeping the semantic marker with the
/// continuation); falls back to splitting between any two digits at the limit.
/// Returns a byte offset (1..avail), or 0 if no valid point found.
fn fold_number(
    value: &str,
    indent: usize,
    first_line_extra: usize,
    style: FoldStyle,
    wrap_width: Option<usize>,
) -> Option<Vec<String>> {
    if matches!(style, FoldStyle::None) {
        return None;
    }
    let w = wrap_width?;
    let first_avail = w.saturating_sub(indent + first_line_extra);
    if value.len() <= first_avail {
        return None; // fits on one line
    }
    let cont_avail = w.saturating_sub(indent + 2);
    if cont_avail < MIN_FOLD_CONTINUATION {
        return None;
    }
    let auto_mode = matches!(style, FoldStyle::Auto);
    let mut lines: Vec<String> = Vec::new();
    let mut rest = value;
    let mut current_avail = first_avail;
    let ind = spaces(indent);
    loop {
        if rest.len() <= current_avail {
            lines.push(format!("{}{}", ind, rest));
            break;
        }
        let split_at = find_number_fold_point(rest, current_avail, auto_mode);
        if split_at == 0 {
            lines.push(format!("{}{}", ind, rest));
            break;
        }
        lines.push(format!("{}{}", ind, &rest[..split_at]));
        rest = &rest[split_at..];
        current_avail = cont_avail;
        // Subsequent lines use "/ " prefix
        let last = lines.last_mut().unwrap();
        // First line has no prefix adjustment; continuation lines need "/ " prefix.
        // Restructure: first push was the segment, now we need to wrap in continuation format.
        // Actually build correctly from the start:
        // → rebuild: first line is plain, continuations are "/ segment"
        // We already pushed the first segment above — fix continuation format below.
        let _ = last; // handled in next iteration via prefix logic
    }
    // The above loop pushes segments without "/ " prefix on continuations. Rebuild properly.
    // Simpler: redo with explicit first/rest tracking.
    lines.clear();
    let mut rest = value;
    let mut first = true;
    let mut current_avail = first_avail;
    loop {
        if rest.len() <= current_avail {
            if first {
                lines.push(format!("{}{}", ind, rest));
            } else {
                lines.push(format!("{}/ {}", ind, rest));
            }
            break;
        }
        let split_at = find_number_fold_point(rest, current_avail, auto_mode);
        if split_at == 0 {
            if first {
                lines.push(format!("{}{}", ind, rest));
            } else {
                lines.push(format!("{}/ {}", ind, rest));
            }
            break;
        }
        if first {
            lines.push(format!("{}{}", ind, &rest[..split_at]));
            first = false;
        } else {
            lines.push(format!("{}/ {}", ind, &rest[..split_at]));
        }
        rest = &rest[split_at..];
        current_avail = cont_avail;
    }
    Some(lines)
}

/// Character class used by [`find_bare_fold_point`] to assign break priorities.
fn fold_json_string(
    value: &str,
    indent: usize,
    first_line_extra: usize,
    style: FoldStyle,
    wrap_width: Option<usize>,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    let encoded = render_json_string(value);
    // First-line budget: indent + first_line_extra + content (the encoded string including quotes)
    let first_avail = w.saturating_sub(indent + first_line_extra);
    if encoded.len() <= first_avail {
        return None; // fits on one line
    }
    let cont_avail = w.saturating_sub(indent + 2);
    if cont_avail < MIN_FOLD_CONTINUATION {
        return None; // too little room for useful continuation content
    }
    // The encoded string starts with `"` and ends with `"`.
    // We strip the outer quotes and work with the raw encoded content.
    let inner = &encoded[1..encoded.len() - 1]; // strip opening and closing `"`
    let mut lines: Vec<String> = Vec::new();
    let mut rest = inner;
    let mut first = true;
    let mut current_avail = first_avail.saturating_sub(1); // -1 for the opening `"`
    loop {
        if rest.is_empty() {
            // Close the string: add closing `"` to the last line
            if let Some(last) = lines.last_mut() {
                last.push('"');
            }
            break;
        }
        // Adjust avail: first line has opening `"` (-1), last segment needs closing `"` (-1)
        let segment_avail = if rest.len() <= current_avail {
            // Last segment: needs room for closing `"`
            current_avail.saturating_sub(1)
        } else {
            current_avail
        };
        if rest.len() <= segment_avail {
            let segment = rest;
            if first {
                lines.push(format!("{}\"{}\"", spaces(indent), segment));
            } else {
                lines.push(format!("{}/ {}\"", spaces(indent), segment));
            }
            break;
        }
        // Find fold point
        let split_at = match style {
            FoldStyle::Auto => {
                let candidate = &rest[..segment_avail.min(rest.len())];
                // Prefer to split before a space run (spec: "fold BEFORE unescaped space runs")
                find_json_fold_point(candidate)
            }
            FoldStyle::Fixed | FoldStyle::None => {
                safe_json_split(rest, segment_avail.min(rest.len()))
            }
        };
        if split_at == 0 {
            // Can't fold cleanly — emit rest as final segment
            if first {
                lines.push(format!("{}\"{}\"", spaces(indent), rest));
            } else {
                lines.push(format!("{}/ {}\"", spaces(indent), rest));
            }
            break;
        }
        let segment = &rest[..split_at];
        if first {
            lines.push(format!("{}\"{}\"", spaces(indent), segment));
            // Fix: first line should NOT have closing quote yet
            let last = lines.last_mut().unwrap();
            last.pop(); // remove the premature closing `"`
            first = false;
        } else {
            lines.push(format!("{}/ {}", spaces(indent), segment));
        }
        rest = &rest[split_at..];
        current_avail = cont_avail;
    }
    if lines.len() <= 1 {
        None
    } else {
        Some(lines)
    }
}

/// Count consecutive backslashes immediately before `pos` in `bytes`.
fn render_folding_quotes(value: &str, indent: usize, options: &RenderOptions) -> Vec<String> {
    let ind = spaces(indent);
    let pieces: Vec<&str> = value.split('\n').collect();
    // Encode each piece's inner content (no outer quotes, no \n — we add \n explicitly).
    let mut lines: Vec<String> = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        let is_last = i == pieces.len() - 1;
        let encoded = render_json_string(piece);
        let inner = &encoded[1..encoded.len() - 1]; // strip outer quotes
        let nl = if is_last { "" } else { "\\n" };
        if i == 0 {
            lines.push(format!("{}\"{}{}", ind, inner, nl));
            if !is_last {
                // No closing quote yet — string continues on next line
            } else {
                lines.last_mut().unwrap().push('"');
            }
        } else if is_last {
            lines.push(format!("{}/ {}\"", ind, inner));
        } else {
            lines.push(format!("{}/ {}{}", ind, inner, nl));
        }
        // Width-fold within this piece if the line is still too wide
        // and string_multiline_fold_style is not None.
        if !matches!(options.string_multiline_fold_style, FoldStyle::None)
            && let Some(w) = options.wrap_width {
                let last = lines.last().unwrap();
                if last.len() > w {
                    // The piece itself overflows; leave it long — within-piece folding
                    // of JSON strings mid-escape is not safe to split here.
                    // Future: could re-fold the piece using fold_json_string.
                }
            }
    }
    lines
}

/// Split a rendered table row line for a fold continuation.
/// The fold must happen within a cell's string value, between the first and last
/// data character (spec: "between the first data character... and the last data character").
/// Returns `(before_fold, after_fold)` or `None` if no valid fold point is found.
enum PackedToken {
    /// A flat inline token string (number, null, bool, short string, empty array/object).
    /// Also carries the original value for lone-overflow fold fallback.
    Inline(String, Value),
    /// A block element (multiline string, nonempty array, nonempty object) that interrupts
    /// packing. Carries the original value; rendered lazily at the right continuation indent.
    Block(Value),
}

pub(crate) struct Renderer;

impl Renderer {
    pub(crate) fn render(value: &Value, options: &RenderOptions) -> Result<String> {
        let lines = Self::render_root(value, options, options.start_indent)?;
        Ok(lines.join("\n"))
    }

    fn render_root(
        value: &Value,
        options: &RenderOptions,
        start_indent: usize,
    ) -> Result<Vec<String>> {
        match value {
            Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_) => Ok(Self::render_scalar_lines(value, start_indent, options)?),
            Value::Array(values) if values.is_empty() => {
                Ok(Self::render_scalar_lines(value, start_indent, options)?)
            }
            Value::Object(entries) if entries.is_empty() => {
                Ok(Self::render_scalar_lines(value, start_indent, options)?)
            }
            Value::Array(values) if effective_force_markers(options) => {
                Self::render_explicit_array(values, start_indent, options)
            }
            Value::Array(values) => Self::render_implicit_array(values, start_indent, options),
            Value::Object(entries) if effective_force_markers(options) => {
                Self::render_explicit_object(entries, start_indent, options)
            }
            Value::Object(entries) => {
                Self::render_implicit_object(entries, start_indent, options)
            }
        }
    }

    fn render_implicit_object(
        entries: &[Entry],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let pair_indent = parent_indent + 2;
        let mut lines = Vec::new();
        let mut packed_line = String::new();

        for Entry { key, value } in entries {
            if effective_inline_objects(options)
                && let Some(token) = Self::render_inline_object_token(key, value, options)? {
                    let candidate = if packed_line.is_empty() {
                        format!("{}{}", spaces(pair_indent), token)
                    } else {
                        format!("{packed_line}{}{token}", spaces(options.kv_pack_multiple * 2))
                    };
                    if fits_wrap(options, &candidate) {
                        packed_line = candidate;
                        continue;
                    }
                    if !packed_line.is_empty() {
                        lines.push(std::mem::take(&mut packed_line));
                    }
                    // First entry or wrap exceeded: fall through to render_object_entry
                    // so folding and other per-entry logic can apply.
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
        value: &Value,
        pair_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let is_bare = options.bare_keys == BareStyle::Prefer
            && parse_bare_key_prefix(key).is_some_and(|end| end == key.len());
        let key_text = render_key(key, options);

        let key_fold_enabled = if is_bare {
            options.string_bare_fold_style != FoldStyle::None
        } else {
            options.string_quoted_fold_style != FoldStyle::None
        };

        // Key fold lines — last line gets ":" appended before the value.
        // Bare keys use string_bare_fold_style; quoted keys use string_quoted_fold_style.
        // Only the first (standalone) key on a line is ever folded; inline-packed keys
        // are not candidates (they are rendered via render_inline_object_token, not here).
        let key_fold: Option<Vec<String>> =
            if is_bare && options.string_bare_fold_style != FoldStyle::None {
                fold_bare_key(&key_text, pair_indent, options.string_bare_fold_style, options.wrap_width)
            } else if !is_bare && options.string_quoted_fold_style != FoldStyle::None {
                fold_json_string(key, pair_indent, 0, options.string_quoted_fold_style, options.wrap_width)
            } else {
                None
            };

        if let Some(mut fold_lines) = key_fold {
            // Key itself folds across multiple lines. Determine available space on the last fold
            // line (after appending ":") and attach the value there or as a fold continuation.
            let last_fold_line = fold_lines.last().unwrap();
            // last_fold_line is like "  / lastpart" — pair_indent + "/ " + content.
            // Available width after appending ":" = wrap_width - last_fold_line.len() - 1
            let after_colon_avail = options.wrap_width
                .map(|w| w.saturating_sub(last_fold_line.len() + 1))
                .unwrap_or(usize::MAX);

            let normal = Self::render_object_entry_body(&key_text, value, pair_indent, key_fold_enabled, options)?;
            let key_prefix = format!("{}{}:", spaces(pair_indent), key_text);
            let suffix = normal[0].strip_prefix(&key_prefix).unwrap_or("").to_owned();

            // Check if the value suffix fits on the last fold line, or needs its own continuation
            if suffix.is_empty() || after_colon_avail >= suffix.len() {
                // Value fits (or is empty: non-scalar like arrays/objects start on the next line)
                let last = fold_lines.pop().unwrap();
                fold_lines.push(format!("{}:{}", last, suffix));
                fold_lines.extend(normal.into_iter().skip(1));
            } else {
                // Value doesn't fit on the last key fold line — fold after colon
                let cont_lines = Self::render_scalar_value_continuation_lines(value, pair_indent, options)?;
                let last = fold_lines.pop().unwrap();
                fold_lines.push(format!("{}:", last));
                let first_cont = &cont_lines[0][pair_indent..];
                fold_lines.push(format!("{}/ {}", spaces(pair_indent), first_cont));
                fold_lines.extend(cont_lines.into_iter().skip(1));
            }
            return Ok(fold_lines);
        }

        Self::render_object_entry_body(&key_text, value, pair_indent, key_fold_enabled, options)
    }

    /// Render a scalar value's lines for use as fold-after-colon continuation(s).
    /// The first line uses `first_line_extra = 2` (the "/ " prefix overhead) so that
    /// content is correctly fitted to `wrap_width - pair_indent - 2 - (leading space if bare)`.
    /// The caller prefixes the first element's content (after stripping `pair_indent`) with "/ ".
    fn render_scalar_value_continuation_lines(
        value: &Value,
        pair_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            Value::String(s) => Self::render_string_lines(s, pair_indent, 2, options),
            Value::Number(n) => {
                let ns = n.to_string();
                if let Some(folds) = fold_number(&ns, pair_indent, 2, options.number_fold_style, options.wrap_width) {
                    Ok(folds)
                } else {
                    Ok(vec![format!("{}{}", spaces(pair_indent), ns)])
                }
            }
            _ => Self::render_scalar_lines(value, pair_indent, options),
        }
    }

    fn render_object_entry_body(
        key_text: &str,
        value: &Value,
        pair_indent: usize,
        key_fold_enabled: bool,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            Value::Array(values) if !values.is_empty() => {
                if effective_tables(options)
                    && let Some(table_lines) = Self::render_table(values, pair_indent, options)? {
                        if let Some(target_indent) = table_unindent_target(pair_indent, &table_lines, options) {
                            let Some(offset_lines) = Self::render_table(values, target_indent, options)? else {
                                return Err(Error::Render(
                                    "table eligible at natural indent failed to re-render at offset indent".into(),
                                ));
                            };
                            let key_line = format!("{}{}", spaces(pair_indent), key_text);
                            let mut lines = indent_glyph_open_lines(&key_line, pair_indent, options);
                            if effective_force_markers(options) {
                                let elem_indent = target_indent + 2;
                                let first = offset_lines.first().ok_or_else(|| Error::Render("empty table".into()))?;
                                let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".into()))?;
                                lines.push(format!("{}[ {}", spaces(target_indent), stripped));
                                lines.extend(offset_lines.into_iter().skip(1));
                            } else {
                                lines.extend(offset_lines);
                            }
                            lines.push(format!("{} />", spaces(pair_indent)));
                            return Ok(lines);
                        }
                        let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                        if effective_force_markers(options) {
                            let elem_indent = pair_indent + 2;
                            let first = table_lines.first().ok_or_else(|| Error::Render("empty table".into()))?;
                            let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".into()))?;
                            lines.push(format!("{}[ {}", spaces(pair_indent), stripped));
                            lines.extend(table_lines.into_iter().skip(1));
                        } else {
                            lines.extend(table_lines);
                        }
                        return Ok(lines);
                    }

                if should_use_indent_glyph(value, pair_indent, options) {
                    let key_line = format!("{}{}", spaces(pair_indent), key_text);
                    let mut lines = indent_glyph_open_lines(&key_line, pair_indent, options);
                    if values.first().is_some_and(needs_explicit_array_marker) {
                        lines.extend(Self::render_explicit_array(values, 2, options)?);
                    } else {
                        lines.extend(Self::render_array_children(values, 2, options)?);
                    }
                    lines.push(format!("{} />", spaces(pair_indent)));
                    return Ok(lines);
                }

                if effective_inline_arrays(options) {
                    let all_simple = values.iter().all(|v| match v {
                        Value::Array(a) => a.is_empty(),
                        Value::Object(o) => o.is_empty(),
                        _ => true,
                    });
                    if all_simple
                        && let Some(lines) = Self::render_packed_array_lines(
                            values,
                            format!("{}{}:  ", spaces(pair_indent), key_text),
                            pair_indent + 2,
                            options,
                        )? {
                            return Ok(lines);
                        }
                }

                let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                if values.first().is_some_and(needs_explicit_array_marker) || effective_force_markers(options) {
                    lines.extend(Self::render_explicit_array(
                        values,
                        pair_indent,
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
            Value::Object(entries) if !entries.is_empty() => {
                if should_use_indent_glyph(value, pair_indent, options) {
                    let key_line = format!("{}{}", spaces(pair_indent), key_text);
                    let mut lines = indent_glyph_open_lines(&key_line, pair_indent, options);
                    lines.extend(Self::render_implicit_object(entries, 0, options)?);
                    lines.push(format!("{} />", spaces(pair_indent)));
                    return Ok(lines);
                }

                let mut lines = vec![format!("{}{}:", spaces(pair_indent), key_text)];
                if effective_force_markers(options) {
                    lines.extend(Self::render_explicit_object(entries, pair_indent, options)?);
                } else {
                    lines.extend(Self::render_implicit_object(entries, pair_indent, options)?);
                }
                Ok(lines)
            }
            _ => {
                let scalar_lines = if let Value::String(s) = value {
                    Self::render_string_lines(s, pair_indent, key_text.len() + 1, options)?
                } else {
                    Self::render_scalar_lines(value, pair_indent, options)?
                };
                let first = scalar_lines[0].clone();
                let value_suffix = &first[pair_indent..]; // " value" for bare string, "value" for others

                // Check if "key: value" assembled first line overflows wrap_width.
                // If so, and key fold is enabled, fold after the colon: key on its own line,
                // value as a "/ " continuation at pair_indent.
                let assembled_len = pair_indent + key_text.len() + 1 + value_suffix.len();
                if key_fold_enabled
                    && let Some(w) = options.wrap_width
                        && assembled_len > w {
                            let cont_lines = Self::render_scalar_value_continuation_lines(value, pair_indent, options)?;
                            let key_line = format!("{}{}:", spaces(pair_indent), key_text);
                            let first_cont = &cont_lines[0][pair_indent..];
                            let mut lines = vec![key_line, format!("{}/ {}", spaces(pair_indent), first_cont)];
                            lines.extend(cont_lines.into_iter().skip(1));
                            return Ok(lines);
                        }

                let mut lines = vec![format!(
                    "{}{}:{}",
                    spaces(pair_indent),
                    key_text,
                    value_suffix
                )];
                lines.extend(scalar_lines.into_iter().skip(1));
                Ok(lines)
            }
        }
    }

    fn render_implicit_array(
        values: &[Value],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if effective_tables(options)
            && let Some(lines) = Self::render_table(values, parent_indent, options)? {
                return Ok(lines);
            }

        if effective_inline_arrays(options) && !values.first().is_some_and(needs_explicit_array_marker)
            && let Some(lines) = Self::render_packed_array_lines(
                values,
                spaces(parent_indent + 2),
                parent_indent + 2,
                options,
            )? {
                return Ok(lines);
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
                Error::Render("expected at least one array element line".into())
            })?;
            let stripped = first_line.get(elem_indent..).ok_or_else(|| {
                Error::Render("failed to align the explicit outer array marker".into())
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
        values: &[Value],
        elem_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        let table_row_prefix = format!("{}|", spaces(elem_indent));
        for value in values {
            let prev_was_table = lines.last().map(|l: &String| l.starts_with(&table_row_prefix)).unwrap_or(false);
            let elem_lines = Self::render_array_element(value, elem_indent, options)?;
            let curr_is_table = elem_lines.first().map(|l| l.starts_with(&table_row_prefix)).unwrap_or(false);
            if prev_was_table && curr_is_table {
                // Two consecutive tables: the second needs a `[ ` marker to separate them.
                let first = elem_lines.first().unwrap();
                let stripped = &first[elem_indent..]; // e.g. "|col  |..."
                lines.push(format!("{}[ {}", spaces(elem_indent.saturating_sub(2)), stripped));
                lines.extend(elem_lines.into_iter().skip(1));
            } else {
                lines.extend(elem_lines);
            }
        }
        Ok(lines)
    }

    fn render_explicit_array(
        values: &[Value],
        marker_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if effective_tables(options)
            && let Some(lines) = Self::render_table(values, marker_indent, options)? {
                // Always prepend "[ " — render_explicit_array always needs its marker,
                // whether the elements render as a table or in any other form.
                let elem_indent = marker_indent + 2;
                let first = lines.first().ok_or_else(|| Error::Render("empty table".into()))?;
                let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".into()))?;
                let mut out = vec![format!("{}[ {}", spaces(marker_indent), stripped)];
                out.extend(lines.into_iter().skip(1));
                return Ok(out);
            }

        if effective_inline_arrays(options)
            && let Some(lines) = Self::render_packed_array_lines(
                values,
                format!("{}[ ", spaces(marker_indent)),
                marker_indent + 2,
                options,
            )? {
                return Ok(lines);
            }

        let elem_indent = marker_indent + 2;
        let mut element_lines = Vec::new();
        for value in values {
            element_lines.push(Self::render_array_element(value, elem_indent, options)?);
        }
        let first = element_lines
            .first()
            .ok_or_else(|| Error::Render("explicit arrays must be nonempty".into()))?;
        let first_line = first
            .first()
            .ok_or_else(|| Error::Render("expected at least one explicit array line".into()))?;
        let stripped = first_line
            .get(elem_indent..)
            .ok_or_else(|| Error::Render("failed to align an explicit array marker".into()))?;
        let mut lines = vec![format!("{}[ {}", spaces(marker_indent), stripped)];
        lines.extend(first.iter().skip(1).cloned());
        for extra in element_lines.iter().skip(1) {
            lines.extend(extra.clone());
        }
        Ok(lines)
    }

    fn render_explicit_object(
        entries: &[Entry],
        marker_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        let pair_indent = marker_indent + 2;
        let implicit_lines = Self::render_implicit_object(entries, marker_indent, options)?;
        let first_line = implicit_lines.first()
            .ok_or_else(|| Error::Render("expected at least one explicit object line".into()))?;
        let stripped = first_line
            .get(pair_indent..)
            .ok_or_else(|| Error::Render("failed to align an explicit object marker".into()))?;
        let mut lines = vec![format!("{}{{ {}", spaces(marker_indent), stripped)];
        lines.extend(implicit_lines.into_iter().skip(1));
        Ok(lines)
    }

    fn render_array_element(
        value: &Value,
        elem_indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            Value::Array(values) if !values.is_empty() => {
                if should_use_indent_glyph(value, elem_indent, options) {
                    let mut lines = vec![format!("{} /<", spaces(elem_indent))];
                    if values.first().is_some_and(needs_explicit_array_marker) {
                        lines.extend(Self::render_explicit_array(values, 0, options)?);
                    } else {
                        lines.extend(Self::render_array_children(values, 0, options)?);
                    }
                    lines.push(format!("{} />", spaces(elem_indent)));
                    return Ok(lines);
                }
                Self::render_explicit_array(values, elem_indent, options)
            }
            Value::Object(entries) if !entries.is_empty() => {
                Self::render_explicit_object(entries, elem_indent, options)
            }
            _ => Self::render_scalar_lines(value, elem_indent, options),
        }
    }

    fn render_scalar_lines(
        value: &Value,
        indent: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        match value {
            Value::Null => Ok(vec![format!("{}null", spaces(indent))]),
            Value::Bool(value) => Ok(vec![format!(
                "{}{}",
                spaces(indent),
                if *value { "true" } else { "false" }
            )]),
            Value::Number(value) => {
                let s = value.to_string();
                if let Some(lines) = fold_number(&s, indent, 0, options.number_fold_style, options.wrap_width) {
                    return Ok(lines);
                }
                Ok(vec![format!("{}{}", spaces(indent), s)])
            }
            Value::String(value) => Self::render_string_lines(value, indent, 0, options),
            Value::Array(values) => {
                if values.is_empty() {
                    Ok(vec![format!("{}[]", spaces(indent))])
                } else {
                    Err(Error::Render(
                        "nonempty arrays must be rendered through array context".into(),
                    ))
                }
            }
            Value::Object(entries) => {
                if entries.is_empty() {
                    Ok(vec![format!("{}{{}}", spaces(indent))])
                } else {
                    Err(Error::Render(
                        "nonempty objects must be rendered through object or array context".into(),
                    ))
                }
            }
        }
    }

    fn render_string_lines(
        value: &str,
        indent: usize,
        first_line_extra: usize,
        options: &RenderOptions,
    ) -> Result<Vec<String>> {
        if value.is_empty() {
            return Ok(vec![format!("{}\"\"", spaces(indent))]);
        }
        // FoldingQuotes: for EOL-containing strings, always use folded JSON string —
        // checked before the multiline block so it short-circuits even if multiline_strings=false.
        if matches!(options.multiline_style, MultilineStyle::FoldingQuotes)
            && detect_multiline_local_eol(value).is_some()
        {
            return Ok(render_folding_quotes(value, indent, options));
        }

        if options.multiline_strings
            && !value.chars().any(is_forbidden_literal_tjson_char)
            && let Some(local_eol) = detect_multiline_local_eol(value)
        {
            let suffix = local_eol.opener_suffix();
            let parts: Vec<&str> = match local_eol {
                MultilineLocalEol::Lf => value.split('\n').collect(),
                MultilineLocalEol::CrLf => value.split("\r\n").collect(),
            };
            let min_eols = options.multiline_min_lines.max(1);
            // parts.len() - 1 == number of EOLs in value
            if parts.len().saturating_sub(1) >= min_eols {
                let fold_style = options.string_multiline_fold_style;
                let wrap = options.wrap_width;

                // Content safety checks shared across all styles
                let pipe_heavy = {
                    let pipe_count = parts
                        .iter()
                        .filter(|p| line_starts_with_ws_then(p, '|'))
                        .count();
                    !parts.is_empty() && pipe_count * 10 > parts.len()
                };
                let backtick_start = parts.iter().any(|p| line_starts_with_ws_then(p, '`'));
                let forced_bold = pipe_heavy || backtick_start;

                // Whether any content line overflows wrap_width at indent+2
                let overflows_at_natural = wrap
                    .map(|w| parts.iter().any(|p| indent + 2 + p.len() > w))
                    .unwrap_or(false);

                // Whether line count exceeds the configured maximum
                let too_many_lines = options.multiline_max_lines > 0
                    && parts.len() > options.multiline_max_lines;

                let bold = |body_indent: usize| {
                    Self::render_multiline_double_backtick(
                        &parts, indent, body_indent, suffix, fold_style, wrap,
                    )
                };

                return Ok(match options.multiline_style {
                    MultilineStyle::Floating => {
                        // Fall back to `` when content is unsafe OR would exceed width/line-count
                        if forced_bold || overflows_at_natural || too_many_lines {
                            bold(0)
                        } else {
                            Self::render_multiline_single_backtick(
                                &parts, indent, suffix, fold_style, wrap,
                            )
                        }
                    }
                    MultilineStyle::Light => {
                        // Fall back to `` only when content looks like TJSON markers (pipe-heavy /
                        // backtick-starting). Width overflow and line count do NOT trigger fallback —
                        // Light prefers a long ` over a heavy ``.
                        if forced_bold {
                            bold(0)
                        } else {
                            Self::render_multiline_single_backtick(
                                &parts, indent, suffix, fold_style, wrap,
                            )
                        }
                    }
                    MultilineStyle::Bold => bold(0),
                    MultilineStyle::BoldFloating => {
                        let body = if forced_bold || overflows_at_natural { 0 } else { indent };
                        bold(body)
                    }
                    MultilineStyle::Transparent => {
                        if forced_bold {
                            bold(0)
                        } else {
                            Self::render_multiline_triple_backtick(&parts, indent, suffix)
                        }
                    }
                    MultilineStyle::FoldingQuotes => unreachable!(),
                });
            }
        }
        if options.bare_strings == BareStyle::Prefer && is_allowed_bare_string(value) {
            if options.string_bare_fold_style != FoldStyle::None
                && let Some(lines) =
                    fold_bare_string(value, indent, first_line_extra, options.string_bare_fold_style, options.wrap_width)
                {
                    return Ok(lines);
                }
            return Ok(vec![format!("{} {}", spaces(indent), value)]);
        }
        if options.string_quoted_fold_style != FoldStyle::None
            && let Some(lines) =
                fold_json_string(value, indent, first_line_extra, options.string_quoted_fold_style, options.wrap_width)
            {
                return Ok(lines);
            }
        Ok(vec![format!("{}{}", spaces(indent), render_json_string(value))])
    }

    /// Render a multiline string using ` (single backtick, unmarked body at indent+2).
    /// Body lines are at indent+2. Fold continuations (if enabled) at indent.
    /// No folding is allowed when fold_style is None.
    fn render_multiline_single_backtick(
        parts: &[&str],
        indent: usize,
        suffix: &str,
        fold_style: FoldStyle,
        wrap_width: Option<usize>,
    ) -> Vec<String> {
        let glyph = format!("{} `{}", spaces(indent), suffix);
        let body_indent = indent + 2;
        let fold_prefix = format!("{}/ ", spaces(indent));
        let avail = wrap_width.map(|w| w.saturating_sub(body_indent));
        let mut lines = vec![glyph.clone()];
        for part in parts {
            if fold_style != FoldStyle::None
                && let Some(avail_w) = avail
                    && part.len() > avail_w {
                        let segments = split_multiline_fold(part, avail_w, fold_style);
                        let mut first = true;
                        for seg in segments {
                            if first {
                                lines.push(format!("{}{}", spaces(body_indent), seg));
                                first = false;
                            } else {
                                lines.push(format!("{}{}", fold_prefix, seg));
                            }
                        }
                        continue;
                    }
            lines.push(format!("{}{}", spaces(body_indent), part));
        }
        lines.push(glyph);
        lines
    }

    /// Render a multiline string using `` (double backtick, pipe-guarded body).
    /// Body lines are at body_indent with `| ` prefix. Fold continuations at body_indent-2.
    fn render_multiline_double_backtick(
        parts: &[&str],
        indent: usize,
        body_indent: usize,
        suffix: &str,
        fold_style: FoldStyle,
        wrap_width: Option<usize>,
    ) -> Vec<String> {
        let glyph = format!("{} ``{}", spaces(indent), suffix);
        let fold_prefix = format!("{}/ ", spaces(body_indent.saturating_sub(2)));
        // Available width for body content: wrap_width minus the `| ` prefix (2 chars) and body_indent
        let avail = wrap_width.map(|w| w.saturating_sub(body_indent + 2));
        let mut lines = vec![glyph.clone()];
        for part in parts {
            if fold_style != FoldStyle::None
                && let Some(avail_w) = avail
                    && part.len() > avail_w {
                        let segments = split_multiline_fold(part, avail_w, fold_style);
                        let mut first = true;
                        for seg in segments {
                            if first {
                                lines.push(format!("{}| {}", spaces(body_indent), seg));
                                first = false;
                            } else {
                                lines.push(format!("{}{}", fold_prefix, seg));
                            }
                        }
                        continue;
                    }
            lines.push(format!("{}| {}", spaces(body_indent), part));
        }
        lines.push(glyph);
        lines
    }

    /// Render a multiline string using ``` (triple backtick, body at col 0).
    /// No folding is allowed in ``` format per spec.
    /// Currently not invoked by the default selection heuristic; available for explicit use.
    #[allow(dead_code)]
    fn render_multiline_triple_backtick(parts: &[&str], indent: usize, suffix: &str) -> Vec<String> {
        let glyph = format!("{} ```{}", spaces(indent), suffix);
        let mut lines = vec![glyph.clone()];
        for part in parts {
            lines.push((*part).to_owned());
        }
        lines.push(glyph);
        lines
    }

    fn render_inline_object_token(
        key: &str,
        value: &Value,
        options: &RenderOptions,
    ) -> Result<Option<String>> {
        let Some(value_text) = Self::render_scalar_token(value, options)? else {
            return Ok(None);
        };
        Ok(Some(format!("{}:{}", render_key(key, options), value_text)))
    }

    fn render_scalar_token(value: &Value, options: &RenderOptions) -> Result<Option<String>> {
        let rendered = match value {
            Value::Null => "null".to_owned(),
            Value::Bool(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            Value::Number(value) => value.to_string(),
            Value::String(value) => {
                if value.contains('\n') || value.contains('\r') {
                    return Ok(None);
                }
                if options.bare_strings == BareStyle::Prefer && is_allowed_bare_string(value) {
                    format!(" {}", value)
                } else {
                    render_json_string(value)
                }
            }
            Value::Array(values) if values.is_empty() => "[]".to_owned(),
            Value::Object(entries) if entries.is_empty() => "{}".to_owned(),
            Value::Array(_) | Value::Object(_) => return Ok(None),
        };

        Ok(Some(rendered))
    }

    fn render_packed_array_lines(
        values: &[Value],
        first_prefix: String,
        continuation_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if values.is_empty() {
            return Ok(Some(vec![format!("{first_prefix}[]")]));
        }

        if values
            .iter()
            .all(|value| matches!(value, Value::String(_)))
        {
            return Self::render_string_array_lines(
                values,
                first_prefix,
                continuation_indent,
                options,
            );
        }

        let tokens = Self::render_packed_array_tokens(values, options)?;
        Self::render_packed_token_lines(tokens, first_prefix, continuation_indent, false, options)
    }

    fn render_string_array_lines(
        values: &[Value],
        first_prefix: String,
        continuation_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        match options.string_array_style {
            StringArrayStyle::None => Ok(None),
            StringArrayStyle::Spaces => {
                let tokens = Self::render_packed_array_tokens(values, options)?;
                Self::render_packed_token_lines(
                    tokens,
                    first_prefix,
                    continuation_indent,
                    true,
                    options,
                )
            }
            StringArrayStyle::PreferSpaces => {
                let preferred = Self::render_packed_token_lines(
                    Self::render_packed_array_tokens(values, options)?,
                    first_prefix.clone(),
                    continuation_indent,
                    true,
                    options,
                )?;
                let fallback = Self::render_packed_token_lines(
                    Self::render_packed_array_tokens(values, options)?,
                    first_prefix,
                    continuation_indent,
                    false,
                    options,
                )?;
                Ok(pick_preferred_string_array_layout(
                    preferred, fallback, options,
                ))
            }
            StringArrayStyle::Comma => {
                let tokens = Self::render_packed_array_tokens(values, options)?;
                Self::render_packed_token_lines(
                    tokens,
                    first_prefix,
                    continuation_indent,
                    false,
                    options,
                )
            }
            StringArrayStyle::PreferComma => {
                let preferred = Self::render_packed_token_lines(
                    Self::render_packed_array_tokens(values, options)?,
                    first_prefix.clone(),
                    continuation_indent,
                    false,
                    options,
                )?;
                let fallback = Self::render_packed_token_lines(
                    Self::render_packed_array_tokens(values, options)?,
                    first_prefix,
                    continuation_indent,
                    true,
                    options,
                )?;
                Ok(pick_preferred_string_array_layout(
                    preferred, fallback, options,
                ))
            }
        }
    }

    fn render_packed_array_tokens(
        values: &[Value],
        options: &RenderOptions,
    ) -> Result<Vec<PackedToken>> {
        let mut tokens = Vec::new();
        for value in values {
            let token = match value {
                // Multiline strings are block elements — cannot be packed inline.
                Value::String(text) if text.contains('\n') || text.contains('\r') => {
                    PackedToken::Block(value.clone())
                }
                // Nonempty arrays and objects are block elements.
                Value::Array(vals) if !vals.is_empty() => PackedToken::Block(value.clone()),
                Value::Object(entries) if !entries.is_empty() => {
                    PackedToken::Block(value.clone())
                }
                // Inline string: force JSON quoting for comma-like chars to avoid parse ambiguity.
                Value::String(text) => {
                    let token_str = if text.chars().any(is_comma_like) {
                        render_json_string(text)
                    } else {
                        Self::render_scalar_token(value, options)?
                            .expect("non-multiline string always renders as scalar token")
                    };
                    PackedToken::Inline(token_str, value.clone())
                }
                // All other scalars (null, bool, number, empty array, empty object).
                _ => {
                    let token_str = Self::render_scalar_token(value, options)?
                        .expect("scalar always renders as inline token");
                    PackedToken::Inline(token_str, value.clone())
                }
            };
            tokens.push(token);
        }
        Ok(tokens)
    }

    /// Try to fold a lone-overflow inline token value into multiple lines.
    /// Returns `Some(lines)` (with 2+ lines) when fold succeeded, `None` when it didn't
    /// (value fits or fold is disabled / below MIN_FOLD_CONTINUATION).
    fn fold_packed_inline(
        value: &Value,
        continuation_indent: usize,
        first_line_extra: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        match value {
            Value::String(s) => {
                let lines =
                    Self::render_string_lines(s, continuation_indent, first_line_extra, options)?;
                Ok(if lines.len() > 1 { Some(lines) } else { None })
            }
            Value::Number(n) => {
                let ns = n.to_string();
                Ok(
                    fold_number(
                        &ns,
                        continuation_indent,
                        first_line_extra,
                        options.number_fold_style,
                        options.wrap_width,
                    )
                    .filter(|l| l.len() > 1),
                )
            }
            _ => Ok(None),
        }
    }

    fn render_packed_token_lines(
        tokens: Vec<PackedToken>,
        first_prefix: String,
        continuation_indent: usize,
        string_spaces_mode: bool,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if tokens.is_empty() {
            return Ok(Some(vec![first_prefix]));
        }

        // If the prefix alone already fills or exceeds wrap_width, no token can fit inline.
        if let Some(w) = options.wrap_width
            && first_prefix.len() >= w
        {
            return Ok(None);
        }

        // Spaces mode is incompatible with block elements (which are never strings).
        if string_spaces_mode && tokens.iter().any(|t| matches!(t, PackedToken::Block(_))) {
            return Ok(None);
        }

        let separator = if string_spaces_mode { "  " } else { ", " };
        let continuation_prefix = spaces(continuation_indent);

        // `current` is the line being built. `current_is_fresh` is true when nothing
        // has been appended to `current` yet (it holds only the line prefix).
        let mut current = first_prefix.clone();
        let mut current_is_fresh = true;
        let mut lines: Vec<String> = Vec::new();

        for token in tokens {
            match token {
                PackedToken::Block(value) => {
                    // Flush the current line if it has content, then render the block.
                    if !current_is_fresh {
                        if !string_spaces_mode {
                            current.push(',');
                        }
                        lines.push(current);
                    }

                    let block_lines = match &value {
                        Value::String(s) => {
                            Self::render_string_lines(s, continuation_indent, 0, options)?
                        }
                        Value::Array(vals) if !vals.is_empty() => {
                            Self::render_explicit_array(vals, continuation_indent, options)?
                        }
                        Value::Object(entries) if !entries.is_empty() => {
                            Self::render_explicit_object(entries, continuation_indent, options)?
                        }
                        _ => unreachable!("PackedToken::Block must contain a block value"),
                    };

                    // Merge the first block line with the current prefix.
                    // block_lines[0] is indented at continuation_indent; strip that and
                    // prepend whichever prefix we're currently using.
                    let current_prefix_str = if lines.is_empty() {
                        first_prefix.clone()
                    } else {
                        continuation_prefix.clone()
                    };
                    let first_block_content =
                        block_lines[0].get(continuation_indent..).unwrap_or("");
                    lines.push(format!("{}{}", current_prefix_str, first_block_content));
                    for bl in block_lines.into_iter().skip(1) {
                        lines.push(bl);
                    }

                    current = continuation_prefix.clone();
                    current_is_fresh = true;
                }
                PackedToken::Inline(token_str, value) => {
                    if current_is_fresh {
                        // Place the token on the fresh line (first_prefix or continuation).
                        current.push_str(&token_str);
                        current_is_fresh = false;

                        // Lone-overflow check: the token alone already exceeds the width.
                        if !fits_wrap(options, &current) {
                            let first_line_extra = if lines.is_empty() {
                                first_prefix.len().saturating_sub(continuation_indent)
                            } else {
                                0
                            };
                            if let Some(fold_lines) = Self::fold_packed_inline(
                                &value,
                                continuation_indent,
                                first_line_extra,
                                options,
                            )? {
                                // Attach the real line prefix to the first fold line.
                                let actual_prefix = if lines.is_empty() {
                                    first_prefix.clone()
                                } else {
                                    continuation_prefix.clone()
                                };
                                let first_content =
                                    fold_lines[0].get(continuation_indent..).unwrap_or("");
                                lines.push(format!("{}{}", actual_prefix, first_content));
                                for fl in fold_lines.into_iter().skip(1) {
                                    lines.push(fl);
                                }
                                current = continuation_prefix.clone();
                                current_is_fresh = true;
                            }
                            // else: overflow accepted — `current` retains the long line.
                        }
                    } else {
                        // Try to pack the token onto the current line.
                        let candidate = format!("{current}{separator}{token_str}");
                        if fits_wrap(options, &candidate) {
                            current = candidate;
                        } else {
                            // Flush current line, move token to a fresh continuation line.
                            if !string_spaces_mode {
                                current.push(',');
                            }
                            lines.push(current);
                            current = format!("{}{}", continuation_prefix, token_str);
                            current_is_fresh = false;

                            // Lone-overflow check on the new continuation line.
                            if !fits_wrap(options, &current)
                                && let Some(fold_lines) = Self::fold_packed_inline(
                                    &value,
                                    continuation_indent,
                                    0,
                                    options,
                                )? {
                                    let first_content =
                                        fold_lines[0].get(continuation_indent..).unwrap_or("");
                                    lines.push(format!(
                                        "{}{}",
                                        continuation_prefix, first_content
                                    ));
                                    for fl in fold_lines.into_iter().skip(1) {
                                        lines.push(fl);
                                    }
                                    current = continuation_prefix.clone();
                                    current_is_fresh = true;
                                }
                                // else: overflow accepted.
                        }
                    }
                }
            }
        }

        if !current_is_fresh {
            lines.push(current);
        }

        Ok(Some(lines))
    }

    fn render_table(
        values: &[Value],
        parent_indent: usize,
        options: &RenderOptions,
    ) -> Result<Option<Vec<String>>> {
        if values.len() < options.table_min_rows {
            return Ok(None);
        }

        let mut columns = Vec::<String>::new();
        let mut present_cells = 0usize;

        // Build column order from the first row, then verify all rows use the same order
        // for their shared keys. Differing key order would silently reorder keys on
        // round-trip — that is data loss, not a similarity issue.
        let mut first_row_keys: Option<Vec<&str>> = None;

        for value in values {
            let Value::Object(entries) = value else {
                return Ok(None);
            };
            present_cells += entries.len();
            for Entry { key, value: cell } in entries {
                if matches!(cell, Value::Array(inner) if !inner.is_empty())
                    || matches!(cell, Value::Object(inner) if !inner.is_empty())
                    || matches!(cell, Value::String(text) if text.contains('\n') || text.contains('\r'))
                {
                    return Ok(None);
                }
                if !columns.iter().any(|column| column == key) {
                    columns.push(key.clone());
                }
            }
            // Check that shared keys appear in the same relative order as in the first row.
            let row_keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
            if let Some(ref first) = first_row_keys {
                let shared_in_first: Vec<&str> = first.iter().copied().filter(|k| row_keys.contains(k)).collect();
                let shared_in_row: Vec<&str> = row_keys.iter().copied().filter(|k| first.contains(k)).collect();
                if shared_in_first != shared_in_row {
                    return Ok(None);
                }
            } else {
                first_row_keys = Some(row_keys);
            }
        }

        if columns.len() < options.table_min_columns {
            return Ok(None);
        }

        let similarity = present_cells as f32 / (values.len() * columns.len()) as f32;
        if similarity < options.table_min_similarity {
            return Ok(None);
        }

        let mut header_cells = Vec::new();
        let mut rows = Vec::new();
        for column in &columns {
            header_cells.push(render_key(column, options));
        }

        for value in values {
            let Value::Object(entries) = value else {
                return Ok(None);
            };
            let mut row: Vec<String> = Vec::new();
            for column in &columns {
                let token = if let Some(entry) = entries.iter().find(|e| &e.key == column) {
                    let value = &entry.value;
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
        // Bail out if any column's content exceeds table_column_max_width.
        // This does not and should not depend on table_fold.
        if let Some(col_max) = options.table_column_max_width
            && widths.iter().any(|w| *w > col_max) {
                return Ok(None);
        }
        for width in &mut widths {
            *width += 2;
        }

        // Bail out if the table is too wide to fit within wrap_width even at indent 0.
        // Each row is: (parent_indent + 2) spaces + |col1|col2|...|, where each colN width
        // includes 2 chars of padding. The caller handles unindenting via /< />, but if the
        // table still won't fit even at indent 0, block layout is better than overflow.
        if let Some(w) = options.wrap_width {
            // Each column renders as "|" + cell padded to `width` chars, plus trailing "|".
            // Minimum row width assumes indent 0: 2 spaces prefix + sum(widths) + one "|" per column + trailing "|".
            // The unindent logic may reduce indent below parent_indent, so only bail if it can't fit even at indent 0.
            // If table_fold is on, skip this bail-out — the fold logic below will handle overflow rows.
            let min_row_width = 2 + widths.iter().sum::<usize>() + widths.len() + 1;
            if min_row_width > w && !options.table_fold {
                return Ok(None);
            }
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

        // pair_indent for fold marker is two to the left of the `|` on each row
        let pair_indent = parent_indent; // elem rows at parent_indent+2, fold at parent_indent
        let fold_prefix = spaces(pair_indent);

        for row in rows {
            let row_line = format!(
                "{}{}",
                indent,
                row.iter()
                    .zip(widths.iter())
                    .map(|(cell, width)| format!("|{cell:<width$}", width = *width))
                    .collect::<String>()
                    + "|"
            );

            if options.table_fold {
                // Fold if the row line exceeds wrap_width.
                // The fold splits the row line at a point within a cell's string value,
                // between the first and last data character (not between `|` and value start).
                // Find the fold point by scanning back from the wrap boundary.
                let fold_avail = options
                    .wrap_width
                    .unwrap_or(usize::MAX)
                    .saturating_sub(pair_indent + 2); // content after `  ` row prefix
                if row_line.len() > fold_avail + pair_indent + 2 {
                    // Find a fold point: must be within a cell's string data, after the
                    // leading space of a bare string or after the first `"` of a JSON string.
                    // We look for a space inside a cell value (not the cell padding spaces).
                    if let Some((before, after)) = split_table_row_for_fold(&row_line, fold_avail + pair_indent + 2) {
                        lines.push(before);
                        lines.push(format!("{}/ {}", fold_prefix, after));
                        continue;
                    }
                }
            }

            lines.push(row_line);
        }

        Ok(Some(lines))
    }

    fn render_table_cell_token(
        value: &Value,
        options: &RenderOptions,
    ) -> Result<Option<String>> {
        Ok(match value {
            Value::Null => Some("null".to_owned()),
            Value::Bool(value) => Some(if *value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }),
            Value::Number(value) => Some(value.to_string()),
            Value::String(value) => {
                if value.contains('\n') || value.contains('\r') {
                    None
                } else if options.bare_strings == BareStyle::Prefer
                    && is_allowed_bare_string(value)
                    && !is_reserved_word(value) //matches!(value.as_str(), "true" | "false" | "null")
                    // '|' itself is also checked in is_pipe_like but here too for clarity
                    && !value.contains('|')
                    && value.chars().find(|c| is_pipe_like(*c)).is_none()
                {
                    Some(format!(" {}", value))
                } else {
                    Some(render_json_string(value))
                }
            }
            Value::Array(values) if values.is_empty() => Some("[]".to_owned()),
            Value::Object(entries) if entries.is_empty() => Some("{}".to_owned()),
            _ => None,
        })
    }
}

