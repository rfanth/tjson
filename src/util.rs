use unicode_general_category::{GeneralCategory, get_general_category};

pub(crate) fn count_leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

pub(crate) fn spaces(count: usize) -> String {
    " ".repeat(count)
}


pub(crate) fn starts_with_marker_chain(content: &str) -> bool {
    content.starts_with("[ ") || content.starts_with("{ ")
}

pub(crate) fn parse_json_string_prefix(content: &str) -> Option<(String, usize)> {
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
                end = Some(index + '"'.len_utf8());
                break;
            }
            '\n' | '\r' => return None,
            _ => {}
        }
    }
    let end = end?;
    // TJSON allows literal tab characters inside quoted strings; escape them before JSON parsing.
    let json_src = if content[..end].contains('\t') {
        std::borrow::Cow::Owned(content[..end].replace('\t', "\\t"))
    } else {
        std::borrow::Cow::Borrowed(&content[..end])
    };
    let parsed = serde_json::from_str(&json_src).ok()?;
    Some((parsed, end))
}

pub(crate) fn split_pipe_cells(row: &str) -> Option<Vec<String>> {
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

pub(crate) fn is_minimal_json_candidate(content: &str) -> bool {
    let bytes = content.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    (bytes[0] == b'{' && bytes[1] != b'}' && bytes[1] != b' ')
        || (bytes[0] == b'[' && bytes[1] != b']' && bytes[1] != b' ')
}

pub(crate) fn is_valid_minimal_json(content: &str) -> Result<(), usize> {
    let mut in_string = false;
    let mut escaped = false;

    for (col, ch) in content.chars().enumerate() {
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
            ch if ch.is_whitespace() => return Err(col),
            _ => {}
        }
    }

    if in_string || escaped { Err(content.len()) } else { Ok(()) }
}

pub(crate) fn parse_bare_key_prefix(content: &str) -> Option<usize> {
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


pub(crate) fn is_unicode_letter_or_number(ch: char) -> bool {
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

pub(crate) fn is_forbidden_literal_tjson_char(ch: char) -> bool {
    is_forbidden_control_char(ch)
        || is_default_ignorable_code_point(ch)
        || is_private_use_code_point(ch)
        || is_noncharacter_code_point(ch)
        || matches!(ch, '\u{2028}' | '\u{2029}')
}

pub(crate) fn is_forbidden_bare_char(ch: char) -> bool {
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

pub(crate) fn is_forbidden_control_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000B}'..='\u{000C}'
            | '\u{000E}'..='\u{001F}'
            | '\u{007F}'..='\u{009F}'
    )
}

pub(crate) fn is_default_ignorable_code_point(ch: char) -> bool {
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

pub(crate) fn is_private_use_code_point(ch: char) -> bool {
    matches!(get_general_category(ch), GeneralCategory::PrivateUse)
}

pub(crate) fn is_noncharacter_code_point(ch: char) -> bool {
    let code_point = ch as u32;
    (0xFDD0..=0xFDEF).contains(&code_point)
        || (code_point <= 0x10FFFF && (code_point & 0xFFFE) == 0xFFFE)
}

pub(crate) fn render_json_string(value: &str) -> String {
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

pub(crate) fn push_json_unicode_escape(rendered: &mut String, ch: char) {
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

/// Returns true if the line starts with zero or more whitespace chars then the given char.
pub(crate) fn line_starts_with_ws_then(line: &str, ch: char) -> bool {
    let trimmed = line.trim_start_matches(|c: char| c.is_whitespace());
    trimmed.starts_with(ch)
}

/// Split a multiline-string body part into segments for fold continuations.
/// Returns the original text as a single segment if no fold is needed.
/// Segments: first is the line body, rest are fold continuations (without the `/ ` prefix).
pub(crate) fn safe_json_split(s: &str, split_at: usize) -> usize {
    // Walk backwards from split_at to find the last `\` and see if split is mid-escape
    let bytes = s.as_bytes();
    let pos = split_at.min(bytes.len());
    // Count consecutive backslashes before pos
    let mut backslashes = 0usize;
    let mut i = pos;
    while i > 0 && bytes[i - 1] == b'\\' {
        backslashes += 1;
        i -= 1;
    }
    if backslashes % 2 == 1 {
        // We are inside a `\X` escape — back up one more
        pos.saturating_sub(1)
    } else {
        pos
    }
}

/// Attempt to fold a bare string into multiple lines with `/ ` continuations.
/// Returns None if folding is not needed or not possible.
/// The first element is the first line (`{spaces(indent)} {first_segment}`),
/// subsequent elements are fold lines (`{spaces(indent)}/ {segment}`).
pub(crate) fn find_number_fold_point(s: &str, avail: usize, auto_mode: bool) -> usize {
    let avail = avail.min(s.len());
    if avail == 0 || avail >= s.len() {
        return 0;
    }
    if auto_mode {
        // Prefer the last `.` or `e`/`E` at or before avail — fold before it.
        let candidate = &s[..avail];
        if let Some(pos) = candidate.rfind(['.', 'e', 'E'])
            && pos > 0 {
                return pos; // fold before the separator
            }
    }
    // Fall back: split between two digit characters at the avail boundary.
    // Walk back to find a digit-digit boundary.
    let bytes = s.as_bytes();
    let mut pos = avail;
    while pos > 1 {
        if bytes[pos - 1].is_ascii_digit() && bytes[pos].is_ascii_digit() {
            return pos;
        }
        pos -= 1;
    }
    0
}

/// Fold a number value into multiple lines with `/ ` continuations.
/// Numbers have no leading space (unlike bare strings). Returns None if no fold needed.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharClass {
    Space,
    Letter,
    Digit,
    /// Punctuation that prefers to trail at the end of a line: `.` `,` `/` `-` `_` `~` `@` `:`.
    StickyEnd,
    Other,
}

pub(crate) fn char_class(ch: char) -> CharClass {
    if ch == ' ' {
        return CharClass::Space;
    }
    if matches!(ch, '.' | ',' | '/' | '-' | '_' | '~' | '@' | ':') {
        return CharClass::StickyEnd;
    }
    match get_general_category(ch) {
        GeneralCategory::UppercaseLetter
        | GeneralCategory::LowercaseLetter
        | GeneralCategory::TitlecaseLetter
        | GeneralCategory::ModifierLetter
        | GeneralCategory::OtherLetter
        | GeneralCategory::LetterNumber => CharClass::Letter,
        GeneralCategory::DecimalNumber | GeneralCategory::OtherNumber => CharClass::Digit,
        _ => CharClass::Other,
    }
}

/// Find a fold point in a bare string candidate slice.
/// Returns a byte offset suitable for splitting, or 0 if none found.
///
/// `lookahead` is the character immediately after the candidate window. When provided,
/// the transition at `s.len()` (take the full window) is also considered as a split point.
///
/// Priorities (highest first, rightmost position within each priority wins):
/// 1. Before a `Space` — space moves to the next line.
/// 2. `StickyEnd`→`Letter`/`Digit` — punctuation trails the current line, next word starts fresh.
/// 3. `Letter`↔`Digit` — finer boundary within an alphanumeric run.
/// 4. `Letter`/`Digit`→`StickyEnd`/`Other` — weakest: word trailing into punctuation.
pub(crate) fn find_bare_fold_point(s: &str, lookahead: Option<char>) -> usize {
    // Track the last-seen position for each priority level (0 = highest).
    let mut best = [0usize; 4];
    let mut prev: Option<(usize, CharClass)> = None;

    for (byte_pos, ch) in s.char_indices() {
        let cur = char_class(ch);
        if let Some((_, p)) = prev {
            match (p, cur) {
                // P1: anything → Space (split before the space)
                (_, CharClass::Space) if byte_pos > 0 => best[0] = byte_pos,
                // P2: StickyEnd → Letter or Digit (after punctuation run, before a word)
                (CharClass::StickyEnd, CharClass::Letter | CharClass::Digit) => best[1] = byte_pos,
                // P3: Letter ↔ Digit
                (CharClass::Letter, CharClass::Digit) | (CharClass::Digit, CharClass::Letter) => {
                    best[2] = byte_pos
                }
                // P4: Letter/Digit → StickyEnd or Other
                (CharClass::Letter | CharClass::Digit, CharClass::StickyEnd | CharClass::Other) => {
                    best[3] = byte_pos
                }
                _ => {}
            }
        }
        prev = Some((byte_pos, cur));
    }

    // Check the edge: transition between the last char of the window and the lookahead.
    // A split here means taking the full window (split_at = s.len()).
    if let (Some((_, last_class)), Some(next_ch)) = (prev, lookahead) {
        let next_class = char_class(next_ch);
        let edge = s.len();
        match (last_class, next_class) {
            (_, CharClass::Space) => best[0] = best[0].max(edge),
            (CharClass::StickyEnd, CharClass::Letter | CharClass::Digit) => {
                best[1] = best[1].max(edge)
            }
            (CharClass::Letter, CharClass::Digit) | (CharClass::Digit, CharClass::Letter) => {
                best[2] = best[2].max(edge)
            }
            (CharClass::Letter | CharClass::Digit, CharClass::StickyEnd | CharClass::Other) => {
                best[3] = best[3].max(edge)
            }
            _ => {}
        }
    }

    // Return rightmost position of the highest priority found.
    best.into_iter().find(|&p| p > 0).unwrap_or(0)
}

/// Attempt to fold a JSON-encoded string value into multiple lines with `/ ` continuations.
/// The output strings form a JSON string spanning multiple lines with fold markers.
/// Returns None if folding is not needed.
pub(crate) fn count_preceding_backslashes(bytes: &[u8], pos: usize) -> usize {
    let mut count = 0;
    let mut p = pos;
    while p > 0 {
        p -= 1;
        if bytes[p] == b'\\' { count += 1; } else { break; }
    }
    count
}

/// Find a fold point in a JSON-encoded string slice.
///
/// Priority:
/// 1. After an escaped EOL sequence (`\n` or `\r` in the encoded inner string) — fold after
///    the escape so the EOL stays with the preceding content.
/// 2. Before a literal space character.
/// 3. Safe split at end.
///
/// Returns byte offset into `s`, or 0 if no suitable point is found.
pub(crate) fn find_json_fold_point(s: &str) -> usize {
    let bytes = s.as_bytes();

    // Pass 1: prefer splitting after an escaped \n (the encoded two-char sequence `\n`).
    // This naturally keeps \r\n together: when value has \r\n, the encoded form is `\r\n`
    // and we split after the `\n`, which is after the full pair.
    // Scan backward; return the rightmost such position that fits.
    let mut i = bytes.len();
    while i > 1 {
        i -= 1;
        if bytes[i] == b'n' && bytes[i - 1] == b'\\' {
            // Count the run of backslashes ending at i-1
            let bs = count_preceding_backslashes(bytes, i) + 1; // +1 for bytes[i-1]
            if bs % 2 == 1 {
                // Genuine \n escape — split after it
                return (i + 1).min(bytes.len());
            }
        }
    }

    // Pass 2: split before a literal space.
    let mut i = bytes.len();
    while i > 1 {
        i -= 1;
        if bytes[i] == b' ' {
            let safe = safe_json_split(s, i);
            if safe == i {
                return i;
            }
        }
    }

    // Pass 3: fall back to any word boundary (letter-or-number ↔ other).
    // The encoded inner string is ASCII-compatible, so we scan for byte-level
    // alphanumeric transitions. Non-ASCII escaped as \uXXXX are all alphanumeric
    // in the encoded form so boundaries naturally occur at the leading `\`.
    let mut last_boundary = 0usize;
    let mut prev_is_word: Option<bool> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let cur_is_word = bytes[i].is_ascii_alphanumeric();
        if let Some(prev) = prev_is_word
            && prev != cur_is_word {
                let safe = safe_json_split(s, i);
                if safe == i {
                    last_boundary = i;
                }
            }
        prev_is_word = Some(cur_is_word);
        i += 1;
    }
    if last_boundary > 0 {
        return last_boundary;
    }

    // Final fallback: hard split at end.
    safe_json_split(s, s.len())
}

/// Render an EOL-containing string as a folded JSON string (`FoldingQuotes` style).
///
/// Always folds at `\n` boundaries — each newline in the original value becomes a `/ `
/// continuation point. Within-piece width folding follows `string_multiline_fold_style`.
pub(crate) fn split_table_row_for_fold(row: &str, max_len: usize) -> Option<(String, String)> {
    if row.len() <= max_len {
        return None;
    }
    let bytes = row.as_bytes();
    // Walk backwards from max_len to find a split point inside a string cell.
    // A valid fold point is a space character that is inside a cell value
    // (not the padding spaces right after `|`, and not the leading space of a bare string).
    let scan_end = max_len.min(bytes.len());
    // Find the last space that is preceded by a non-space (i.e., inside content)
    let mut pos = scan_end;
    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b' ' && pos > 0 && bytes[pos - 1] != b'|' && bytes[pos - 1] != b' ' {
            let before = row[..pos].to_owned();
            let after = row[pos + ' '.len_utf8()..].to_owned(); // skip the space itself
            return Some((before, after));
        }
    }
    None
}

pub(crate) fn is_comma_like(ch: char) -> bool {
    matches!(ch, ',' | '\u{FF0C}' | '\u{FE50}')
}

pub(crate) fn is_quote_like(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::InitialPunctuation | GeneralCategory::FinalPunctuation
    ) || matches!(ch, '"' | '\'' | '`')
}

/// matches a literal '|' pipe or a PIPELIKE CHARACTER
/// PIPELIKE CHARACTER in spec:  PIPELIKE CHARACTER DEFINITION A pipelike character is U+007C (VERTICAL LINE) or any character in the following set: U+00A6, U+01C0, U+2016, U+2223, U+2225, U+254E, U+2502, U+2503, U+2551, U+FF5C, U+FFE4
pub(crate) fn is_pipe_like(ch: char) -> bool {
    matches!(
        ch, '|' | '\u{00a6}' | '\u{01c0}' | '\u{2016}' | '\u{2223}' | '\u{2225}' | '\u{254e}' | '\u{2502}' | '\u{2503}' | '\u{2551}' | '\u{ff5c}' | '\u{ffe4}'
    )
}
pub(crate) fn is_reserved_word(s: &str) -> bool {
    matches!(s, "true" | "false" | "null" | "[]" | "{}" | "\"\"") // "" is logically reserved but unreachable: '"' is quote-like and forbidden as a bare string first/last char
}

pub(crate) fn is_allowed_bare_string(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let first = value.chars().next().unwrap();
    let last = value.chars().next_back().unwrap();
    if first == ' '
        || last == ' '
        || first == '/'
        || is_pipe_like(first)
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
            if previous_space { return false; }
            previous_space = true;
        } else {
            previous_space = false;
        }
    }
    true
}
