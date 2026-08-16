use crate::number::{DECIMAL_POINT, EXPONENT_MARKERS};
use crate::position::{Columns, Marker};

use crate::options::{ParseOptions, SPEC_FORMS};
use unicode_general_category::{GeneralCategory, get_general_category};

pub(crate) fn count_leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

/// The bytes of a `\uXXXX` escape: a backslash, a `u`, and four hex digits.
///
/// The surrogate-pair code below walks these offsets in both directions, and every
/// one of them is this same fact -- where the hex starts, how far back the escape
/// began, where the next one would. Written once so a reader checking one site is
/// checking all of them, and so no future change can move some and not others.
pub(crate) const UNICODE_ESCAPE_LEN: usize = 6;

/// The hex digits in a `\uXXXX` escape.
pub(crate) const UNICODE_ESCAPE_HEX: usize = 4;

/// Where the hex digits begin within a `\uXXXX` escape, past the `\u`.
pub(crate) const UNICODE_ESCAPE_HEX_START: usize = UNICODE_ESCAPE_LEN - UNICODE_ESCAPE_HEX;

pub(crate) fn starts_with_marker_chain(content: &str) -> bool {
    Marker::CHAIN.iter().any(|m| m.opens(content))
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

/// One cell of a pipe row: its text, and where that text begins in the row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PipeCell<'a> {
    /// Byte offset of `text` within the row that was split.
    pub(crate) at: usize,
    pub(crate) text: &'a str,
}

/// Split a pipe row into its cells, at the `|` characters that are not inside a
/// quoted string.
///
/// Returns where each cell starts, because both callers need to point a caret at
/// one. The header path used to rebuild these offsets after the fact by summing
/// cell lengths and separator widths, and the row path gave up and pointed every
/// cell error at column 1. The split already knows both, so it says both -- which
/// also stops it copying every cell of every row into a `String` on the way out.
///
/// A cell is always a contiguous slice of `row`: this only finds boundaries, it
/// never rewrites content.
///
/// `None` when `row` does not open with `|`, or ends inside a quoted string.
pub(crate) fn split_pipe_cells(row: &str) -> Option<Vec<PipeCell<'_>>> {
    if !row.starts_with('|') {
        return None;
    }
    let mut cells = Vec::new();
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in row.char_indices() {
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
            '|' => {
                cells.push(PipeCell { at: start, text: &row[start..index] });
                start = index + '|'.len_utf8();
            }
            _ => {}
        }
    }

    if in_string || escaped {
        return None;
    }

    cells.push(PipeCell { at: start, text: &row[start..] });
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

/// Byte length of the MINIMAL JSON value at the front of `content`.
///
/// Scans to the bracket closing the one it opens with, ignoring brackets inside
/// strings. Returns `None` if it never closes.
///
/// This is what lets MINIMAL JSON be followed by another key-value pair on the
/// same line -- `c:[1,2]    a:1`. Without it the value ran to end of line and
/// swallowed the pair after it, so pasting `[1]` into a packed line only worked
/// if you put it last.
pub(crate) fn minimal_json_end(content: &str) -> Option<usize> {
    let open = content.chars().next()?;
    let close = match open {
        '[' => ']',
        '{' => '}',
        _ => return None,
    };
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in content.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
        } else if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(index + ch.len_utf8());
            }
        }
    }
    None
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

/// How far a bare key runs, under `forms`.
///
/// Note that the sets decide where the run *ends*, not merely whether it is
/// accepted: emptying one can lengthen a key rather than only permitting a
/// character inside it, so two readings of the same bytes can produce two
/// different keys. That is inherent to a format whose runs have no closing
/// delimiter, and is why the sets are internal.
pub(crate) fn parse_bare_key_prefix(content: &str, forms: &ParseOptions) -> Option<usize> {
    let mut chars = content.char_indices().peekable();
    let (_, first) = chars.next()?;
    // Rules 1 and 2: a key opens with a letter or number. Rules 0 and 4 then
    // subtract the lookalikes Unicode files under those categories -- U+01C0 is
    // a PIPELIKE and a letter, U+02BB is a COMMALIKE and a letter, U+02CD is an
    // UNDERSCORELIKE and a letter -- so the category test alone is not enough
    // and each set is checked in its own right. Rules 6 and 7 apply from the
    // first character too.
    //
    // FORESLASHLIKE holds no letters or numbers today, so its test cannot fire;
    // it is here because that is a fact about the current set rather than about
    // the rule, and the next character added to it should not have to be caught
    // by someone remembering this line exists.
    if !is_unicode_letter_or_number(first)
        || forms.is_pipe_like(first)
        || forms.is_comma_like(first)
        || forms.is_quote_like(first)
        || forms.is_colon_like(first)
        || forms.is_underscore_like(first)
        || forms.is_foreslash_like(first)
        || is_weird_class_char(first)
    {
        return None;
    }
    let mut end = first.len_utf8();

    let mut previous_space = false;
    for (index, ch) in chars {
        // Rule 6 forbids a COLONLIKE anywhere, rule 7 the weird classes. Both
        // can hide inside \p{L}, so neither is implied by the character set
        // below and both end the run rather than being accepted into it.
        if forms.is_colon_like(ch) || (ch != ' ' && is_weird_class_char(ch)) {
            break;
        }
        if is_unicode_letter_or_number(ch)
            || matches!(
                ch,
                '_' | '(' | ')' | '/' | '\'' | '.' | '!' | '%' | '&' | ',' | '-'
                    | ';' | '@' | '$' | '#' | '*' | '=' | '?' | '^' | '~' | '<' | '>' | '+'
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

    // A bare key may not end on a space, a comma-like or a quote-like character.
    // That does not make the run invalid, it makes it unfinished: give the tail
    // back rather than discarding the whole run, the same way `bare_string_run`
    // holds one back. A caller then sees the run that is really there -- which is
    // what lets a folded key keep collecting continuations across a comma, where
    // returning `None` here reported "not a bare key at all".
    while let Some(last) = content[..end].chars().next_back() {
        if is_held_back_from_run_end(last, forms) {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 { None } else { Some(end) }
}

/// May a bare key run end on `ch`, or is it held back?
///
/// One rule, asked from two directions: [`parse_bare_key_prefix`] strips this set
/// off the end of a run, and `only_held_back_tail` asks whether what the run gave
/// back is only this set. They were written separately and disagreed about
/// PIPELIKE, which is reachable because U+01C0 is a PIPELIKE *and* a letter, so
/// it gets into a run and is then stripped from it. A folded bare key ending in
/// one was refused with `there is no colon on this line` -- and there was one, on
/// the continuation. Neither side of the pair may hold this list alone.
pub(crate) fn is_held_back_from_run_end(ch: char, forms: &ParseOptions) -> bool {
    ch == ' ' || forms.is_comma_like(ch) || forms.is_quote_like(ch) || forms.is_pipe_like(ch)
}


/// True for `\p{L}` and `\p{N}`: the categories a BARE KEY may open with, and the
/// bulk of what any bare run may continue with.
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

/// Which rule of the specification's FORBIDDEN CHARACTERS a character breaks.
///
/// Carried rather than collapsed to a bool because the six answers want six
/// different sentences: a C1 control is almost always an encoding mistake, a
/// private use code point is a private agreement that cannot survive being
/// shared, and a line separator is a character that makes the reader and the
/// parser disagree about where the lines are. "Forbidden" alone tells a reader
/// none of that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ForbiddenLiteral {
    /// Rule 1, ASCII half: C0 other than TAB, LF and CR, or DEL.
    Control,
    /// Rule 1, non-ASCII half: the C1 block, U+0080..=U+009F.
    C1Control,
    /// Rule 2, and rule 4 with it -- every Bidi_Control character is `Cf`.
    DefaultIgnorable,
    /// Rule 5.
    PrivateUse,
    /// Rule 6.
    Noncharacter,
    /// Rule 7: U+2028 and U+2029.
    LineSeparator,
}

impl ForbiddenLiteral {
    /// What went wrong and what to do about it, for `ch`.
    ///
    /// Every arm names an escape route, because every one of these characters is
    /// legal escaped -- the document is rejected over how it was written, not over
    /// what it holds.
    pub(crate) fn describe(self, ch: char) -> String {
        let code = format!("U+{:04X}", ch as u32);
        match self {
            Self::Control => format!(
                "{code} is a control character, and only TAB, LF and CR may appear \
                 literally -- write it as an escape inside a JSON string"
            ),
            Self::C1Control => format!(
                "{code} is a C1 control character (U+0080 to U+009F), which is almost \
                 always Windows-1252 text read as Latin-1 rather than anything intended \
                 -- check the encoding of the source, and if it really is meant, write \
                 it as an escape inside a JSON string"
            ),
            Self::DefaultIgnorable => format!(
                "{code} is a default-ignorable code point, invisible where it is \
                 rendered, so a reader cannot see what the parser is reading -- write \
                 it as an escape inside a JSON string"
            ),
            Self::PrivateUse => format!(
                "{code} is a private use code point, whose meaning is an agreement \
                 between two particular parties and does not survive being read by \
                 anyone else -- write it as an escape inside a JSON string"
            ),
            Self::Noncharacter => format!(
                "{code} is a noncharacter, permanently reserved by Unicode and never \
                 assigned to anything -- write it as an escape inside a JSON string"
            ),
            Self::LineSeparator => format!(
                "{code} is a Unicode line separator, which a reader honoring Unicode \
                 line breaking shows as a line break and this parser does not -- the \
                 lines you see would not be the lines that are read; write it as an \
                 escape inside a JSON string"
            ),
        }
    }
}

/// Which FORBIDDEN CHARACTERS rule `ch` breaks, or `None` if it may appear
/// literally.
///
/// The single decider for that question. [`is_forbidden_literal_tjson_char`] is
/// this with the answer thrown away, never a second implementation.
///
/// Six of the specification's seven rules have a clause below. Rule 3, surrogates,
/// has none and needs none: a Rust `char` is a Unicode scalar value, so a surrogate
/// cannot be built and cannot appear in a `&str`. The type forbids what this
/// function would otherwise have to. (Escaped surrogate pairs inside a JSON STRING
/// are a separate question, settled where that string is decoded.)
pub(crate) fn check_forbidden_literal(ch: char) -> Result<(), ForbiddenLiteral> {
    if ch.is_ascii() {
        // FAST PATH, ascii only.
        //
        // Rule 1's ASCII half is the whole answer here, because every other rule's
        // set begins above ASCII: C1 at U+0080, default-ignorable at U+00AD, private
        // use at U+E000, the noncharacters at U+FDD0, the separators at U+2028. So
        // this skips a Unicode general-category lookup per character, and that
        // lookup was 17% of the time spent rendering a 46 MB document.
        //
        // `forbidden_literal_ascii_matches_the_general_path` pins this against the
        // general check below across all 128, reason and all, so they cannot drift.
        if is_forbidden_ascii_control_char(ch) {
            return Err(ForbiddenLiteral::Control);
        }
        return Ok(());
    }

    // General check, works for ASCII and not.
    if is_forbidden_ascii_control_char(ch) {
        return Err(ForbiddenLiteral::Control);          // rule 1, ASCII half
    }
    if is_c1_control_char(ch) {
        return Err(ForbiddenLiteral::C1Control);        // rule 1, the half that is not ASCII
    }
    if is_default_ignorable_code_point(ch) {
        return Err(ForbiddenLiteral::DefaultIgnorable); // rule 2, and rule 4 with it
    }
    if is_private_use_code_point(ch) {
        return Err(ForbiddenLiteral::PrivateUse);       // rule 5
    }
    if is_noncharacter_code_point(ch) {
        return Err(ForbiddenLiteral::Noncharacter);     // rule 6
    }
    if is_line_or_paragraph_separator(ch) {
        return Err(ForbiddenLiteral::LineSeparator);    // rule 7
    }
    Ok(())
}

/// True for characters that may never appear literally anywhere in a TJSON
/// document -- not in a bare run, not inside a JSON string, not in a comment.
/// They can only be written as `\uXXXX` escapes. `scan_lines` rejects them across
/// the whole input before parsing begins, so no later scan can meet one.
///
/// For callers that only need the verdict; [`check_forbidden_literal`] decides.
pub(crate) fn is_forbidden_literal_tjson_char(ch: char) -> bool {
    check_forbidden_literal(ch).is_err()
}

/// True for U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR, the two
/// characters Unicode defines as line breaks that are neither CR nor LF.
///
/// TJSON splits lines on `\n` and reads indentation as structure, while a reader
/// honoring Unicode's line breaking treats these as breaks too. A document holding
/// one literally would show a person a different set of lines than the parser
/// measures -- and in a format where location is depth, that is a document meaning
/// two things at once.
///
/// `Zl` and `Zp` contain exactly one character each and always will, so matching
/// the two literals is the whole of both categories and avoids a category lookup.
fn is_line_or_paragraph_separator(ch: char) -> bool {
    matches!(ch, '\u{2028}' | '\u{2029}')
}

/// True for the "weird" characters a BARE STRING -- and so a BARE KEY -- may not
/// contain anywhere: `\p{C}`, `\p{Z}`, `\p{M}`, and Default_Ignorable_Code_Point.
/// That is bare key rule 7, which the specification notes is fully implied by the
/// bare string rules it comes from.
///
/// This is a *category* test and nothing more. It says nothing about the
/// structural characters (`|`, `` ` ``, `"`, `:`, `[`, `{`, `\`), which a bare run
/// excludes by not admitting them to its character class, and nothing about the
/// lookalike sets, which are checked in their own right. Reading it as "the
/// characters forbidden in a bare key" is wrong in both directions.
pub(crate) fn is_weird_class_char(ch: char) -> bool {
    // Of the categories excluded below, only Control and SpaceSeparator reach
    // into ASCII -- Format, Unassigned, the two separators and the three mark
    // classes all begin above it. So for ASCII the answer is the controls plus
    // the space, and the general-category lookup can be skipped entirely. That
    // lookup sits in the innermost loop of every bare key and bare string scan.
    //
    // `weird_class_ascii_matches_the_general_path` checks this against the
    // general path for all 128, so it cannot drift.
    if ch.is_ascii() {
        return ch <= ' ' || ch == '\u{7F}';
    }
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

/// The ASCII half of rule 1: C0 except TAB, LF and CR, plus DEL.
///
/// TAB is absent on purpose. It is legal as a character and rejected separately as
/// a layout error by `Parser::ensure_line_has_no_tabs`, because an indent measured
/// in tabs is a different problem from a character that cannot be represented.
pub(crate) fn is_forbidden_ascii_control_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0000}'..='\u{0008}'
            | '\u{000B}'..='\u{000C}'
            | '\u{000E}'..='\u{001F}'
            | '\u{007F}'
    )
}

/// The non-ASCII half of rule 1: the C1 controls, U+0080..=U+009F.
///
/// ECMA-48's 8-bit control block -- NEL, CSI, SS2, OSC and the rest. Two bytes each
/// in UTF-8, so `char::is_ascii` is false for every one of them and an ASCII fast
/// path cannot stand in for this test.
///
/// Two reasons beyond the rule. U+0085 NEL is a line break under Unicode's rules, so
/// a literal one shows a reader lines the parser does not measure, exactly as
/// [`is_line_or_paragraph_separator`] describes. And these byte values are printable
/// in Windows-1252 (curly quotes, em dash, the euro sign) while being controls in
/// Latin-1, so text mislabelled between the two lands here -- a literal C1 character
/// almost always means an encoding went wrong upstream, and accepting it would
/// preserve the damage.
pub(crate) fn is_c1_control_char(ch: char) -> bool {
    matches!(ch, '\u{0080}'..='\u{009F}')
}

/// True for Unicode's Default_Ignorable_Code_Point property: every `Format`
/// character, plus the code points that carry the property without being `Format`
/// and so have to be listed by hand.
///
/// Those listed ones are why the property is checked at all rather than the
/// category alone. Several are `\p{L}` -- the Hangul fillers U+115F, U+1160,
/// U+3164 and U+FFA0 are `OtherLetter` -- so a category test would admit an
/// invisible letter wherever letters are allowed.
///
/// This also carries the specification's rule 4, Bidi_Control, which has no clause
/// of its own anywhere: every character with that property is `Cf` and so is caught
/// by the `Format` test below. That containment is a fact about Unicode rather than
/// anything this code arranges, so narrowing this to the enumerated list alone would
/// silently make bidi overrides legal, with nothing failing to say so.
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

/// True for the three Private Use Areas, whose meaning is by definition an
/// agreement between two parties and cannot survive being written to a document
/// meant to be read by anyone else.
pub(crate) fn is_private_use_code_point(ch: char) -> bool {
    matches!(get_general_category(ch), GeneralCategory::PrivateUse)
}

/// True for the 66 code points Unicode permanently reserves as noncharacters:
/// U+FDD0..=U+FDEF, and the last two of every plane (`xxFFFE` and `xxFFFF`).
/// They are valid `char` values in Rust and are guaranteed never to be assigned,
/// so nothing legitimate can be trying to write one.
pub(crate) fn is_noncharacter_code_point(ch: char) -> bool {
    let code_point = ch as u32;
    (0xFDD0..=0xFDEF).contains(&code_point)
        || (code_point <= 0x10FFFF && (code_point & 0xFFFE) == 0xFFFE)
}

pub(crate) fn render_json_string(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len() + 2);
    push_json_string(&mut rendered, value);
    rendered
}

/// Writes `value` as a quoted JSON STRING directly into `out`.
///
/// The single escaper for every JSON STRING this crate emits, in TJSON output
/// and in MINIMAL JSON alike. Both must agree, because the spec requires the
/// forbidden set to be escaped "EVERY TIME IN EVERY CONTEXT, INCLUDING MINIMAL
/// JSON" -- a rule with two writers and one implementation is a rule that holds
/// in one of them.
///
/// What it escapes is a strict superset of what JSON requires: the seven
/// mandatory escapes, everything at or below U+001F, and additionally the TJSON
/// forbidden set, which JSON is happy to pass through literally. So the output
/// is always valid JSON *and* always valid TJSON. `json_output_still_satisfies_serde_json`
/// pins the first half against serde_json as an oracle;
/// `no_writer_emits_a_forbidden_literal` pins the second.
///
/// Writes in place rather than returning a `String`, because the callers are
/// building one document out of many strings and an owned return would allocate
/// and discard a buffer per string and per key.
pub(crate) fn push_json_string(out: &mut String, value: &str) {
    out.reserve(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{001F}' || is_forbidden_literal_tjson_char(ch) => {
                push_json_unicode_escape(out, ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
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
/// The first element is the first line (the indent, then the first segment);
/// subsequent elements are fold lines (the indent, then `/ `, then a segment).
/// Smallest run of bare digits worth a line of its own. A shorter chunk says
/// nothing about where in the number it sits, so it reads as debris rather
/// than as part of a value.
const MIN_NUMBER_FOLD_CHUNK: usize = 10;

pub(crate) fn find_number_fold_point(s: &str, avail: Columns, auto_mode: bool) -> usize {
    // The budget spent along the text, which is where it runs out in bytes. A
    // JSON number is ASCII, so this equalled `avail` itself before the crossing
    // existed -- true by what the data happens to be rather than by anything
    // stated, which is the kind of agreement that stops holding quietly.
    let avail = avail.spent_in(s);
    if avail == 0 || avail >= s.len() {
        return 0;
    }
    if auto_mode {
        // Prefer the last `.` or `e`/`E` at or before avail, folding *before*
        // it, so the continuation opens with the marker and tells the reader
        // at a glance whether they are looking at a fraction or an exponent.
        //
        // The chunk left behind still has to be worth a line: breaking before
        // the `.` of `1.234…` would strand a single digit. The chunk that
        // follows may be short, because a line opening with `.` or `e`
        // describes itself and needs no length to be legible.
        // `e` outranks `.`: an exponent changes the number's order of
        // magnitude while a fractional part only refines it, so when there is
        // room for a single division, the exponent is the meaningful place to
        // put it.
        let candidate = &s[..avail];
        for markers in [&EXPONENT_MARKERS[..], &[DECIMAL_POINT][..]] {
            if let Some(pos) = candidate.rfind(markers)
                && pos >= MIN_NUMBER_FOLD_CHUNK {
                    return pos;
                }
        }
    }
    // Fall back to a digit-digit boundary.
    let bytes = s.as_bytes();
    let mut pos = avail;
    let floor = if auto_mode { MIN_NUMBER_FOLD_CHUNK } else { 1 };
    while pos > floor {
        // In auto mode both sides must be worth a line, since neither
        // announces what it is, and requiring it of the remainder is what
        // stops a long number ending in a two-digit widow. Fixed mode is
        // asking for an exact width and gets it, tail and all.
        if (!auto_mode || s.len() - pos >= MIN_NUMBER_FOLD_CHUNK)
            && bytes[pos - 1].is_ascii_digit()
            && bytes[pos].is_ascii_digit()
        {
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

// ---------------------------------------------------------------------------
// Visible character boundaries -- enforced on every fold path.
//
// Spec: "FOLDING IN THE MIDDLE OF A DATA CHARACTER OR IN THE MIDDLE OF A
// VISIBLE CHARACTER IS ALWAYS FORBIDDEN IN EVERY CONTEXT". That rule is
// currently unenforced: folding a string containing a ZWJ emoji sequence
// splits the cluster across two fold lines. It still round trips, because the
// escapes preserve the bytes, so this is a conformance and display fault
// rather than data loss.
//
// Everything below implements the test the rule needs, including decoding
// `\uXXXX` escapes -- necessary because fold points are chosen against the
// rendered string, where a joiner appears as six ASCII characters rather than
// as U+200D.
//
// The escape rule and the cluster rule interact: backing a split off to a
// cluster boundary can land inside an escape sequence, and backing off for the
// escape can land inside a cluster. An attempt that applied them in sequence
// traded one violation for the other, which is why `floor_legal_split` tests
// both together on each step back rather than applying them in turn.
//
// It runs in `fold_lines`, so every folder in the crate goes through it. That is
// deliberately a second pass after `floor_safe_fold_point`: the whitelist there
// picks a *good* place to fold and is tuned for readability, while this enforces
// a specification MUST. Keeping the invariant separate from the heuristic means
// retuning the heuristic can never quietly stop enforcing the rule.
// ---------------------------------------------------------------------------

/// Is `ch` a regional indicator? Two in a row render as one flag.
fn is_regional_indicator(ch: char) -> bool {
    matches!(ch, '\u{1F1E6}'..='\u{1F1FF}')
}

/// Does `ch` continue the visible character before it rather than start a new one?
///
/// Spec: "anything displayed together is also a single visible character, like an
/// emoji for instance." These are the pieces that join to what precedes them.
fn continues_visible_character(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    ) || matches!(
        ch,
        '\u{200D}'                    // zero width joiner
            | '\u{FE00}'..='\u{FE0F}' // variation selectors
            | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
            | '\u{1F3FB}'..='\u{1F3FF}' // emoji skin tone modifiers
            | '\u{1160}'..='\u{11FF}' // hangul vowel and trailing jamo
    )
}

/// The character logically at byte index `i`, seeing through a leading
/// `\uXXXX` escape and surrogate pair. The mirror of `logical_char_before`,
/// and needed for the same reason: a joiner written as an escape must read as
/// a joiner from both sides of a candidate split.
fn logical_char_after(s: &str, i: usize) -> Option<char> {
    let tail = s.get(i..)?;
    if let Some(cp) = leading_unicode_escape(tail) {
        return Some(cp);
    }
    tail.chars().next()
}

/// Decode a `\uXXXX` (or surrogate pair) sitting at the very start of `tail`.
fn leading_unicode_escape(tail: &str) -> Option<char> {
    let one = |t: &str| -> Option<u32> {
        let b = t.as_bytes();
        if b.len() < 6 || b[0] != b'\\' || b[1] != b'u' {
            return None;
        }
        u32::from_str_radix(t.get(UNICODE_ESCAPE_HEX_START..UNICODE_ESCAPE_LEN)?, 16).ok()
    };
    let high = one(tail)?;
    if (0xD800..0xDC00).contains(&high) {
        let low = one(tail.get(UNICODE_ESCAPE_LEN..)?)?;
        if (0xDC00..0xE000).contains(&low) {
            let scalar = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(scalar);
        }
        return None;
    }
    char::from_u32(high)
}

/// The character logically preceding byte index `i`, seeing through a `\uXXXX`
/// escape and through a surrogate pair, so a joiner that has been escaped still
/// reads as a joiner.
///
/// Fold points are chosen against the *rendered* string, where a zero width
/// joiner is the six characters `\u200d` rather than U+200D. Testing visible
/// character boundaries on that text without decoding would see an ASCII `d`
/// followed by an emoji and call it a boundary, which is how a family emoji
/// ended up split across two fold lines.
fn logical_char_before(s: &str, i: usize) -> Option<char> {
    let head = &s[..i];
    if let Some(cp) = trailing_unicode_escape(head) {
        return Some(cp);
    }
    head.chars().next_back()
}

/// Decode a `\uXXXX` (or surrogate pair) sitting at the very end of `head`.
fn trailing_unicode_escape(head: &str) -> Option<char> {
    let one = |h: &str| -> Option<u32> {
        let bytes = h.as_bytes();
        let n = bytes.len();
        if n < UNICODE_ESCAPE_LEN
            || bytes[n - UNICODE_ESCAPE_LEN] != b'\\'
            || bytes[n - UNICODE_ESCAPE_LEN + 1] != b'u'
        {
            return None;
        }
        // An odd run of backslashes before the `u` means this one is escaped.
        let mut slashes = 0;
        let mut k = n - UNICODE_ESCAPE_LEN;
        while k > 0 && head.as_bytes()[k - 1] == b'\\' {
            slashes += 1;
            k -= 1;
        }
        if slashes % 2 == 1 {
            return None;
        }
        u32::from_str_radix(&h[n - UNICODE_ESCAPE_HEX..], 16).ok()
    };

    let low = one(head)?;
    if (0xDC00..0xE000).contains(&low) {
        // Low surrogate: pull the high one in front of it to rebuild the pair.
        let high = one(&head[..head.len() - UNICODE_ESCAPE_LEN])?;
        if (0xD800..0xDC00).contains(&high) {
            let scalar = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(scalar);
        }
        return None;
    }
    char::from_u32(low)
}

/// Would a split at byte `i` land inside an escape sequence?
///
/// `safe_json_split` only knows about two-character `\X` escapes: it looks at
/// the backslashes immediately behind the split, so a cut inside `\u200d`
/// looks safe to it because the character before is a hex digit. Escapes are
/// scanned forward from the start here, so a `\uXXXX` -- and a surrogate
/// pair, which must not be separated or it becomes a lone surrogate -- is
/// treated as the single indivisible unit it is.
fn splits_an_escape(s: &str, i: usize) -> bool {
    let b = s.as_bytes();
    let mut k = 0usize;
    while k < b.len() && k < i {
        if b[k] != b'\\' {
            k += 1;
            continue;
        }
        let len = if b.get(k + 1) == Some(&b'u') {
            let high = s
                .get(k + UNICODE_ESCAPE_HEX_START..k + UNICODE_ESCAPE_LEN)
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .unwrap_or(0);
            let paired = (0xD800..0xDC00).contains(&high)
                && b.get(k + UNICODE_ESCAPE_LEN) == Some(&b'\\')
                && b.get(k + UNICODE_ESCAPE_LEN + 1) == Some(&b'u');
            if paired { 12 } else { 6 }
        } else {
            2
        };
        if i > k && i < k + len {
            return true;
        }
        k += len;
    }
    false
}

/// Back a chosen split off to the nearest position at or before it that does
/// not sit inside a visible character.
///
/// Spec: "FOLDING IN THE MIDDLE OF A DATA CHARACTER OR IN THE MIDDLE OF A
/// VISIBLE CHARACTER IS ALWAYS FORBIDDEN IN EVERY CONTEXT". Every fold path
/// funnels its final answer through here rather than testing the rule itself,
/// so there is one definition of what a visible character is and it cannot
/// drift between the bare, quoted, key and number folders.
pub(crate) fn floor_legal_split(s: &str, split_at: usize) -> usize {
    let mut i = split_at.min(s.len());
    loop {
        if i == 0 {
            return 0;
        }
        if s.is_char_boundary(i)
            // Not inside an escape sequence...
            && !splits_an_escape(s, i)
            // ...and not inside a visible character.
            && is_visible_character_boundary(s, i)
        {
            return i;
        }
        i -= 1;
    }
}


/// May a fold be placed at byte index `i`, or would it cut a visible character?
fn is_visible_character_boundary(s: &str, i: usize) -> bool {
    if i == 0 || i == s.len() {
        return true;
    }
    let after = logical_char_after(s, i).expect("i < len and on a char boundary");
    let before = logical_char_before(s, i).expect("i > 0 and on a char boundary");

    if before == '\u{200D}' {
        return false; // a joiner always binds to what follows
    }
    if continues_visible_character(after) {
        return false;
    }
    if is_regional_indicator(before) && is_regional_indicator(after) {
        // Flags pair up, so only every second indicator starts a new one.
        let run = s[..i]
            .chars()
            .rev()
            .take_while(|c| is_regional_indicator(*c))
            .count();
        return run % 2 == 0;
    }
    true
}

/// Is a fold at byte index `i` *provably* safe, with no Unicode tables involved?
///
/// This is a whitelist on purpose, and the direction matters more than the
/// contents. A blacklist -- "these code points continue the character before
/// them" -- fails **open**: a sequence Unicode adds next year is not on the list,
/// so the folder concludes it may cut there and splits a glyph. A whitelist fails
/// **closed**: an unrecognised neighbour is simply not provably safe, so the fold
/// is declined and the line runs long. The spec permits the second outcome
/// ("either overflow the width or use indent glyphs") and forbids the first
/// outright, so the asymmetry in the code should match the asymmetry in the spec.
///
/// It also keeps a simple generator possible. These two facts need no tables and
/// cannot be invalidated by a Unicode revision:
///
/// - a space combines with nothing;
/// - ASCII has no combining forms.
///
/// A generator that folds only here is conformant forever without shipping any
/// Unicode data. This implementation may add more -- see
/// [`is_extended_safe_fold_point`] -- but only ever by *adding* proofs, never by
/// assuming safety it cannot demonstrate.
fn is_known_safe_fold_point(s: &str, i: usize) -> bool {
    if i == 0 || i >= s.len() {
        return false; // a fold must have data on both sides
    }
    if !s.is_char_boundary(i) {
        return false;
    }
    let after = s[i..].chars().next().expect("i < len, on a boundary");
    let before = s[..i].chars().next_back().expect("i > 0, on a boundary");
    if after == ' ' || before == ' ' {
        return true;
    }
    before.is_ascii() && after.is_ascii()
}

/// Fold points this implementation can prove safe using the Unicode data it
/// already carries, on top of [`is_known_safe_fold_point`].
///
/// Only additions, and only where the proof is a *stable* property rather than a
/// catalogue that grows: two adjacent ideographs are separable because neither
/// combines with the other, which is a fact about the blocks, not about any
/// particular sequence. Nothing here is required of a conforming generator, and
/// if this data were stale the effect is a missed fold, never a split glyph.
fn is_extended_safe_fold_point(s: &str, i: usize) -> bool {
    if i == 0 || i >= s.len() || !s.is_char_boundary(i) {
        return false;
    }
    let after = s[i..].chars().next().expect("checked");
    let before = s[..i].chars().next_back().expect("checked");
    is_separable_ideograph(before) && is_separable_ideograph(after)
}

/// CJK ideographs and kana, which stand alone: they take no combining marks in
/// normal text and join nothing to either side.
fn is_separable_ideograph(ch: char) -> bool {
    matches!(ch,
        '\u{3040}'..='\u{30FF}'   // hiragana, katakana
        | '\u{3400}'..='\u{4DBF}' // CJK ext A
        | '\u{4E00}'..='\u{9FFF}' // CJK unified
        | '\u{F900}'..='\u{FAFF}' // compatibility ideographs
    )
}

/// Largest byte index at or before `budget` where a fold is provably safe, or 0
/// when there is none -- in which case the caller must not fold at all.
pub(crate) fn floor_safe_fold_point(s: &str, budget: Columns) -> usize {
    let mut i = budget.spent_in(s);
    while i > 0 {
        if is_known_safe_fold_point(s, i) || is_extended_safe_fold_point(s, i) {
            return i;
        }
        i -= 1;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
    }
    0
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
/// Split a table row for a `/ ` continuation, at a space inside a cell's value.
///
/// `max_columns` is a budget in columns, not a byte length. The two coincide only
/// for ASCII, and comparing a byte length against a column budget folded a row of
/// CJK at a third of the intended width while the same table in Latin text did
/// not fold at all.
///
/// **The space at the split rides with the second half.** A fold joins its two
/// lines by concatenation and contributes nothing of its own, so a character
/// dropped here is a character gone from the document: this used to skip the
/// space, and `"alpha beta"` came back from a folded row as `"alphabeta"`
/// whenever the split landed inside a value rather than in a cell's padding.
pub(crate) fn split_table_row_for_fold(
    row: &str,
    max_columns: Columns,
) -> Option<(String, String)> {
    if Columns::of(row) <= max_columns {
        return None;
    }
    let bytes = row.as_bytes();
    // Where the budget runs out, in bytes: the start of the character one past
    // it, or the end of the row if it has fewer characters than that.
    let scan_end = max_columns.spent_in(row);
    // Walk back for a split point inside a cell value -- a space preceded by
    // something other than the `|` that opens a cell or the padding after it.
    let mut pos = scan_end;
    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b' ' && pos > 0 && bytes[pos - 1] != b'|' && bytes[pos - 1] != b' ' {
            let before = row[..pos].to_owned();
            // `pos` is an ASCII space, so it is a character boundary and this
            // slice cannot split one.
            let after = row[pos..].to_owned();
            return Some((before, after));
        }
    }
    None
}

// The four lookalike questions are asked through a `ParseOptions` and nowhere
// else -- `SPEC_FORMS` for the generator, which always emits specification
// TJSON, and the caller's own options for the parser. There is deliberately no
// free `is_comma_like(ch)`: a call has to say which reading it means, because
// the two can disagree.

pub(crate) fn is_reserved_word(s: &str) -> bool {
    matches!(s, "true" | "false" | "null" | "[]" | "{}" | "\"\"") // "" is logically reserved but unreachable: '"' is quote-like and forbidden as a bare string first/last char
}

/// Which bare string rule a candidate broke.
///
/// The parser used to report every one of these as `invalid bare string`, which
/// tells the reader that a rule exists but not which one, and several of these
/// characters are invisible or easy to misread. Naming the rule turns the error
/// into something actionable without the reader going back to the spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BareStringFault {
    Empty,
    LeadingSpace,
    TrailingSpace,
    LeadingSlash,
    LeadingUnderscore,
    LeadingPipe,
    TrailingPipe,
    LeadingDoubleQuote,
    LeadingQuote,
    TrailingQuote,
    LeadingComma,
    TrailingComma,
    LeadingSquareBracket,
    TrailingSquareBracket,
    LeadingCurlyBracket,
    TrailingCurlyBracket,
    ConsecutiveSpaces,
    ForbiddenChar(char),
}

impl BareStringFault {
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Empty => {
                "a bare string cannot be empty; the empty string is written as \"\"".to_owned()
            }
            Self::LeadingSpace => {
                "a bare string cannot begin with a space; the one space before it is already \
                 its opening quote, and a second one starts a new value"
                    .to_owned()
            }
            Self::TrailingSpace => {
                "a bare string cannot end with a space; trailing whitespace is never data"
                    .to_owned()
            }
            Self::LeadingSlash => {
                "a bare string cannot begin with `/` or a character shaped like one; that \
                 would read as a comment or a fold marker -- double quote it"
                    .to_owned()
            }
            Self::LeadingPipe => {
                "a bare string cannot begin with a pipe or pipelike character; that would read \
                 as a table row or a multiline string -- double quote it"
                    .to_owned()
            }
            Self::TrailingPipe => {
                "a bare string cannot end with a pipe or pipelike character; that would read \
                 as the edge of a table row -- double quote it"
                    .to_owned()
            }
            // Split from `LeadingQuote` because "double quote it instead" is
            // useless advice to someone who already did. A leading `"` is
            // almost always a correctly quoted string that landed one column
            // too far right: at the structural column a quote opens a string,
            // and one space past it is where a bare string begins. So the
            // character is not the mistake here, the space in front of it is,
            // and that is what the message names.
            //
            // Says nothing about which column is the right one, because this
            // fault is reached from both sides: a misplaced quoted *key* also
            // arrives here, since at one space too far right the parser is
            // reading a value and has no key to complain about yet. Deleting
            // the space is the fix either way.
            Self::LeadingDoubleQuote => {
                "this is a double quoted string with an extra space in front of it, which put \
                 it where a bare string goes; delete that one space before the opening \" and \
                 it parses as the quoted string it looks like"
                    .to_owned()
            }
            // The low line is not forbidden because it is confusing in itself --
            // it is forbidden because it has a job in that exact position. A
            // BARE STRING opens with a space, and a writer who wants that
            // opening to be visible may write `_` in its place, so a `_` at the
            // start of the data would sit where the marker sits and be read as
            // one.
            Self::LeadingUnderscore => {
                "a bare string cannot begin with `_` or a character shaped like one; that \
                 position belongs to the optional marker that makes a bare string's opening \
                 space visible, so a leading one would be read as the marker rather than as \
                 data -- double quote the string"
                    .to_owned()
            }
            Self::LeadingQuote => {
                "a bare string cannot begin with a quote character -- double quote it instead"
                    .to_owned()
            }
            Self::TrailingQuote => {
                "a bare string cannot end with a quote character -- double quote it instead"
                    .to_owned()
            }
            Self::LeadingComma => {
                "a bare string cannot begin with a comma -- double quote it, or give it its \
                 own line"
                    .to_owned()
            }
            Self::TrailingComma => {
                "a bare string cannot end with a comma or be packed with commas, as we may \
                 not know what was meant -- double quote this string, or give it its own line"
                    .to_owned()
            }
            // What a bracket is mistaken for is not the same at the two ends, so
            // the two messages are not the same message.
            //
            // A *leading* one has three readings to be confused with: the `[ `
            // marker that opens a level, the first half of `[]`, and the opening
            // of MINIMAL JSON.
            //
            // A *trailing* one closes something in exactly one place: the end of
            // MINIMAL JSON. Nowhere else does TJSON close a container with a
            // bracket -- containers close by indentation -- and `[]` is a
            // two-character spelling of the empty array rather than an open and a
            // close. Saying "it would read as an array closing here" would
            // describe a construct the format does not have.
            //
            // Interior brackets stay legal and both messages say so, since `a[b`
            // being fine is otherwise surprising next to `[ab` not being.
            Self::LeadingSquareBracket => {
                "a bare string cannot begin with `[` or `]`, or a character shaped like one; \
                 `[ ` opens an array level, `[]` is the whole spelling of an empty array, and \
                 MINIMAL JSON opens with `[` -- so this reads as nesting that is not there. \
                 Double quote it. Inside the string they are fine: `a[b` is a bare string."
                    .to_owned()
            }
            Self::TrailingSquareBracket => {
                "a bare string cannot end with `[` or `]`, or a character shaped like one; the \
                 one place a trailing `]` closes anything is the end of MINIMAL JSON, as in \
                 `k:[90,85,92]`, and that is enough for a reader to take this for one. Double \
                 quote it. Inside the string they are fine: `a[b` is a bare string."
                    .to_owned()
            }
            Self::LeadingCurlyBracket => {
                "a bare string cannot begin with `{` or `}`, or a character shaped like one; \
                 `{ ` opens an object level, `{}` is the whole spelling of an empty object, \
                 and MINIMAL JSON opens with `{` -- so this reads as nesting that is not \
                 there. Double quote it. Inside the string they are fine: `a{b` is a bare \
                 string."
                    .to_owned()
            }
            Self::TrailingCurlyBracket => {
                "a bare string cannot end with `{` or `}`, or a character shaped like one; the \
                 one place a trailing `}` closes anything is the end of MINIMAL JSON, as in \
                 `k:{\"a\":1}`, and that is enough for a reader to take this for one. Double \
                 quote it. Inside the string they are fine: `a{b` is a bare string."
                    .to_owned()
            }
            Self::ConsecutiveSpaces => {
                "a bare string cannot contain two spaces in a row; two spaces separate values"
                    .to_owned()
            }
            Self::ForbiddenChar(ch) => format!(
                "a bare string cannot contain U+{:04X}; double quote the string so the character \
                 is escaped",
                ch as u32
            ),
        }
    }
}

/// Why `value` is not a valid bare string, or `None` if it is one.
///
/// The check order is the order the rules are stated in the spec, and it decides
/// which fault a string with more than one problem reports. First and last
/// character rules come before the interior scan, so the reported fault is the
/// one at the edge the reader is most likely looking at.
/// Why a run of text cannot be a BARE KEY.
///
/// A key that fails the rules otherwise surfaces as "invalid value start",
/// which points at the wrong construct and teaches nothing. TJSON is stricter
/// than comparable formats about what may go unquoted, so a rejection has to
/// say which rule was hit and what to do instead -- most people meet the edges
/// of the format through these messages rather than through the specification.
#[derive(Clone, Copy)]
pub(crate) enum BareKeyFault {
    Empty,
    LeadingNotLetterOrNumber(char),
    LeadingPipe(char),
    LeadingComma(char),
    LeadingQuote(char),
    Colonlike(char),
    ForbiddenChar(char),
    TrailingSpace,
    TrailingPipe(char),
    TrailingComma(char),
    TrailingQuote(char),
    ConsecutiveSpaces,
}

impl BareKeyFault {
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Empty => "a key cannot be empty; write \"\" for the empty key".to_owned(),
            Self::LeadingNotLetterOrNumber(ch) => format!(
                "a bare key must begin with a letter or a number, not {} -- \
                 double quote the key",
                show(ch)
            ),
            Self::LeadingPipe(ch) => format!(
                "a bare key cannot begin with {}, a pipelike character; a line \
                 beginning with a pipe is a table row -- double quote the key",
                show(ch)
            ),
            Self::LeadingComma(ch) => format!(
                "a bare key cannot begin with {}, a commalike character; it would \
                 read as an array separator -- double quote the key",
                show(ch)
            ),
            Self::LeadingQuote(ch) => format!(
                "a bare key cannot begin with {}, a quotelike character; it would \
                 read as a quoted key -- double quote the key instead",
                show(ch)
            ),
            Self::Colonlike(ch) => format!(
                "a bare key cannot contain {}, a colonlike character, anywhere; a \
                 colon is what separates a key from its value, so a character \
                 drawn like one would leave the split ambiguous to a reader -- \
                 double quote the key",
                show(ch)
            ),
            Self::ForbiddenChar(ch) => format!(
                "a bare key cannot contain {}; control, invisible and combining \
                 characters are not allowed unquoted because they cannot be seen \
                 -- double quote the key and escape it",
                show(ch)
            ),
            Self::TrailingSpace => "a bare key cannot end with a space; the space \
                 would be invisible before the colon -- double quote the key"
                .to_owned(),
            Self::TrailingPipe(ch) => format!(
                "a bare key cannot end with {}, a pipelike character; it would read \
                 as the edge of a table row -- double quote the key",
                show(ch)
            ),
            Self::TrailingComma(ch) => format!(
                "a bare key cannot end with {}, a commalike character; it would read \
                 as an array separator -- double quote the key",
                show(ch)
            ),
            Self::TrailingQuote(ch) => format!(
                "a bare key cannot end with {}, a quotelike character; it would read \
                 as a quoted key -- double quote the key instead",
                show(ch)
            ),
            Self::ConsecutiveSpaces => "a bare key cannot contain two spaces in a \
                 row; two spaces are what separate one packed key-value pair from \
                 the next -- double quote the key"
                .to_owned(),
        }
    }
}

/// Render a character for a diagnostic: itself when it can be seen, its code
/// point when it cannot.
fn show(ch: char) -> String {
    if ch == ' ' {
        return "a space".to_owned();
    }
    if is_weird_class_char(ch) || ch.is_control() {
        return format!("U+{:04X}", ch as u32);
    }
    format!("`{}` (U+{:04X})", ch, ch as u32)
}

/// The first rule a candidate bare key breaks, under `forms`.
///
/// Mirrors [`check_bare_string`], and checks the rules in the order the
/// specification states them so the message names the first thing a reader
/// would notice.
pub(crate) fn check_bare_key(key: &str, forms: &ParseOptions) -> Result<(), BareKeyFault> {
    let Some(first) = key.chars().next() else {
        return Err(BareKeyFault::Empty);
    };
    let last = key.chars().next_back().expect("non-empty");

    if forms.is_pipe_like(first) {
        return Err(BareKeyFault::LeadingPipe(first));
    }
    if forms.is_comma_like(first) {
        return Err(BareKeyFault::LeadingComma(first));
    }
    if forms.is_quote_like(first) {
        return Err(BareKeyFault::LeadingQuote(first));
    }
    if !is_unicode_letter_or_number(first) {
        return Err(BareKeyFault::LeadingNotLetterOrNumber(first));
    }

    if last == ' ' {
        return Err(BareKeyFault::TrailingSpace);
    }
    if forms.is_pipe_like(last) {
        return Err(BareKeyFault::TrailingPipe(last));
    }
    if forms.is_comma_like(last) {
        return Err(BareKeyFault::TrailingComma(last));
    }
    if forms.is_quote_like(last) {
        return Err(BareKeyFault::TrailingQuote(last));
    }

    let mut previous_space = false;
    for ch in key.chars() {
        if forms.is_colon_like(ch) {
            return Err(BareKeyFault::Colonlike(ch));
        }
        if ch != ' ' && is_weird_class_char(ch) {
            return Err(BareKeyFault::ForbiddenChar(ch));
        }
        if ch == ' ' {
            if previous_space {
                return Err(BareKeyFault::ConsecutiveSpaces);
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
    }
    Ok(())
}



/// The first bare string rule `value` breaks, under `forms`.
///
/// Emptying the PIPELIKE set lets `abc\u{2502}` through; `abc|` still fails,
/// because the vertical line is tested here directly and is in no set.
pub(crate) fn check_bare_string(
    value: &str,
    forms: &ParseOptions,
) -> Result<(), BareStringFault> {
    let Some(first) = value.chars().next() else {
        return Err(BareStringFault::Empty);
    };
    let last = value.chars().next_back().unwrap();

    if first == ' ' {
        return Err(BareStringFault::LeadingSpace);
    }
    if last == ' ' {
        return Err(BareStringFault::TrailingSpace);
    }
    // Start only, both of these. A `/` or a `_` inside a bare string sits in
    // running text where nothing structural begins, and neither one closes
    // anything -- unlike the pipe and the quote, which are barred at both ends
    // because a reader meets them as edges.
    if forms.is_foreslash_like(first) {
        return Err(BareStringFault::LeadingSlash);
    }
    if forms.is_underscore_like(first) {
        return Err(BareStringFault::LeadingUnderscore);
    }
    if forms.is_pipe_like(first) {
        return Err(BareStringFault::LeadingPipe);
    }
    if forms.is_pipe_like(last) {
        return Err(BareStringFault::TrailingPipe);
    }
    // Before the quotelike test, which would otherwise swallow it: `"` is a
    // member of the quotelike set as well as being the real thing.
    if first == '"' {
        return Err(BareStringFault::LeadingDoubleQuote);
    }
    if forms.is_quote_like(first) {
        return Err(BareStringFault::LeadingQuote);
    }
    if forms.is_quote_like(last) {
        return Err(BareStringFault::TrailingQuote);
    }
    if forms.is_comma_like(first) {
        return Err(BareStringFault::LeadingComma);
    }
    if forms.is_comma_like(last) {
        return Err(BareStringFault::TrailingComma);
    }
    // Spec, BARE STRINGS: "the limitation is not sided, in that neither side of a
    // square bracket or curly brace can appear at the beginning, and neither side
    // ... at the end". So both characters are barred in both positions, and the
    // four tests below are the whole rule rather than an opener/closer pairing.
    //
    // Interior brackets stay legal: `a[b` is a bare string and reads as one,
    // because nothing structural begins in the middle of a value. This is the
    // same start-and-end shape the pipe and the quote have.
    if forms.is_square_bracket_like(first) {
        return Err(BareStringFault::LeadingSquareBracket);
    }
    if forms.is_square_bracket_like(last) {
        return Err(BareStringFault::TrailingSquareBracket);
    }
    if forms.is_curly_bracket_like(first) {
        return Err(BareStringFault::LeadingCurlyBracket);
    }
    if forms.is_curly_bracket_like(last) {
        return Err(BareStringFault::TrailingCurlyBracket);
    }

    let mut previous_space = false;
    for ch in value.chars() {
        // Not governed by the lookalike sets: these are characters that cannot
        // be seen at all rather than characters that resemble syntax, so there
        // is no set to empty and no reading under which they are safe unquoted.
        if ch != ' ' && is_weird_class_char(ch) {
            return Err(BareStringFault::ForbiddenChar(ch));
        }
        if ch == ' ' {
            if previous_space {
                return Err(BareStringFault::ConsecutiveSpaces);
            }
            previous_space = true;
        } else {
            previous_space = false;
        }
    }
    Ok(())
}

/// The generator's question: may this string be written bare in specification
/// TJSON? Always the specification's reading -- what tjson emits does not
/// depend on how some caller elsewhere configured a parser.
pub(crate) fn is_allowed_bare_string(value: &str) -> bool {
    check_bare_string(value, &SPEC_FORMS).is_ok()
}

#[cfg(test)]
mod character_class_tests {
    use super::*;

    /// The ASCII shortcut in `is_weird_class_char`, against the general path.
    #[test]
    fn weird_class_ascii_matches_the_general_path() {
        for cp in 0u32..0x80 {
            let ch = char::from_u32(cp).expect("ASCII is always a char");
            let full = is_forbidden_literal_tjson_char(ch)
                || matches!(
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
                );
            assert_eq!(is_weird_class_char(ch), full, "U+{cp:04X}");
        }
    }

    /// Every `is_*_like` reading, fast path against set search, for all of ASCII.
    ///
    /// The shortcut is only sound because no lookalike set holds ASCII, which
    /// `ParseOptions::checked` refuses outright -- this is the other half of
    /// that guarantee, checking the readings actually behave as the guarantee
    /// says they may.
    #[test]
    fn lookalike_readings_agree_with_their_sets_across_ascii() {
        let forms = SPEC_FORMS;
        for cp in 0u32..0x80 {
            let ch = char::from_u32(cp).expect("ASCII is always a char");
            assert_eq!(forms.is_comma_like(ch), ch == ',', "commalike U+{cp:04X}");
            assert_eq!(forms.is_colon_like(ch), ch == ':', "colonlike U+{cp:04X}");
            assert_eq!(forms.is_pipe_like(ch), ch == '|', "pipelike U+{cp:04X}");
            assert_eq!(forms.is_underscore_like(ch), ch == '_', "underscorelike U+{cp:04X}");
            assert_eq!(forms.is_foreslash_like(ch), ch == '/', "foreslashlike U+{cp:04X}");
            assert_eq!(
                forms.is_quote_like(ch),
                matches!(ch, '"' | '\'' | '`'),
                "quotelike U+{cp:04X}"
            );
        }
    }

    /// The ASCII shortcut in `is_forbidden_literal_tjson_char` must answer
    /// exactly what the full chain answers, for every ASCII character. Checked
    /// against the chain itself rather than against a remembered list, so the
    /// two cannot drift if a class below is ever widened.
    #[test]
    fn forbidden_literal_ascii_matches_the_general_path() {
        for cp in 0u32..0x80 {
            let ch = char::from_u32(cp).expect("ASCII is always a char");
            // The general path written out, reasons and all, so the shortcut is
            // pinned to *which rule* fires and not merely to the verdict. A fast
            // path returning the right answer for the wrong reason would now be a
            // failure, because the reason reaches the user's error message.
            let general = if is_forbidden_ascii_control_char(ch) {
                Err(ForbiddenLiteral::Control)
            } else if is_c1_control_char(ch) {
                Err(ForbiddenLiteral::C1Control)
            } else if is_default_ignorable_code_point(ch) {
                Err(ForbiddenLiteral::DefaultIgnorable)
            } else if is_private_use_code_point(ch) {
                Err(ForbiddenLiteral::PrivateUse)
            } else if is_noncharacter_code_point(ch) {
                Err(ForbiddenLiteral::Noncharacter)
            } else if matches!(ch, '\u{2028}' | '\u{2029}') {
                Err(ForbiddenLiteral::LineSeparator)
            } else {
                Ok(())
            };
            assert_eq!(
                check_forbidden_literal(ch),
                general,
                "U+{cp:04X} disagrees with the general path"
            );
        }
    }

    /// Spec, QUOTELIKE CHARACTER DEFINITION -- the whole set on one line, so a
    /// spec change is a one-line diff here and the two cannot drift apart
    /// silently the way they did when this was a general category test.
    const QUOTELIKE: &str = "\u{0022}\u{0027}\u{0060}\u{00ab}\u{00bb}\u{2018}\u{2019}\u{201a}\u{201b}\u{201c}\u{201d}\u{201e}\u{201f}\u{2039}\u{203a}\u{2e42}\u{300c}\u{300d}\u{300e}\u{300f}\u{301d}\u{301e}\u{301f}\u{fe41}\u{fe42}\u{fe43}\u{fe44}\u{ff02}\u{ff07}\u{ff62}\u{ff63}";

    /// Spec, PIPELIKE CHARACTER DEFINITION -- likewise.
    const PIPELIKE: &str = "\u{007c}\u{00a6}\u{01c0}\u{01c1}\u{05c0}\u{16c1}\u{2016}\u{2223}\u{2225}\u{23d0}\u{2502}\u{2503}\u{2506}\u{2507}\u{250a}\u{250b}\u{254e}\u{254f}\u{2551}\u{258f}\u{2595}\u{2758}\u{2759}\u{275a}\u{2980}\u{2af4}\u{2afc}\u{2afe}\u{2aff}\u{2d4f}\u{fe31}\u{fe33}\u{ff5c}\u{ffe4}\u{1fb70}\u{1fb71}\u{1fb72}\u{1fb73}\u{1fb74}\u{1fb75}";

    #[test]
    fn quotelike_set_matches_the_spec() {
        assert_eq!(QUOTELIKE.chars().count(), 31);
        for ch in QUOTELIKE.chars() {
            assert!(SPEC_FORMS.is_quote_like(ch), "U+{:04X} should be quotelike", ch as u32);
        }
    }

    #[test]
    fn pipelike_set_matches_the_spec() {
        assert_eq!(PIPELIKE.chars().count(), 40);
        for ch in PIPELIKE.chars() {
            assert!(SPEC_FORMS.is_pipe_like(ch), "U+{:04X} should be pipelike", ch as u32);
        }
    }

    /// Characters deliberately outside the sets. The substitution brackets are
    /// `Pi`, which an earlier general-category test wrongly swept in; the wiggly
    /// vertical line and the danda fail the shape test the pipelike set is built
    /// on -- one is visibly wavy, the other is a short mark on the baseline.
    #[test]
    fn near_misses_stay_outside_the_sets() {
        for ch in ['\u{2e02}', '\u{2e03}', '\u{2e04}', '\u{2e05}'] {
            assert!(!SPEC_FORMS.is_quote_like(ch), "U+{:04X} is not a quotation mark", ch as u32);
        }
        for ch in ['\u{2e3e}', '\u{0964}', '\u{0965}'] {
            assert!(!SPEC_FORMS.is_pipe_like(ch), "U+{:04X} is not pipelike", ch as u32);
        }
    }

    /// Both sets are barred at either end of a bare string, and permitted inside
    /// it -- only the first and last characters are restricted.
    #[test]
    fn sets_are_barred_at_the_ends_of_a_bare_string_only() {
        for ch in QUOTELIKE.chars().chain(PIPELIKE.chars()) {
            let leading = format!("{ch}abc");
            let trailing = format!("abc{ch}");
            let interior = format!("a{ch}c");
            assert!(!is_allowed_bare_string(&leading), "leading U+{:04X}", ch as u32);
            if SPEC_FORMS.is_quote_like(ch) {
                assert!(!is_allowed_bare_string(&trailing), "trailing U+{:04X}", ch as u32);
            }
            assert!(is_allowed_bare_string(&interior), "interior U+{:04X}", ch as u32);
        }
    }

    /// Spec, COMMALIKE CHARACTER DEFINITION -- the whole set on one line, as
    /// for QUOTELIKE and PIPELIKE above.
    const COMMALIKE: &str = "\u{002c}\u{02bb}\u{02bc}\u{02bd}\u{060c}\u{066b}\u{201a}\u{2e32}\u{2e34}\u{2e41}\u{2e4c}\u{3001}\u{fe50}\u{fe51}\u{ff0c}\u{ff64}";

    /// Spec, COLONLIKE CHARACTER DEFINITION -- likewise.
    const COLONLIKE: &str = "\u{003a}\u{02d0}\u{02f8}\u{0589}\u{05c3}\u{0703}\u{0704}\u{0903}\u{0a83}\u{0c03}\u{0c83}\u{0d03}\u{16ec}\u{205a}\u{2236}\u{2982}\u{a789}\u{fe13}\u{fe30}\u{ff1a}";

    #[test]
    fn commalike_set_matches_the_spec() {
        assert_eq!(COMMALIKE.chars().count(), 16);
        for ch in COMMALIKE.chars() {
            assert!(SPEC_FORMS.is_comma_like(ch), "U+{:04X} should be commalike", ch as u32);
        }
    }

    #[test]
    fn colonlike_set_matches_the_spec() {
        assert_eq!(COLONLIKE.chars().count(), 20);
        for ch in COLONLIKE.chars() {
            assert!(SPEC_FORMS.is_colon_like(ch), "U+{:04X} should be colonlike", ch as u32);
        }
    }

    /// Each structural character answers `true` to its own question -- a comma
    /// is certainly commalike -- even though it is not a member of the set a
    /// caller can replace. Both halves matter and they pull in opposite
    /// directions, so the predicate is pinned here; that the sets exclude the
    /// structural characters is pinned in `options`, against the table that
    /// pairs them.
    #[test]
    fn structural_characters_are_lookalikes_of_themselves() {
        assert!(SPEC_FORMS.is_comma_like(','));
        assert!(SPEC_FORMS.is_colon_like(':'));
        assert!(SPEC_FORMS.is_pipe_like('|'));
        for quote in ['"', '\'', '`'] {
            assert!(SPEC_FORMS.is_quote_like(quote));
        }
    }
}
