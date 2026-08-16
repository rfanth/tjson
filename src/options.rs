use std::str::FromStr;
use serde::{Deserialize, Serialize};

pub const MIN_WRAP_WIDTH: usize = 20;
pub const DEFAULT_WRAP_WIDTH: usize = 80;
pub(crate) const MIN_FOLD_CONTINUATION: usize = 10;

/// Controls when `/<` / `/>` indent-offset glyphs are emitted to push content to visual indent 0.
///
/// - `Auto` (default): apply glyphs to avoid overflow and reduce screen volume, using a weighted
///   algorithm that considers the overall shape of the object.
/// - `Fixed`: always apply glyphs once the indent depth exceeds a threshold, without waiting for overflow.
/// - `None`: never apply glyphs; content may overflow `wrap_width`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndentGlyphStyle {
    /// Apply glyphs in order to avoid overflow and save screen volume, using an
    /// intelligent weighting algorithm that looks at the entire object shape.
    #[default]
    Auto,
    /// Always apply glyphs past a fixed indent threshold, regardless of overflow.
    Fixed,
    /// Never apply indent-offset glyphs.
    None,
}

impl FromStr for IndentGlyphStyle {
    type Err = String;
    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "auto" => Ok(Self::Auto),
            "fixed" => Ok(Self::Fixed),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "invalid indent glyph style '{input}' (expected one of: auto, fixed, none)"
            )),
        }
    }
}

/// Controls how the `/<` opening glyph of an indent-offset block is placed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndentGlyphMarkerStyle {
    /// `/<` trails the key on the same line: `key: /<` (default).
    #[default]
    Compact,
    /// `/<` appears on its own line at the key's indent level:
    /// ```text
    /// key:
    ///  /<
    /// ```
    Separate,
    // Like `Separate`, but with additional context info after `/<` (reserved for future use).
    // Currently emits the same output as `Separate`.
    // TODO: WISHLIST: decide what info to include with Marked (depth, key path, …)
    //Marked,
}

/// Internal resolved glyph algorithm. Mapped from [`IndentGlyphStyle`] by `indent_glyph_mode()`.
/// Not part of the public API — use [`IndentGlyphStyle`] and [`RenderOptions`] instead.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub(crate) enum IndentGlyphMode {
    /// Fire based on pure geometry: `pair_indent × line_count >= threshold × w²`
    IndentWeighted(f64),
    /// Fire based on content density: `pair_indent × byte_count >= threshold × w²`
    ///
    /// Not yet used on purpose, but planned for later.
    ByteWeighted(f64),
    /// Fire whenever `pair_indent >= w / 2`
    Fixed,
    /// Never fire
    None,
}

pub(crate) fn indent_glyph_mode(options: &RenderOptions) -> IndentGlyphMode {
    match options.indent_glyph_style {
        IndentGlyphStyle::Auto  => IndentGlyphMode::IndentWeighted(0.2),
        IndentGlyphStyle::Fixed => IndentGlyphMode::Fixed,
        IndentGlyphStyle::None  => IndentGlyphMode::None,
    }
}

/// Controls how tables are horizontally repositioned using `/< />` indent-offset glyphs.
///
/// The overflow decision is always made against the table as rendered at its natural indent,
/// before any table-fold continuations are applied.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableUnindentStyle {
    /// Push the table to visual indent 0 using `/< />` glyphs, unless already there.
    /// Applies regardless of `wrap_width`.
    Left,
    /// Push to visual indent 0 only when the table overflows `wrap_width` at its natural
    /// indent. If the table would still overflow even at indent 0, glyphs are not used.
    /// With unlimited width this is effectively `None`. Default.
    #[default]
    Auto,
    /// Push left by the minimum amount needed to fit within `wrap_width` — not necessarily
    /// all the way to 0. If the table fits at its natural indent, nothing moves. With
    /// unlimited width this is effectively `None`.
    Floating,
    /// Never apply indent-offset glyphs to tables, even if the table overflows `wrap_width`
    /// or would otherwise not be rendered.
    None,
}

impl FromStr for TableUnindentStyle {
    type Err = String;
    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "left"     => Ok(Self::Left),
            "auto"     => Ok(Self::Auto),
            "floating" => Ok(Self::Floating),
            "none"     => Ok(Self::None),
            _ => Err(format!(
                "invalid table unindent style '{input}' (expected one of: left, auto, floating, none)"
            )),
        }
    }
}



// ---- Lookalike character sets ----
//
// Each set below holds the characters a reader could mistake for one of TJSON's
// structural characters. The structural character itself is never in the set:
// a comma IS an array separator, it does not merely resemble one, and it is
// tested directly by the code that looks for a separator. That split is what
// makes these sets safe to hand to a caller -- redefining them changes which
// *impostors* are refused, and can never change what a comma does.
//
// Enumerated rather than derived: no Unicode property selects any of them.
// Sorted, because `ParseOptions` binary searches them; `ParseOptions::checked`
// enforces both properties on anything a caller supplies.

/// The COMMALIKE set as the specification enumerates it, less the comma.
pub(crate) const SPEC_COMMALIKE: &[char] = &[
    '\u{02BB}', '\u{02BC}', '\u{02BD}', '\u{060C}', '\u{066B}', '\u{201A}', '\u{2E32}',
    '\u{2E34}', '\u{2E41}', '\u{2E4C}', '\u{3001}', '\u{FE50}', '\u{FE51}', '\u{FF0C}',
    '\u{FF64}',
];

/// The COLONLIKE set as the specification enumerates it, less the colon.
///
/// A colon separates a key from its value, so a character drawn like one inside
/// a bare key would let `a<colonlike>b:1` read as two different splits depending
/// on which colon the reader believes.
pub(crate) const SPEC_COLONLIKE: &[char] = &[
    '\u{02D0}', '\u{02F8}', '\u{0589}', '\u{05C3}', '\u{0703}', '\u{0704}', '\u{0903}',
    '\u{0A83}', '\u{0C03}', '\u{0C83}', '\u{0D03}', '\u{16EC}', '\u{205A}', '\u{2236}',
    '\u{2982}', '\u{A789}', '\u{FE13}', '\u{FE30}', '\u{FF1A}',
];

/// The QUOTELIKE set as the specification enumerates it, less the three quotes
/// the specification names outright (`"`, `'` and the backtick).
///
/// The 28 remaining span Po, Ps, Pe, Pi, Pf and Sk, which is why the test is a
/// list. An earlier version tested `InitialPunctuation | FinalPunctuation` and
/// reached only 10 of them -- it let the whole corner bracket family through
/// (`「』﹁｣`), which open and close quotations in Japanese exactly as `"` does
/// in English, while wrongly catching the `⸂⸃⸄⸅` substitution brackets, which
/// are not quotes at all.
pub(crate) const SPEC_QUOTELIKE: &[char] = &[
    '\u{00AB}', '\u{00BB}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}', '\u{201C}',
    '\u{201D}', '\u{201E}', '\u{201F}', '\u{2039}', '\u{203A}', '\u{2E42}', '\u{300C}',
    '\u{300D}', '\u{300E}', '\u{300F}', '\u{301D}', '\u{301E}', '\u{301F}', '\u{FE41}',
    '\u{FE42}', '\u{FE43}', '\u{FE44}', '\u{FF02}', '\u{FF07}', '\u{FF62}', '\u{FF63}',
];

/// The PIPELIKE set as the specification enumerates it, less the vertical line.
///
/// The test is shape, not frequency -- any character rendering as a full-height
/// vertical stroke qualifies, however rare, because the confusion it creates at
/// the start of a line is the same either way. That is why the click letters and
/// the runic letter are in it despite being letters, and why the danda family is
/// not: those are short marks on the baseline rather than full-height strokes.
pub(crate) const SPEC_PIPELIKE: &[char] = &[
    '\u{00A6}', '\u{01C0}', '\u{01C1}', '\u{05C0}', '\u{16C1}', '\u{2016}', '\u{2223}',
    '\u{2225}', '\u{23D0}', '\u{2502}', '\u{2503}', '\u{2506}', '\u{2507}', '\u{250A}',
    '\u{250B}', '\u{254E}', '\u{254F}', '\u{2551}', '\u{258F}', '\u{2595}', '\u{2758}',
    '\u{2759}', '\u{275A}', '\u{2980}', '\u{2AF4}', '\u{2AFC}', '\u{2AFE}', '\u{2AFF}',
    '\u{2D4F}', '\u{FE31}', '\u{FE33}', '\u{FF5C}', '\u{FFE4}', '\u{1FB70}', '\u{1FB71}',
    '\u{1FB72}', '\u{1FB73}', '\u{1FB74}', '\u{1FB75}',
];

/// The FORESLASHLIKE set as the specification enumerates it, less the solidus.
///
/// The test is shape: a single straight stroke leaning from the bottom left to
/// the top right. Doubled and dotted forms are not members -- a reader tells
/// `⫽` and `⹊` apart from `/` at a glance -- and neither are the tapering
/// calligraphic strokes such as U+4E3F, which curve at the foot.
///
/// This set does more work than the others. The solidus is genuinely parse
/// critical: it opens a fold continuation and both indent offset glyphs. So a
/// lookalike at the start of a BARE STRING is read as a fold marker by a person
/// even though it is not one to the parser, which is the confusion this
/// prevents.
pub(crate) const SPEC_FORESLASHLIKE: &[char] = &[
    '\u{1735}', '\u{2044}', '\u{2215}', '\u{2571}', '\u{29F8}', '\u{FF0F}',
];

/// The UNDERSCORELIKE set as the specification enumerates it, less the low line.
///
/// The test is shape: a single solid straight stroke resting on the floor of the
/// cell. Doubled, dashed and wavy low lines are not members -- a reader tells
/// `‗`, `﹍` and `﹏` apart from `_` at a glance, so they are different marks
/// rather than impostors. Orientation excludes the rest: a vertical low line
/// such as U+FE33 is never mistaken for a floor stroke, and is a PIPELIKE
/// character instead.
///
/// The low line matters because it may stand where the space that opens a BARE
/// STRING would go, for a writer who wants to be unmistakable. A character that
/// looks like one must not be able to sit in that slot and be read as the
/// marker.
pub(crate) const SPEC_UNDERSCORELIKE: &[char] = &[
    '\u{02CD}', '\u{23BC}', '\u{23BD}', '\u{2581}', '\u{FF3F}',
];

/// The SQUAREBRACKETLIKE set as the specification enumerates it, less the two
/// square brackets.
///
/// The test is shape, and only shape: a character whose outline a reader would
/// take for a square bracket, whatever its width, weight or name. That is a
/// narrower question than the one [`SPEC_QUOTELIKE`] asks, which admits
/// characters that do a quote's *job* without looking like one.
///
/// `[` opens an array level as `[ ` and spells the empty array as `[]`, so a
/// lookalike at either end of a BARE STRING reads as nesting that is not there.
pub(crate) const SPEC_SQUAREBRACKETLIKE: &[char] = &[
    '\u{2772}', '\u{2773}', '\u{27E6}', '\u{27E7}', '\u{298B}', '\u{298C}', '\u{3010}',
    '\u{3011}', '\u{301A}', '\u{301B}', '\u{FF3B}', '\u{FF3D}',
];

/// The CURLYBRACKETLIKE set as the specification enumerates it, less the two
/// curly brackets.
///
/// The same shape test as [`SPEC_SQUAREBRACKETLIKE`], applied to a brace: a
/// vertical stroke with a mid-height point and curled ends. Smaller than its
/// neighbour because fewer characters are drawn that way.
///
/// `{` opens an object level as `{ ` and spells the empty object as `{}`.
pub(crate) const SPEC_CURLYBRACKETLIKE: &[char] =
    &['\u{2774}', '\u{2775}', '\u{FF5B}', '\u{FF5D}'];

/// The characters the format is actually built on, paired with the set of
/// impostors each one attracts. A caller may replace a set; it may never touch
/// this column, and `ParseOptions::checked` refuses a set that tries.
const STRUCTURAL_COMMA: &[char] = &[','];
const STRUCTURAL_COLON: &[char] = &[':'];
const STRUCTURAL_PIPE: &[char] = &['|'];
/// Three, because the specification names three: `"` and the backtick open
/// strings and `'` is reserved alongside them.
const STRUCTURAL_QUOTE: &[char] = &['"', '\'', '`'];
/// One, because the low line is the only character that may stand where a BARE
/// STRING's opening space goes.
const STRUCTURAL_UNDERSCORE: &[char] = &['_'];
/// One, because the solidus is the only character that opens a fold or an
/// indent offset glyph.
const STRUCTURAL_FORESLASH: &[char] = &['/'];
/// Two, and the pair is why: the specification bars *both* from *both* ends of a
/// BARE STRING -- "the limitation is not sided" -- so a closer opening a string
/// is as much a misreading as an opener does.
const STRUCTURAL_SQUAREBRACKET: &[char] = &['[', ']'];
/// Two, for the same reason as [`STRUCTURAL_SQUAREBRACKET`].
const STRUCTURAL_CURLYBRACKET: &[char] = &['{', '}'];

/// How the parser reads bare strings and bare keys.
///
/// The four lookalike sets are the point of this type. They are the *only*
/// thing about the bare forms a caller may move, and they exist so that a
/// policy change stays survivable: narrowing what may appear unquoted turns
/// documents that were valid when written into documents that will not load,
/// and a caller holding the older sets can still read them. Handing back the
/// definition is also what keeps the parser from carrying these lists around --
/// it asks its options what counts, and only the structural characters are
/// written into the code that looks for them.
///
/// Not public and not exposed through the CLI. [`SPEC_FORMS`] is the default
/// and is what every ordinary caller gets.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ParseOptions {
    pub(crate) start_indent: usize,
    // Private, unlike `start_indent`: these can only be set through the builders
    // below, which is where a set containing a structural character is caught.
    // A struct literal elsewhere in the crate would bypass that check.
    commalike: &'static [char],
    colonlike: &'static [char],
    pipelike: &'static [char],
    quotelike: &'static [char],
    underscorelike: &'static [char],
    foreslashlike: &'static [char],
    squarebracketlike: &'static [char],
    curlybracketlike: &'static [char],

    /// Spaces at the end of a line, where they carry nothing. No reading of
    /// this reaches inside a multiline string body, where a trailing space is
    /// data like any other character.
    pub(crate) trailing_spaces: TrailingSpaces,
    /// A comment landing inside a fold, which the specification does not allow.
    pub(crate) comment_placement_error: CommentPlacementError,
    /// A byte order mark at the very start of the input.
    pub(crate) byte_order_mark: ByteOrderMark,
    /// A key whose value is indented more than one level below it, with no
    /// marker chain saying how deep it goes.
    pub(crate) missing_indent_marker: MissingIndentMarker,
    /// How little a MULTILINE STRING is allowed to hold.
    pub(crate) multiline_minimum: MultilineMinimum,
    // TODO: duplicate keys. Today the last one silently wins, which is not a
    // decision anyone made -- it is what the map insert happens to do, and no
    // test pins it. Other JSON implementations keep the first or the last and
    // almost nothing preserves both; a policy would also cover the case where
    // we know the value is headed somewhere that accepts only one.
    // serde_json is last, so that's nice for now.
    // `duplicate_keys: Reject | KeepFirst | KeepLast | Preserve`.
    //
    // TODO: several TJSON values in one input. Nothing to decide until TJSON
    // Lines exists. `multiple_values: Reject | Stop`.
}

/// What to do with spaces at the end of a line that carry no data.
///
/// Spec: "TRAILING SPACES ARE TREATED AS ERRORS BY DEFAULT WHERE NOT
/// MEANINGFUL". The "by default" is this option; "where not meaningful" is why
/// neither reading reaches inside a multiline string body.
// Only the tests construct the non-default readings today. They exist ahead of
// their consumer on purpose: the point of this type is that a preset can be
// assembled from them later without the policy having to be rediscovered at
// each site that applies it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TrailingSpaces {
    /// An error naming the spaces and how to be rid of them. The
    /// specification's reading.
    #[default]
    Reject,
    /// Read the line as though they had not been typed.
    Discard,
}

/// What to do with a comment inside a fold.
///
/// Spec: "A comment may not be within a fold." A fold is one value spread over
/// several lines, so a comment landing in the middle of one has no position in
/// the document -- there is no node it sits before.
// Only the tests construct the non-default readings today. They exist ahead of
// their consumer on purpose: the point of this type is that a preset can be
// assembled from them later without the policy having to be rediscovered at
// each site that applies it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CommentPlacementError {
    /// An error naming the comment and where it could go instead. The
    /// specification's reading.
    #[default]
    Reject,
    /// Lift the comment to just before the value the fold belongs to, the
    /// nearest position that exists. The comment survives and the document
    /// re-renders as legal TJSON -- what a `--fix` pass wants.
    Hoist,
    /// Drop the comment and keep the value.
    ///
    /// The one reading that loses something a person wrote, which is why it is
    /// named for what it does. It was once the accidental behaviour on the
    /// table path, where a comment inside a folded row vanished with nothing
    /// reported; a caller may still want it when only the data matters, but it
    /// should have to say so.
    Discard,
}

/// What to do with a byte order mark at the start of the input.
///
/// U+FEFF is a zero-width no-break space, so a document beginning with one has
/// an invisible first character. At byte 0 it is usually not content at all:
/// Windows editors add it when saving UTF-8, so the author neither typed it nor
/// can see it. Anywhere else in the input it stays forbidden under both
/// readings -- there it really is an invisible character sitting inside data.
// Only the tests construct the non-default readings today. They exist ahead of
// their consumer on purpose: the point of this type is that a preset can be
// assembled from them later without the policy having to be rediscovered at
// each site that applies it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ByteOrderMark {
    /// An error saying a mark is present and how to remove it. The
    /// specification's reading: TJSON has no encoding preamble.
    #[default]
    Reject,
    /// Parse from the character after it. Byte offsets in spans are then
    /// relative to the input with the mark removed.
    Discard,
}

/// FOR TESTING ONLY DO NOT USE NON-DEFAULT VALUE
/// 
/// What to do with a key whose value is indented more than one level below it.
///
/// One level down is ordinary nesting and needs no marker. Deeper than that,
/// TJSON asks for an explicit chain -- `[ [ 3` rather than an extra two spaces
/// -- because the chain also says what each level *is*, which indentation alone
/// does not.
///
/// This is the one rule in the format that is conditional on how far a value
/// moved, and conditional rules are the ones a writer drops. That matters most
/// for the writers who cannot be corrected: a language model emits indentation
/// correctly because indentation is positional, and omits the marker because
/// "required on a multi-level jump" is a clause it has to remember to apply.
///
/// [`Infer`](Self::Infer) is not a guess. The depth is already written down --
/// it is the indentation -- and each level's kind follows from the format
/// rather than from a heuristic:
///
/// - Levels 1 through N-1 can only be arrays. An object cannot sit directly
///   inside an object; it would need a key, and there is no line for one.
/// - Level N is whatever the content there says it is, decided by exactly the
///   test that decides it for an ordinary one-level nesting: a key followed by
///   a colon makes an object, anything else an array. So a colon inside a bare
///   string (`k: 10:30:00`) cannot mislead it, because that test never reads a
///   colon that does not terminate a key.
///
/// The reading it produces is therefore the only one the document can have --
/// which is why this can be a policy at all, rather than a repair.
// Only the tests construct the non-default reading today. It is deliberately not
// reachable from the CLI or the public API yet: the default must keep behaving
// exactly as the specification says while the inference gets exercised.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MissingIndentMarker {
    /// An error. The specification's reading: the depth is expressible only
    /// with a marker chain, so a document without one is not TJSON.
    #[default]
    Reject,
    /// FOR TESTING ONLY DO NOT USE - VIOLATES SPECIFICATION
    /// 
    /// Read the missing levels off the indentation, as described above.
    /// This is not to ever be exposed or used, the point of this option is to
    /// test and sharpen the internal structure of the parser, not to be used
    /// by the public.
    /// 
    /// Infer here is not specification compliant.
    Infer,
    /// Require the generator to force indent markers on in order to parse, and
    /// not even generate one level without a marker.  This does not force the
    /// '[' in '  key:[ 9' vs '  key:  9' as that one isn't in the indent,
    /// and it's incredibly ugly.
    /// 
    /// This exists primarily as a way to test force_markers.
    RequireForced,
}

/// How little a MULTILINE STRING is allowed to hold.
///
/// Neither reading accepts an empty one. The specification gives exactly one
/// way to write the empty string -- `""` -- and a fence holding nothing would
/// be a second, so that stays an error however this is set.
///
/// The two differ only on a string with content but no line break in it.
///
/// Neither reading is a generator setting. The generator obeys the linefeed
/// rule unconditionally: every path that emits a fence is gated on the value
/// really holding a newline, so no setting here makes this library write one
/// that does not. What is being chosen is only how tolerant the *reader* is of
/// a fence someone else wrote.
// Only the tests construct the non-default reading today. It exists ahead of its
// consumer on purpose: the point of this type is that a preset can be assembled
// from these later without the policy having to be rediscovered at each site
// that applies it.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MultilineMinimum {
    /// At least one real data EOL in the multiline.
    ///
    /// Stricter than the specification, deliberately. The linefeed is a SHOULD,
    /// so a fence holding one unbroken line is TJSON and this refuses it
    /// anyway -- the reading for a caller who would rather hear about it,
    /// because on the writing side a single-line fence is usually a mistake: a
    /// generator that reached for a fence it did not need, or an edit that
    /// deleted every line but one. That guess may be wrong sometimes (the
    /// specification's own example of a fence used for deliberate emphasis is
    /// exactly this shape), so it cannot be the default.
    Eol,
    /// At least one character, linefeed or not.
    /// 
    /// This is the straight from the spec. The specification's reading:
    /// "the actual string data being displayed SHOULD contain at least one
    /// linefeed and is REQUIRED to contain at least one data character", and
    /// again among the requirements, "MULTILINE STRINGS SHOULD contain at least
    /// one real unescaped newline, but MUST contain at least one character in
    /// order to parse."  Also, a "" is the only allowed empty string.
    ///
    /// The floor a multiline has to clear to be TJSON at all is one
    /// character, which is what a parser applies by default. A fence around a
    /// single line is legal and means what it says (no data EOL); but it is
    /// not something a generator should normally pick on its own.
    /// 
    /// A multiline that contains exactly one character, an EOL, is also valid.
    #[default]
    Character,
}

/// The specification's own reading -- every set exactly as TJSON defines it.
///
/// This is [`ParseOptions::default`], and it is also what the generator writes
/// against: `util::is_comma_like` and its siblings are this constant asked the
/// question, so the emitter and a default parser cannot disagree about what a
/// lookalike is.
pub(crate) const SPEC_FORMS: ParseOptions = ParseOptions {
    start_indent: 0,
    commalike: SPEC_COMMALIKE,
    colonlike: SPEC_COLONLIKE,
    pipelike: SPEC_PIPELIKE,
    quotelike: SPEC_QUOTELIKE,
    underscorelike: SPEC_UNDERSCORELIKE,
    foreslashlike: SPEC_FORESLASHLIKE,
    squarebracketlike: SPEC_SQUAREBRACKETLIKE,
    curlybracketlike: SPEC_CURLYBRACKETLIKE,
    trailing_spaces: TrailingSpaces::Reject,
    comment_placement_error: CommentPlacementError::Reject,
    byte_order_mark: ByteOrderMark::Reject,
    missing_indent_marker: MissingIndentMarker::Reject,
    multiline_minimum: MultilineMinimum::Character,
};

impl Default for ParseOptions {
    fn default() -> Self {
        SPEC_FORMS
    }
}

/// Every lookalike set that exists, paired with the structural characters it
/// may not contain. The builders validate one entry; `spec_sets_obey_the
/// _rules_they_impose` validates all four of the constants, which are otherwise
/// never checked -- [`SPEC_FORMS`] is a struct literal and goes nowhere near a
/// builder.
#[cfg(test)]
const LOOKALIKE_SETS: [(&str, &[char], &[char]); 8] = [
    ("commalike", STRUCTURAL_COMMA, SPEC_COMMALIKE),
    ("colonlike", STRUCTURAL_COLON, SPEC_COLONLIKE),
    ("pipelike", STRUCTURAL_PIPE, SPEC_PIPELIKE),
    ("quotelike", STRUCTURAL_QUOTE, SPEC_QUOTELIKE),
    ("underscorelike", STRUCTURAL_UNDERSCORE, SPEC_UNDERSCORELIKE),
    ("foreslashlike", STRUCTURAL_FORESLASH, SPEC_FORESLASHLIKE),
    ("squarebracketlike", STRUCTURAL_SQUAREBRACKET, SPEC_SQUAREBRACKETLIKE),
    ("curlybracketlike", STRUCTURAL_CURLYBRACKET, SPEC_CURLYBRACKETLIKE),
];

impl ParseOptions {
    /// Accept a lookalike set, or say why it is not one.
    ///
    /// Refuses a set that would redefine a structural character, that is
    /// unsorted, or that repeats itself. The structural check is the one that
    /// matters. The other two are here because the sets are binary searched, so
    /// an unsorted set does not merely perform badly -- it silently fails to
    /// match, which would look like a lookalike being accepted rather than like
    /// a mistake in the call.
    ///
    /// Runs once per set installed, not once per character tested. It cannot
    /// move to compile time for a caller's set: a `&'static [char]` arriving
    /// through a builder is opaque to this crate until it is handed over. The
    /// sets this crate owns are a different matter and are pinned by test.
    ///
    /// `set_name` is only ever one of the four literals below. It is a
    /// `&'static str` so it stays that way -- a name assembled at runtime would
    /// make the errors below unpredictable text rather than fixed prose.
    fn checked(
        set_name: &'static str,
        structural: &'static [char],
        lookalikes: &'static [char],
    ) -> std::result::Result<&'static [char], String> {
        for &ch in lookalikes {
            if structural.contains(&ch) {
                return Err(format!(
                    "the {set_name} set may not contain `{ch}` (U+{:04X}): that character is \
                     not something that resembles TJSON syntax, it is TJSON syntax, and the \
                     parser tests for it directly. These sets hold only the characters a \
                     reader could mistake for it.",
                    ch as u32
                ));
            }
        }

        // No lookalike is ASCII, and this is what makes that true of a set a
        // caller supplies rather than only of the ones below. The classification
        // fast paths lean on it: for an ASCII character every `is_*_like` test
        // reduces to comparing against the structural character itself, with no
        // set to search. A set holding ASCII would silently break them, and
        // silently is the problem -- so it is refused here instead.
        //
        // Nothing is lost. A lookalike is a character that RESEMBLES a
        // structural one, the structural ones are all ASCII and are held apart
        // from these sets, and no ASCII character resembles another closely
        // enough to be confused with it in a monospace font.
        for &ch in lookalikes {
            if ch.is_ascii() {
                return Err(format!(
                    "the {set_name} set may not contain `{ch}` (U+{:04X}): lookalike sets hold \
                     no ASCII, because an ASCII character is either a structural character in \
                     its own right or is not mistakable for one. The parser's fast paths read \
                     that as a guarantee and stop searching the set for ASCII input.",
                    ch as u32
                ));
            }
        }

        for pair in lookalikes.windows(2) {
            if pair[0] == pair[1] {
                return Err(format!(
                    "the {set_name} set lists U+{:04X} twice",
                    pair[0] as u32
                ));
            }
            if pair[0] > pair[1] {
                return Err(format!(
                    "the {set_name} set must be sorted by code point, but U+{:04X} precedes \
                     U+{:04X}; the set is binary searched, so an unsorted one would quietly \
                     stop matching some of its own members",
                    pair[0] as u32, pair[1] as u32
                ));
            }
        }

        Ok(lookalikes)
    }

    /// Replace the COMMALIKE set. The comma itself is not a member and cannot
    /// be made one -- see [`Self::checked`].
    #[allow(dead_code)] // Recovery path: no caller in the library or CLI selects it yet.
    pub(crate) fn commalike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.commalike = Self::checked("commalike", STRUCTURAL_COMMA, set)?;
        Ok(self)
    }

    /// Replace the COLONLIKE set.
    #[allow(dead_code)]
    pub(crate) fn colonlike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.colonlike = Self::checked("colonlike", STRUCTURAL_COLON, set)?;
        Ok(self)
    }

    /// Replace the PIPELIKE set.
    #[allow(dead_code)]
    pub(crate) fn pipelike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.pipelike = Self::checked("pipelike", STRUCTURAL_PIPE, set)?;
        Ok(self)
    }

    /// Replace the QUOTELIKE set.
    #[allow(dead_code)]
    pub(crate) fn quotelike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.quotelike = Self::checked("quotelike", STRUCTURAL_QUOTE, set)?;
        Ok(self)
    }

    /// Replace the FORESLASHLIKE set.
    #[allow(dead_code)]
    pub(crate) fn foreslashlike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.foreslashlike = Self::checked("foreslashlike", STRUCTURAL_FORESLASH, set)?;
        Ok(self)
    }

    /// Replace the UNDERSCORELIKE set.
    #[allow(dead_code)]
    pub(crate) fn underscorelike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.underscorelike = Self::checked("underscorelike", STRUCTURAL_UNDERSCORE, set)?;
        Ok(self)
    }

    /// Replace the SQUAREBRACKETLIKE set.
    #[allow(dead_code)]
    pub(crate) fn squarebracketlike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.squarebracketlike = Self::checked("squarebracketlike", STRUCTURAL_SQUAREBRACKET, set)?;
        Ok(self)
    }

    /// Replace the CURLYBRACKETLIKE set.
    #[allow(dead_code)]
    pub(crate) fn curlybracketlike(
        mut self,
        set: &'static [char],
    ) -> std::result::Result<Self, String> {
        self.curlybracketlike = Self::checked("curlybracketlike", STRUCTURAL_CURLYBRACKET, set)?;
        Ok(self)
    }

    /// How to read spaces at the end of a line that carry no data.
    #[allow(dead_code)] // Awaiting the presets these are here to be assembled into.
    pub(crate) fn trailing_spaces(mut self, policy: TrailingSpaces) -> Self {
        self.trailing_spaces = policy;
        self
    }

    /// What to do with a comment that lands inside a fold.
    #[allow(dead_code)]
    pub(crate) fn comment_placement_error(mut self, policy: CommentPlacementError) -> Self {
        self.comment_placement_error = policy;
        self
    }

    /// What to do with a byte order mark at the start of the input.
    #[allow(dead_code)]
    pub(crate) fn byte_order_mark(mut self, policy: ByteOrderMark) -> Self {
        self.byte_order_mark = policy;
        self
    }

    /// What to do with a value indented more than one level below its key with
    /// no marker chain.
    #[allow(dead_code)]
    pub(crate) fn missing_indent_marker(mut self, policy: MissingIndentMarker) -> Self {
        self.missing_indent_marker = policy;
        self
    }

    /// How little a MULTILINE STRING may hold.
    #[allow(dead_code)]
    pub(crate) fn multiline_minimum(mut self, policy: MultilineMinimum) -> Self {
        self.multiline_minimum = policy;
        self
    }

    /// Would a reader take `ch` for an array separator?
    ///
    /// True for the comma itself as well as for its lookalikes -- a comma is
    /// certainly commalike. It is still not what to call when looking for a
    /// separator: that is `ch == ','`, written out where the split happens.
    /// This answers a question about deception, and deception is policy.
    pub(crate) fn is_comma_like(&self, ch: char) -> bool {
        // ASCII holds no lookalikes (`ParseOptions::checked` enforces it), so for
        // ASCII the structural character is the whole question.
        if ch.is_ascii() {
            return ch == ',';
        }
        self.commalike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for a key/value separator? Not how to find one;
    /// see [`Self::is_comma_like`].
    pub(crate) fn is_colon_like(&self, ch: char) -> bool {
        // ASCII holds no lookalikes (`ParseOptions::checked` enforces it), so for
        // ASCII the structural character is the whole question.
        if ch.is_ascii() {
            return ch == ':';
        }
        self.colonlike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for a table cell delimiter? Not how to find one;
    /// see [`Self::is_comma_like`].
    pub(crate) fn is_pipe_like(&self, ch: char) -> bool {
        // ASCII holds no lookalikes (`ParseOptions::checked` enforces it), so for
        // ASCII the structural character is the whole question.
        if ch.is_ascii() {
            return ch == '|';
        }
        self.pipelike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for a quote? Not how to find one; see
    /// [`Self::is_comma_like`].
    pub(crate) fn is_quote_like(&self, ch: char) -> bool {
        if ch.is_ascii() {
            return STRUCTURAL_QUOTE.contains(&ch);
        }
        self.quotelike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for the solidus that opens a fold or an indent
    /// offset glyph? Not how to find one; see [`Self::is_comma_like`].
    pub(crate) fn is_foreslash_like(&self, ch: char) -> bool {
        if ch.is_ascii() {
            return ch == '/';
        }
        self.foreslashlike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for the low line that may open a BARE STRING?
    /// Not how to find one; see [`Self::is_comma_like`].
    pub(crate) fn is_underscore_like(&self, ch: char) -> bool {
        if ch.is_ascii() {
            return ch == '_';
        }
        self.underscorelike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for a square bracket? Not how to find one; see
    /// [`Self::is_comma_like`].
    ///
    /// Both brackets answer true, in either position. The specification bars
    /// both from both ends of a BARE STRING, so there is no side to this
    /// question and no caller that wants only one of them.
    pub(crate) fn is_square_bracket_like(&self, ch: char) -> bool {
        if ch.is_ascii() {
            return STRUCTURAL_SQUAREBRACKET.contains(&ch);
        }
        self.squarebracketlike.binary_search(&ch).is_ok()
    }

    /// Would a reader take `ch` for a curly bracket? Not how to find one; see
    /// [`Self::is_comma_like`]. Unsided, like [`Self::is_square_bracket_like`].
    pub(crate) fn is_curly_bracket_like(&self, ch: char) -> bool {
        if ch.is_ascii() {
            return STRUCTURAL_CURLYBRACKET.contains(&ch);
        }
        self.curlybracketlike.binary_search(&ch).is_ok()
    }
}

/// Options controlling how TJSON is rendered. Use [`RenderOptions::default`] for sensible
/// defaults, or [`RenderOptions::canonical`] for a compact, diff-friendly format.
/// All fields are set via builder methods.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderOptions {
    pub(crate) wrap_width: Option<usize>,
    pub(crate) start_indent: usize,
    pub(crate) force_markers: bool,
    pub(crate) bare_strings: StringStyle,
    pub(crate) bare_keys: BareStyle,
    pub(crate) inline_objects: bool,
    pub(crate) inline_arrays: bool,
    pub(crate) string_array_style: StringArrayStyle,
    pub(crate) number_fold_style: FoldStyle,
    pub(crate) string_bare_fold_style: FoldStyle,
    pub(crate) string_quoted_fold_style: FoldStyle,
    pub(crate) string_multiline_fold_style: FoldStyle,
    pub(crate) tables: bool,
    pub(crate) table_fold: bool,
    pub(crate) table_unindent_style: TableUnindentStyle,
    pub(crate) indent_glyph_style: IndentGlyphStyle,
    pub(crate) indent_glyph_marker_style: IndentGlyphMarkerStyle,
    pub(crate) table_min_rows: usize,
    pub(crate) table_min_columns: usize,
    pub(crate) table_min_similarity: f32,
    pub(crate) table_column_max_width: Option<usize>,
    /// Undocumented. Use at your own risk — may be discontinued at any time.
    pub(crate) kv_pack_multiple: usize,
    pub(crate) multiline_strings: bool,
    pub(crate) multiline_style: MultilineStyle,
    pub(crate) multiline_min_lines: usize,
    pub(crate) multiline_max_lines: usize,
    pub(crate) eol: Eol,
    // ---- Annotation policy (Document rendering) ----
    // Recording is mechanism, normalizing is policy: these flags decide whether the
    // renderer honors presentation facts recorded on Document nodes. They have no
    // effect when rendering a plain Value, which carries no facts.
    pub(crate) honor_string_forms: bool,
    pub(crate) honor_key_forms: bool,
    pub(crate) honor_tables: bool,
    /// Comments are content, not presentation: they get their own switch, and presets
    /// that normalize layout (canonical) still keep them by default.
    pub(crate) render_comments: bool,
}

/// Controls how long strings are folded across lines using `/ ` continuation markers.
///
/// - `Auto` (default): prefer folding immediately after EOL characters, and at whitespace to word boundaries to fit `wrap_width`.
/// - `Fixed`: fold right at, or if it violates specification (e.g. not between two data characters), immediately before, `wrap_width`.
/// - `None`: do not fold, even if it means overflowing past `wrap_width`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FoldStyle {
    /// Prefer folding immediately after EOL characters, and immediately before
    /// whitespace boundaries to fit `wrap_width`.
    #[default]
    Auto,
    /// Fold right at, or if it violates specification (e.g. not between two data
    /// characters), immediately before, `wrap_width`.
    Fixed,
    /// Do not fold, even if it means overflowing past `wrap_width`.
    None,
}

impl FromStr for FoldStyle {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "auto" => Ok(Self::Auto),
            "fixed" => Ok(Self::Fixed),
            "none" => Ok(Self::None),
            _ => Err(format!(
                "invalid fold style '{input}' (expected one of: auto, fixed, none)"
            )),
        }
    }
}

/// Controls which multiline string format is preferred when rendering strings with newlines.
///
/// Only affects strings that contain at least one EOL (LF or CRLF). Single-line strings
/// always follow the normal `bare_strings` / `string_quoted_fold_style` options.
///
/// - `Bold` (` `` `, default): body pinned to col 2, each content line begins with `| `. Always safe.
/// - `Floating` (`` ` ``): single backtick, body at natural indent `n+2`. Falls back to `Bold`
///   (col 2) on overflow, when the string exceeds `multiline_max_lines`, or when content is
///   pipe-heavy / backtick-starting.
/// - `BoldFloating` (` `` `): same format as `Bold`; body at natural indent `n+2` when it fits,
///   otherwise falls back to col 2.
/// - `Transparent` (` ``` `): triple backtick, body at col 0. Falls back to `Bold` when content is
///   pipe-heavy or has backtick-starting lines (visually unsafe in that format).
/// - `Light` (`` ` `` or ` `` `): prefers `` ` ``; falls back to ` `` ` like `Floating`, but the
///   fallback reason differs — see variant doc for details.
/// - `FoldingQuotes` (JSON string with `/ ` folds): never uses any multiline string format.
///   Renders EOL-containing strings as folded JSON strings. When the encoded string is within
///   25 % of `wrap_width` from fitting, it is emitted unfolded (overrunning the limit is
///   preferred over a fold that saves almost nothing).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultilineStyle {
    /// Single-backtick (`` ` ``); body at natural indent `n+2`. Falls back to `Bold` (col 2)
    /// on overflow, excessive length, or pipe-heavy / backtick-starting content.
    Floating,
    /// ` `` `: body at col 2, each content line begins with `| `. Always safe.
    #[default]
    Bold,
    /// Same ` `` ` format as `Bold`; body at natural indent `n+2` when it fits within
    /// `wrap_width`, otherwise falls back to col 2.
    BoldFloating,
    /// Same ` `` ` format as `BoldFloating` — body at natural indent `n+2` — but never
    /// falls back to the left margin: width overflow does not move the body, and the
    /// pipe-guarded body has no unsafe content to force a move. The `` `` `` analog of
    /// `Light`.
    BoldLight,
    /// ` ``` ` with body at col 0; falls back to `Bold` when content is pipe-heavy or
    /// starts with backtick characters. `string_multiline_fold_style` has no effect here —
    /// `/ ` continuations are not allowed inside triple-backtick blocks.
    Transparent,
    /// `` ` `` preferred; falls back to ` `` ` only when content looks like TJSON markers
    /// (pipe-heavy or backtick-starting lines). Width overflow and line count do NOT trigger
    /// fallback — a long `` ` `` is preferred over the heavier ` `` ` format.
    Light,
    /// Always a JSON string for EOL-containing strings; folds with `/ ` to fit `wrap_width`
    /// unless the overrun is within 25 % of `wrap_width`.
    FoldingQuotes,
}

impl FromStr for MultilineStyle {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "bold" => Ok(Self::Bold),
            "floating" => Ok(Self::Floating),
            "bold-floating" => Ok(Self::BoldFloating),
            "bold-light" => Ok(Self::BoldLight),
            "transparent" => Ok(Self::Transparent),
            "light" => Ok(Self::Light),
            "folding-quotes" => Ok(Self::FoldingQuotes),
            _ => Err(format!(
                "invalid multiline style '{input}' (expected one of: bold, floating, bold-floating, bold-light, transparent, light, folding-quotes)"
            )),
        }
    }
}

/// Controls whether bare (unquoted) strings and keys are preferred.
///
/// - `Prefer` (default): use bare strings/keys when the value is safe to represent without quotes.
/// - `None`: always quote strings and keys.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// How a string value announces that it is a string.
///
/// The three readings are one axis, and it runs from JSON's habits toward
/// TJSON's own. `Quoted` falls back to the mark JSON uses. `Bare` uses TJSON's
/// mark, the single space in front of the value, which is invisible. `Marked`
/// writes that same space as `_` so it can be seen -- not a decoration borrowed
/// from elsewhere, but the format's own opening quote turned up.
///
/// Keys are a separate question and keep [`BareStyle`]: a key sits at the
/// structural column with no opening space in front of it, so there is no slot
/// for a marker and nothing for a third reading to say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StringStyle {
    /// Always `"value"`.
    Quoted,
    /// ` value` where the rules allow it, with the opening quote invisible.
    #[default]
    Bare,
    /// `_value` where the rules allow it, with the opening quote written out.
    Marked,
}

impl FromStr for StringStyle {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "quoted" => Ok(Self::Quoted),
            "bare" => Ok(Self::Bare),
            "marked" => Ok(Self::Marked),
            _ => Err(format!(
                "invalid string style '{input}' (expected one of: quoted, bare, marked)"
            )),
        }
    }
}

/// Controls how a packed array line is put together.
///
/// Every variant is a rule about a *line*, not about the array. A line is packed
/// or it is not; only a packed line has a format; and only array format 2 costs a
/// bare-able string its quotes. So an array whose elements land on separate lines
/// can hold bare and quoted elements at once, and `Comma` does not mean "quote the
/// whole array" -- it means "pack lines with commas".
///
/// The variants form a ladder, each rung taking one more thing away from strings
/// and none of them saying anything about anything else. A string-free array is
/// laid out identically under all five, so adding a string to an array never
/// changes how its numbers are packed:
///
/// - `Comma`: always pack lines with commas, accepting quotes on strings that had
///   no other reason to be quoted.
/// - `PreferComma`: comma pack when it strictly saves a line, and not otherwise.
/// - `PreferSpaces` (default): split the array into runs of similarly formattable
///   elements, one run per line, keeping strings bare where they can be.
/// - `Spaces`: never comma pack a string, bare-able or not, so an unbareable one
///   takes a line to itself -- but bare-able strings still space pack together.
/// - `None`: no string shares a line with anything. Non-strings still pack; to
///   stop arrays packing at all, use `inline_arrays(false)`.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StringArrayStyle {
    Spaces,
    // The default, and it must stay the one `RenderOptions::default` installs --
    // two answers to "what is the default" in one file is how they drift.
    #[default]
    PreferSpaces,
    Comma,
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

/// The end-of-line sequence written between output lines.
///
/// Prefer `Lf`. It is the default on every platform, keeps output byte-identical and
/// canonical, and — per the spec's LF↔CRLF round-trip guarantee — survives conversion to
/// CRLF and back without data loss, so a consumer that wants CRLF can almost always convert
/// at its own boundary. Being on Windows is not itself a reason to switch: modern Windows
/// tooling is largely LF-tolerant. Reach for `CrLf` only when a specific consumer genuinely
/// requires CRLF and cannot convert on its own — for instance a CRLF-native text pipeline
/// that would otherwise rewrite the bytes and break a byte-exact comparison against what was
/// emitted. Choosing `CrLf` gives up canonical, cross-platform-identical output.
///
/// This sets the output's line endings, including those between the lines of a multiline
/// string block. It does not change what that data *means*: a multiline string's local EOL
/// indicator (the human-readable `\n` / `\r\n` marker) records the data's own line ending and
/// is preserved regardless of this setting.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Eol {
    /// Separate output lines with `\n` (line feed). Default, canonical, and the right choice
    /// in nearly all cases.
    #[default]
    Lf,
    /// Separate output lines with `\r\n` (carriage return + line feed). Non-canonical; use
    /// only when a specific consumer genuinely requires CRLF — not merely because the platform
    /// is Windows, which mostly tolerates LF.
    CrLf,
}

impl Eol {
    /// The line-terminator bytes written between output lines (`"\n"` for `Lf`,
    /// `"\r\n"` for `CrLf`). Useful when appending your own trailing newline in the
    /// same style as the rendered output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

impl FromStr for Eol {
    type Err = String;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        match input {
            "lf" => Ok(Self::Lf),
            "crlf" => Ok(Self::CrLf),
            _ => Err(format!(
                "invalid eol '{input}' (expected one of: lf, crlf)"
            )),
        }
    }
}

impl RenderOptions {
    /// Returns options that produce canonical TJSON: one key-value pair per line,
    /// no inline packing, no tables, no multiline strings, no folding.
    pub fn canonical() -> Self {
        Self {
            inline_objects: false,
            inline_arrays: false,
            string_array_style: StringArrayStyle::None,
            tables: false,
            multiline_strings: false,
            number_fold_style: FoldStyle::None,
            string_bare_fold_style: FoldStyle::None,
            string_quoted_fold_style: FoldStyle::None,
            string_multiline_fold_style: FoldStyle::None,
            indent_glyph_style: IndentGlyphStyle::None,
            // Canonical output is a deterministic function of data plus retained
            // content: presentation facts are ignored, comments are kept (strip them
            // with `render_comments(false)` for data-only bytes, e.g. hashing).
            honor_string_forms: false,
            honor_key_forms: false,
            honor_tables: false,
            ..Self::default()
        }
    }

    /// When true (default), honor per-node string forms recorded on a [`crate::Document`]
    /// (bare vs quoted vs which multiline flavor), subject to the usual safety fallbacks.
    /// When false, the global options decide everywhere. No effect on plain [`crate::Value`]s.
    pub fn honor_string_forms(mut self, honor: bool) -> Self {
        self.honor_string_forms = honor;
        self
    }

    /// When true (default), honor per-entry key forms (bare vs quoted) recorded on a
    /// [`crate::Document`]. When false, the global `bare_keys` policy decides everywhere.
    pub fn honor_key_forms(mut self, honor: bool) -> Self {
        self.honor_key_forms = honor;
        self
    }

    /// When true (default), honor per-array table opinions recorded on a
    /// [`crate::Document`]: an array written as a table renders as one (bypassing the
    /// size/similarity heuristics, though not physical impossibility), and an array
    /// written vertically is never table-ified. When false, the table heuristics decide.
    pub fn honor_tables(mut self, honor: bool) -> Self {
        self.honor_tables = honor;
        self
    }

    /// When true (default), emit comments carried by a [`crate::Document`]. When false,
    /// strip them — e.g. for canonical data-only bytes to hash or sign.
    pub fn render_comments(mut self, render: bool) -> Self {
        self.render_comments = render;
        self
    }

    /// When true, force explicit `[` / `{` indent markers even for a only a single n+2
    /// indent jump at a time, that would normally have an implicit indent marker.
    /// Normally, we only use markers when we jump at least two indent steps at once (n+2, n+2 again).
    /// Default is false.
    pub fn force_markers(mut self, force_markers: bool) -> Self {
        self.force_markers = force_markers;
        self
    }

    /// Controls whether string values use bare string format or JSON quoted strings. `Prefer` uses
    /// bare strings whenever the spec permits; `None` always uses JSON quoted strings. Default is `Prefer`.
    pub fn bare_strings(mut self, bare_strings: StringStyle) -> Self {
        self.bare_strings = bare_strings;
        self
    }

    /// Controls whether object keys use bare key format or JSON quoted strings. `Prefer` uses
    /// bare keys whenever the spec permits; `None` always uses JSON quoted strings. Default is `Prefer`.
    pub fn bare_keys(mut self, bare_keys: BareStyle) -> Self {
        self.bare_keys = bare_keys;
        self
    }

    /// When true, pack small objects onto a single line when they fit within `wrap_width`. Default is true.
    pub fn inline_objects(mut self, inline_objects: bool) -> Self {
        self.inline_objects = inline_objects;
        self
    }

    /// When true, pack small arrays onto a single line when they fit within `wrap_width`. Default is true.
    pub fn inline_arrays(mut self, inline_arrays: bool) -> Self {
        self.inline_arrays = inline_arrays;
        self
    }

    /// Controls how packed array lines are put together. Each variant restricts
    /// only what strings may share a line with, so a string-free array is laid out
    /// the same under all of them. Default is `PreferSpaces`; to stop arrays
    /// packing at all, use [`inline_arrays`](Self::inline_arrays).
    pub fn string_array_style(mut self, string_array_style: StringArrayStyle) -> Self {
        self.string_array_style = string_array_style;
        self
    }

    /// When true, render homogeneous arrays of objects as pipe tables when they meet the
    /// minimum row, column, and similarity thresholds. Default is true.
    pub fn tables(mut self, tables: bool) -> Self {
        self.tables = tables;
        self
    }

    /// Set the wrap width. `None` means no wrap limit (infinite width). Values below 20 are
    /// clamped to 20 — use [`wrap_width_checked`](Self::wrap_width_checked) if you want an
    /// error instead.
    pub fn wrap_width(mut self, wrap_width: Option<usize>) -> Self {
        self.wrap_width = wrap_width.map(|w| w.clamp(MIN_WRAP_WIDTH, usize::MAX));
        self
    }

    /// Set the wrap width with validation. `None` means no wrap limit (infinite width).
    /// Returns an error if the value is `Some(n)` where `n < 20`.
    /// Use [`wrap_width`](Self::wrap_width) if you want clamping instead.
    pub fn wrap_width_checked(self, wrap_width: Option<usize>) -> std::result::Result<Self, String> {
        if let Some(w) = wrap_width
            && w < MIN_WRAP_WIDTH {
                return Err(format!("wrap_width must be at least {MIN_WRAP_WIDTH}, got {w}"));
            }
        Ok(self.wrap_width(wrap_width))
    }

    /// Minimum number of data rows an array must have to be rendered as a table. Default is 3.
    pub fn table_min_rows(mut self, table_min_rows: usize) -> Self {
        self.table_min_rows = table_min_rows;
        self
    }

    /// Minimum number of columns a table must have to be rendered as a pipe table. Default is 3.
    pub fn table_min_columns(mut self, table_min_columns: usize) -> Self {
        self.table_min_columns = table_min_columns;
        self
    }

    /// Minimum cell-fill fraction required for table rendering. Computed as
    /// `filled_cells / (rows × columns)` where `filled_cells` is the count of
    /// (row, column) pairs where the row's object actually has that key. A value
    /// of 1.0 requires every row to have every column; 0.0 allows fully sparse
    /// tables. Range 0.0–1.0; default is 0.8.
    pub fn table_min_similarity(mut self, v: f32) -> Self {
        self.table_min_similarity = v;
        self
    }

    /// If any column's content width (including the leading space on bare string values) exceeds
    /// this value, the table is abandoned entirely and falls back to block layout.
    /// `None` means no limit. Default is `Some(40)`.
    pub fn table_column_max_width(mut self, table_column_max_width: Option<usize>) -> Self {
        self.table_column_max_width = table_column_max_width;
        self
    }

    /// Undocumented. Use at your own risk — may be discontinued at any time.
    /// Valid values are 1–4; returns an error otherwise.
    pub fn kv_pack_multiple(mut self, v: usize) -> std::result::Result<Self, String> {
        if !(1..=4).contains(&v) {
            return Err(format!("kv_pack_multiple must be 1–4, got {v}"));
        }
        self.kv_pack_multiple = v;
        Ok(self)
    }

    /// Undocumented. Use at your own risk — may be discontinued at any time.
    /// Sets `kv_pack_multiple` with clamping to 1–4 instead of erroring.
    pub fn kv_pack_multiple_clamped(mut self, v: usize) -> Self {
        self.kv_pack_multiple = v.clamp(1, 4);
        self
    }

    /// Set all four fold styles at once. Individual fold options override this if set after.
    pub fn fold(self, style: FoldStyle) -> Self {
        self.number_fold_style(style)
            .string_bare_fold_style(style)
            .string_quoted_fold_style(style)
            .string_multiline_fold_style(style)
    }

    /// Fold style for numbers. `Auto` folds before `.`/`e`/`E` first, then between digits.
    /// `Fixed` folds between any two digits at the wrap limit. Default is `Auto`.
    pub fn number_fold_style(mut self, style: FoldStyle) -> Self {
        self.number_fold_style = style;
        self
    }

    /// Whether and how to fold long bare strings and bare keys across lines using `/ ` continuation
    /// markers. Applies to both string values and object keys rendered in bare format. Default is `Auto`.
    pub fn string_bare_fold_style(mut self, style: FoldStyle) -> Self {
        self.string_bare_fold_style = style;
        self
    }

    /// Whether and how to fold long quoted strings and quoted keys across lines using `/ ` continuation
    /// markers. Applies to both string values and object keys rendered in JSON quoted format. Default is `Auto`.
    pub fn string_quoted_fold_style(mut self, style: FoldStyle) -> Self {
        self.string_quoted_fold_style = style;
        self
    }

    /// Fold style within `` ` `` and ` `` ` multiline string bodies. Default is `None`.
    ///
    /// Note: ` ``` ` (`Transparent`) multilines cannot fold regardless of this setting —
    /// the spec does not allow `/ ` continuations inside triple-backtick blocks.
    pub fn string_multiline_fold_style(mut self, style: FoldStyle) -> Self {
        self.string_multiline_fold_style = style;
        self
    }

    /// @experimental When true, emit `/ ` fold continuations for wide table lines. Off by default;
    /// the spec notes that table folds are almost always a bad idea.
    pub fn table_fold(mut self, table_fold: bool) -> Self {
        self.table_fold = table_fold;
        self
    }

    /// Controls whether wide tables are repositioned toward the left margin using ` /<' and ` />` indent
    /// glyphs. Default is `Auto`. This is independent of [`indent_glyph_style`](Self::indent_glyph_style).
    pub fn table_unindent_style(mut self, style: TableUnindentStyle) -> Self {
        self.table_unindent_style = style;
        self
    }

    /// Controls whether deeply-nested objects and arrays are wrapped in `/< />` glyphs
    /// and repositioned toward the left margin to reduce visual depth. Default is `Auto`.
    ///
    /// This applies to objects and arrays only — it is independent of table repositioning,
    /// which is controlled by [`table_unindent_style`](Self::table_unindent_style).
    pub fn indent_glyph_style(mut self, style: IndentGlyphStyle) -> Self {
        self.indent_glyph_style = style;
        self
    }

    /// Controls whether the `/<` opening glyph trails its key on the same line (`Compact`)
    /// or appears on its own line (`Separate`). Default is `Compact`.
    pub fn indent_glyph_marker_style(mut self, style: IndentGlyphMarkerStyle) -> Self {
        self.indent_glyph_marker_style = style;
        self
    }

    /// When true, render strings containing newlines using multiline syntax (`` ` ``, ` `` `, or ` ``` `).
    /// When false, all strings are rendered as JSON strings. Default is true.
    pub fn multiline_strings(mut self, multiline_strings: bool) -> Self {
        self.multiline_strings = multiline_strings;
        self
    }

    /// Selects the multiline string format: minimal (`` ` ``), bold (` `` `), or transparent (` ``` `),
    /// each with different body positioning and fallback rules. See [`MultilineStyle`] for the full
    /// breakdown. Default is `Bold`.
    pub fn multiline_style(mut self, multiline_style: MultilineStyle) -> Self {
        self.multiline_style = multiline_style;
        self
    }

    /// Minimum number of newlines a string must contain to be rendered as multiline.
    /// 0 is treated as 1. Default is 1.
    pub fn multiline_min_lines(mut self, multiline_min_lines: usize) -> Self {
        self.multiline_min_lines = multiline_min_lines;
        self
    }

    /// Maximum number of content lines before `Floating` falls back to `Bold`. 0 means no limit. Default is 10.
    pub fn multiline_max_lines(mut self, multiline_max_lines: usize) -> Self {
        self.multiline_max_lines = multiline_max_lines;
        self
    }

    /// Sets the end-of-line sequence written between output lines. `Lf` (default) uses `\n`;
    /// `CrLf` uses `\r\n`. This affects the output's line endings only — it does not change
    /// the local EOL indicator carried by multiline string data. Default is `Lf`.
    pub fn eol(mut self, eol: Eol) -> Self {
        self.eol = eol;
        self
    }
}

impl Default for RenderOptions {
    // SYNC: several numeric literals below are echoed BY HAND in multiple docs as
    // "(default: N)" / "Default: `N`" — the CLI --help (`help_text()` in src/bin/tjson.rs),
    // the builder-method rustdoc above, the TS types (`StringifyOptions` in src/wasm.rs), and
    // the option tables in README.md, npm-README.md, and ../tjson-udf/README.md. They are
    // deliberately NOT shared via public constants: constants would reach only the Rust code,
    // not the markdown/doc-comment prose, so those docs would drift anyway — a false "no-drift"
    // guarantee bought with permanent public API and slightly ossified tuning knobs. So if you
    // change table_min_rows, table_min_columns, table_min_similarity, table_column_max_width,
    // kv_pack_multiple, multiline_min_lines, or multiline_max_lines, update all of those sites
    // by hand. (wrap_width is the exception: shared via DEFAULT_WRAP_WIDTH, pulled live.)
    fn default() -> Self {
        Self {
            start_indent: 0,
            force_markers: false,
            bare_strings: StringStyle::Bare,
            bare_keys: BareStyle::Prefer,
            inline_objects: true,
            inline_arrays: true,
            string_array_style: StringArrayStyle::PreferSpaces,
            tables: true,
            wrap_width: Some(DEFAULT_WRAP_WIDTH),
            table_min_rows: 3,
            table_min_columns: 3,
            table_min_similarity: 0.8,
            table_column_max_width: Some(40),
            kv_pack_multiple: 2,
            number_fold_style: FoldStyle::Auto,
            string_bare_fold_style: FoldStyle::Auto,
            string_quoted_fold_style: FoldStyle::Auto,
            string_multiline_fold_style: FoldStyle::None,
            table_fold: false,
            table_unindent_style: TableUnindentStyle::Auto,
            indent_glyph_style: IndentGlyphStyle::Auto,
            indent_glyph_marker_style: IndentGlyphMarkerStyle::Compact,
            multiline_strings: true,
            multiline_style: MultilineStyle::Bold,
            multiline_min_lines: 1,
            multiline_max_lines: 10,
            eol: Eol::Lf,
            honor_string_forms: true,
            honor_key_forms: true,
            honor_tables: true,
            render_comments: true,
        }
    }
}

// Deserializers that accept camelCase (for JS/WASM) for all enum fields in TjsonConfig.
// PascalCase (serde default) is also accepted as a fallback.
mod camel_de {
    use serde::{Deserialize, Deserializer};

    fn de_str<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Option::<String>::deserialize(d)
    }

    macro_rules! camel_option_de {
        ($fn_name:ident, $Enum:ty, $($camel:literal => $variant:expr),+ $(,)?) => {
            pub fn $fn_name<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$Enum>, D::Error> {
                let Some(s) = de_str(d)? else { return Ok(None); };
                match s.as_str() {
                    $($camel => Ok(Some($variant)),)+
                    // Exactly one accepted spelling per value. A PascalCase fallback
                    // used to be accepted here; it was undocumented tolerance (every
                    // surface advertises camelCase only) and was removed in 0.7.0.
                    _ => Err(serde::de::Error::unknown_variant(&s, &[$($camel),+])),
                }
            }
        };
    }

    camel_option_de!(bare_style, super::BareStyle,
        "prefer" => super::BareStyle::Prefer,
        "none"   => super::BareStyle::None,
    );

    // `prefer` and `none` are the names this option carried when it was a
    // BareStyle, kept as exact synonyms of the two readings that replaced them:
    // `prefer` meant "bare where the spec permits", which is `bare`, and `none`
    // meant "never bare", which is `quoted`. They are accepted here and nowhere
    // else -- the CLI flag takes the current names only. The reason is the
    // published bindings: the C API and the wasm/JS binding both build their
    // options by deserializing this struct, so refusing the old names would
    // break callers who never asked for a new spelling.
    camel_option_de!(string_style, super::StringStyle,
        "quoted" => super::StringStyle::Quoted,
        "bare"   => super::StringStyle::Bare,
        "marked" => super::StringStyle::Marked,
        "prefer" => super::StringStyle::Bare,
        "none"   => super::StringStyle::Quoted,
    );

    camel_option_de!(fold_style, super::FoldStyle,
        "auto"  => super::FoldStyle::Auto,
        "fixed" => super::FoldStyle::Fixed,
        "none"  => super::FoldStyle::None,
    );

    camel_option_de!(multiline_style, super::MultilineStyle,
        "floating"      => super::MultilineStyle::Floating,
        "bold"          => super::MultilineStyle::Bold,
        "boldFloating"  => super::MultilineStyle::BoldFloating,
        "boldLight"     => super::MultilineStyle::BoldLight,
        "transparent"   => super::MultilineStyle::Transparent,
        "light"         => super::MultilineStyle::Light,
        "foldingQuotes" => super::MultilineStyle::FoldingQuotes,
    );

    camel_option_de!(table_unindent_style, super::TableUnindentStyle,
        "left"     => super::TableUnindentStyle::Left,
        "auto"     => super::TableUnindentStyle::Auto,
        "floating" => super::TableUnindentStyle::Floating,
        "none"     => super::TableUnindentStyle::None,
    );

    camel_option_de!(indent_glyph_style, super::IndentGlyphStyle,
        "auto"  => super::IndentGlyphStyle::Auto,
        "fixed" => super::IndentGlyphStyle::Fixed,
        "none"  => super::IndentGlyphStyle::None,
    );

    camel_option_de!(indent_glyph_marker_style, super::IndentGlyphMarkerStyle,
        "compact"  => super::IndentGlyphMarkerStyle::Compact,
        "separate" => super::IndentGlyphMarkerStyle::Separate,
    );

    camel_option_de!(string_array_style, super::StringArrayStyle,
        "spaces"       => super::StringArrayStyle::Spaces,
        "preferSpaces" => super::StringArrayStyle::PreferSpaces,
        "comma"        => super::StringArrayStyle::Comma,
        "preferComma"  => super::StringArrayStyle::PreferComma,
        "none"         => super::StringArrayStyle::None,
    );

    camel_option_de!(eol, super::Eol,
        "lf"   => super::Eol::Lf,
        "crlf" => super::Eol::CrLf,
    );
}

/// The camelCase-deserializable options bag shared by every non-Rust surface:
/// the WASM/JS binding (`src/wasm.rs`), the C API (`src/ffi.rs`), the SQL UDF
/// (the `tjson-udf` crate, via the doc-hidden re-export), and fixture test
/// configs. Not part of the public Rust API — use [`RenderOptions`] directly
/// in Rust code.
///
/// Deliberately tolerant: unknown fields are ignored here (the JS binding's
/// documented options-bag behavior, pinned by test), and `derive(Deserialize)`
/// keeps the positional/seq form available to direct trait users. Surfaces
/// that want strictness (C, SQL) layer it on at their own boundaries with
/// serde_ignored and an object-shape guard — never on this type.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TjsonConfig {
    pub(crate) canonical: bool,
    pub(crate) force_markers: Option<bool>,
    pub(crate) wrap_width: Option<usize>,
    #[serde(deserialize_with = "camel_de::string_style")]
    pub(crate) bare_strings: Option<StringStyle>,
    #[serde(deserialize_with = "camel_de::bare_style")]
    pub(crate) bare_keys: Option<BareStyle>,
    pub(crate) inline_objects: Option<bool>,
    pub(crate) inline_arrays: Option<bool>,
    pub(crate) multiline_strings: Option<bool>,
    #[serde(deserialize_with = "camel_de::multiline_style")]
    pub(crate) multiline_style: Option<MultilineStyle>,
    pub(crate) multiline_min_lines: Option<usize>,
    pub(crate) multiline_max_lines: Option<usize>,
    pub(crate) tables: Option<bool>,
    pub(crate) table_fold: Option<bool>,
    #[serde(deserialize_with = "camel_de::table_unindent_style")]
    pub(crate) table_unindent_style: Option<TableUnindentStyle>,
    pub(crate) table_min_rows: Option<usize>,
    pub(crate) table_min_columns: Option<usize>,
    pub(crate) table_min_similarity: Option<f32>,
    pub(crate) table_column_max_width: Option<usize>,
    #[serde(deserialize_with = "camel_de::string_array_style")]
    pub(crate) string_array_style: Option<StringArrayStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    pub(crate) fold: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    pub(crate) number_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    pub(crate) string_bare_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    pub(crate) string_quoted_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    pub(crate) string_multiline_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::indent_glyph_style")]
    pub(crate) indent_glyph_style: Option<IndentGlyphStyle>,
    #[serde(deserialize_with = "camel_de::indent_glyph_marker_style")]
    pub(crate) indent_glyph_marker_style: Option<IndentGlyphMarkerStyle>,
    pub(crate) kv_pack_multiple: Option<usize>,
    #[serde(deserialize_with = "camel_de::eol")]
    pub(crate) eol: Option<Eol>,
}

impl From<TjsonConfig> for RenderOptions {
    fn from(c: TjsonConfig) -> Self {
        let mut opts = if c.canonical { RenderOptions::canonical() } else { RenderOptions::default() };
        if let Some(v) = c.force_markers      { opts = opts.force_markers(v); }
        if let Some(w) = c.wrap_width         { opts = opts.wrap_width(if w == 0 { None } else { Some(w) }); }
        if let Some(v) = c.bare_strings       { opts = opts.bare_strings(v); }
        if let Some(v) = c.bare_keys          { opts = opts.bare_keys(v); }
        if let Some(v) = c.inline_objects     { opts = opts.inline_objects(v); }
        if let Some(v) = c.inline_arrays      { opts = opts.inline_arrays(v); }
        if let Some(v) = c.multiline_strings  { opts = opts.multiline_strings(v); }
        if let Some(v) = c.multiline_style    { opts = opts.multiline_style(v); }
        if let Some(v) = c.multiline_min_lines { opts = opts.multiline_min_lines(v); }
        if let Some(v) = c.multiline_max_lines { opts = opts.multiline_max_lines(v); }
        if let Some(v) = c.tables             { opts = opts.tables(v); }
        if let Some(v) = c.table_fold        { opts = opts.table_fold(v); }
        if let Some(v) = c.table_unindent_style { opts = opts.table_unindent_style(v); }
        if let Some(v) = c.table_min_rows     { opts = opts.table_min_rows(v); }
        if let Some(v) = c.table_min_columns     { opts = opts.table_min_columns(v); }
        if let Some(v) = c.table_min_similarity { opts = opts.table_min_similarity(v); }
        if let Some(v) = c.table_column_max_width { opts = opts.table_column_max_width(if v == 0 { None } else { Some(v) }); }
        if let Some(v) = c.string_array_style { opts = opts.string_array_style(v); }
        if let Some(v) = c.fold               { opts = opts.fold(v); }
        if let Some(v) = c.number_fold_style  { opts = opts.number_fold_style(v); }
        if let Some(v) = c.string_bare_fold_style { opts = opts.string_bare_fold_style(v); }
        if let Some(v) = c.string_quoted_fold_style { opts = opts.string_quoted_fold_style(v); }
        if let Some(v) = c.string_multiline_fold_style { opts = opts.string_multiline_fold_style(v); }
        if let Some(v) = c.indent_glyph_style { opts = opts.indent_glyph_style(v); }
        if let Some(v) = c.indent_glyph_marker_style { opts = opts.indent_glyph_marker_style(v); }
        if let Some(v) = c.kv_pack_multiple { opts = opts.kv_pack_multiple_clamped(v); }
        if let Some(v) = c.eol                { opts = opts.eol(v); }
        opts
    }
}

/// Options that existed in a previous release and were renamed or removed,
/// paired with a migration hint. Every surface that reports on option fields
/// (the C API and SQL UDF strict parsers, the JS binding's curated check)
/// consults this table so they all give the same guidance. Retiring an
/// option means adding one entry here.
pub(crate) struct RetiredOption {
    /// The camelCase field name as it appeared in the release that had it.
    pub(crate) name: &'static str,
    /// Full migration sentence, e.g. "x has been renamed to y".
    pub(crate) hint: &'static str,
}

pub(crate) const RETIRED_OPTIONS: &[RetiredOption] = &[
    RetiredOption {
        name: "tableMinCols",
        hint: "tableMinCols has been renamed to tableMinColumns",
    },
];

/// Look up the migration hint for a retired option field name.
///
/// Not part of the public Rust API — this exists for tjson's own language
/// bindings; the SQL UDF crate consumes it from outside this crate.
#[doc(hidden)]
pub fn retired_option_hint(field: &str) -> Option<&'static str> {
    RETIRED_OPTIONS
        .iter()
        .find(|retired| retired.name == field)
        .map(|retired| retired.hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The specification's own sets are held to the rule they impose on a
    /// caller's. Nothing else checks them: `SPEC_FORMS` is a struct literal, so
    /// it never passes through a builder, and a set that was out of order would
    /// not fail loudly -- `binary_search` would simply stop finding some of its
    /// own members, and a lookalike would be accepted as ordinary text.
    #[test]
    fn spec_sets_obey_the_rules_they_impose() {
        for (name, structural, set) in LOOKALIKE_SETS {
            ParseOptions::checked(name, structural, set)
                .unwrap_or_else(|error| panic!("SPEC_{}: {error}", name.to_uppercase()));
        }
    }

    /// A structural character answers `true` to its own question while living
    /// outside the replaceable set -- the property the whole split rests on.
    #[test]
    fn structural_characters_are_never_set_members() {
        for (name, structural, set) in LOOKALIKE_SETS {
            for ch in structural {
                assert!(
                    !set.contains(ch),
                    "U+{:04X} is structural and must not be in the {name} set",
                    *ch as u32
                );
            }
        }
    }

    #[test]
    fn config_values_accept_exactly_one_case() {
        // camelCase is the one accepted spelling for option values on every JSON
        // surface (wasm, C FFI, UDF, fixture configs). The PascalCase fallback was
        // removed in 0.7.0 — these assertions pin both directions.
        let ok: TjsonConfig =
            serde_json::from_str(r#"{"multilineStyle":"boldFloating","bareStrings":"quoted"}"#)
                .expect("camelCase values parse");
        assert_eq!(ok.multiline_style, Some(MultilineStyle::BoldFloating));
        assert_eq!(ok.bare_strings, Some(StringStyle::Quoted));

        // The names this option carried as a BareStyle still deserialize, as
        // exact synonyms. The published bindings build their options through
        // this struct, so a caller who never asked for a new spelling keeps
        // working; the CLI flag is where the old names are gone.
        for (old, expected) in [("prefer", StringStyle::Bare), ("none", StringStyle::Quoted)] {
            let aliased: TjsonConfig =
                serde_json::from_str(&format!(r#"{{"bareStrings":"{old}"}}"#))
                    .unwrap_or_else(|e| panic!("`{old}` must still deserialize: {e}"));
            assert_eq!(aliased.bare_strings, Some(expected), "`{old}` reads as {expected:?}");
        }
        assert!(
            "prefer".parse::<StringStyle>().is_err(),
            "but the CLI flag takes the current names only"
        );

        for rejected in [
            r#"{"multilineStyle":"BoldFloating"}"#,
            r#"{"bareStrings":"None"}"#,
            r#"{"eol":"CrLf"}"#,
        ] {
            let err = serde_json::from_str::<TjsonConfig>(rejected)
                .expect_err("PascalCase values must be rejected");
            assert!(
                err.to_string().contains("unknown variant"),
                "error should name the bad value and list valid ones: {err}"
            );
        }
    }

    #[test]
    fn retired_option_lookup_finds_hint() {
        let hint = retired_option_hint("tableMinCols").expect("tableMinCols is retired");
        assert!(hint.contains("tableMinColumns"), "hint must name the replacement: {hint}");
    }

    #[test]
    fn current_option_names_are_not_retired() {
        // The replacement name must never trigger a hint.
        assert_eq!(retired_option_hint("tableMinColumns"), None);
        assert_eq!(retired_option_hint("wrapWidth"), None);
    }

    /// TjsonConfig itself must stay tolerant of unknown fields — the JS
    /// binding's documented options-bag behavior depends on it (strictness
    /// for C and SQL is layered on at those boundaries, never here). If this
    /// fails, someone added deny_unknown_fields to the shared type and broke
    /// the JS contract.
    #[test]
    fn config_tolerates_unknown_fields_and_still_applies_known_ones() {
        let config: TjsonConfig =
            serde_json::from_str(r#"{"notAnOptionAtAll":1,"wrapWidth":40}"#)
                .expect("unknown fields must not fail TjsonConfig itself");
        let options = RenderOptions::from(config);
        assert_eq!(options.wrap_width, Some(40), "known fields must still apply");
    }

    #[test]
    fn config_eol_maps_through_to_render_options() {
        // The shared options bag (every non-Rust surface) carries eol as a camelCase
        // string; it must map to RenderOptions.eol. Absent means the LF default.
        let default_opts = RenderOptions::from(
            serde_json::from_str::<TjsonConfig>(r#"{"wrapWidth":40}"#).unwrap(),
        );
        assert_eq!(default_opts.eol, Eol::Lf, "eol must default to Lf when absent");

        let crlf_opts = RenderOptions::from(
            serde_json::from_str::<TjsonConfig>(r#"{"eol":"crlf"}"#).unwrap(),
        );
        assert_eq!(crlf_opts.eol, Eol::CrLf, "eol:\"crlf\" must map to Eol::CrLf");

        let lf_opts = RenderOptions::from(
            serde_json::from_str::<TjsonConfig>(r#"{"eol":"lf"}"#).unwrap(),
        );
        assert_eq!(lf_opts.eol, Eol::Lf, "eol:\"lf\" must map to Eol::Lf");
    }

    #[test]
    fn eol_from_str_rejects_unknown() {
        assert_eq!("lf".parse::<Eol>(), Ok(Eol::Lf));
        assert_eq!("crlf".parse::<Eol>(), Ok(Eol::CrLf));
        assert!("cr".parse::<Eol>().is_err(), "bare CR is not a supported output eol");
    }

    /// Every name in RETIRED_OPTIONS must actually be gone from TjsonConfig —
    /// a name that is both "retired" and still accepted would hint users away
    /// from an option that works. Detecting "still accepted" needs
    /// serde_ignored, which is only compiled with the capi feature, so this
    /// consistency check runs in the full battery (cargo test --features capi).
    #[cfg(feature = "capi")]
    #[test]
    fn retired_names_are_actually_removed_from_config() {
        for retired in RETIRED_OPTIONS {
            let name = retired.name;
            let probe = format!("{{\"{name}\":1}}");
            let mut unknown: Vec<String> = Vec::new();
            let mut de = serde_json::Deserializer::from_str(&probe);
            let parsed: Result<TjsonConfig, _> =
                serde_ignored::deserialize(&mut de, |path| unknown.push(path.to_string()));
            assert!(
                parsed.is_err() || unknown.iter().any(|field| field == name),
                "{name} is in RETIRED_OPTIONS but TjsonConfig still accepts it"
            );
        }
    }
}

