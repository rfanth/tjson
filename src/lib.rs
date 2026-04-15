#[cfg(target_arch = "wasm32")]
mod wasm;

use std::error::Error as StdError;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::{Map as JsonMap, Number as JsonNumber, Value as JsonValue};
use unicode_general_category::{GeneralCategory, get_general_category};

/// The minimum accepted wrap width. Values below this are clamped by [`TjsonOptions::wrap_width`]
/// and rejected by [`TjsonOptions::wrap_width_checked`].
pub const MIN_WRAP_WIDTH: usize = 20;
/// The default wrap width used by [`TjsonOptions::default`].
pub const DEFAULT_WRAP_WIDTH: usize = 80;
const MIN_FOLD_CONTINUATION: usize = 10;

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
/// Not part of the public API — use [`IndentGlyphStyle`] and [`TjsonOptions`] instead.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
enum IndentGlyphMode {
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

fn indent_glyph_mode(options: &TjsonOptions) -> IndentGlyphMode {
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


#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ParseOptions {
    start_indent: usize,
}

/// Options controlling how TJSON is rendered. Use [`TjsonOptions::default`] for sensible
/// defaults, or [`TjsonOptions::canonical`] for a compact, diff-friendly format.
/// All fields are set via builder methods.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TjsonOptions {
    wrap_width: Option<usize>,
    start_indent: usize,
    force_markers: bool,
    bare_strings: BareStyle,
    bare_keys: BareStyle,
    inline_objects: bool,
    inline_arrays: bool,
    string_array_style: StringArrayStyle,
    number_fold_style: FoldStyle,
    string_bare_fold_style: FoldStyle,
    string_quoted_fold_style: FoldStyle,
    string_multiline_fold_style: FoldStyle,
    tables: bool,
    table_fold: bool,
    table_unindent_style: TableUnindentStyle,
    indent_glyph_style: IndentGlyphStyle,
    indent_glyph_marker_style: IndentGlyphMarkerStyle,
    table_min_rows: usize,
    table_min_columns: usize,
    table_min_similarity: f32,
    table_column_max_width: Option<usize>,
    /// Undocumented. Use at your own risk — may be discontinued at any time.
    kv_pack_multiple: usize,
    multiline_strings: bool,
    multiline_style: MultilineStyle,
    multiline_min_lines: usize,
    multiline_max_lines: usize,
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
            "transparent" => Ok(Self::Transparent),
            "light" => Ok(Self::Light),
            "folding-quotes" => Ok(Self::FoldingQuotes),
            _ => Err(format!(
                "invalid multiline style '{input}' (expected one of: bold, floating, bold-floating, transparent, light, folding-quotes)"
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

/// Controls how arrays of short strings are packed onto a single line.
///
/// - `Spaces`: always separate with spaces (e.g. `[ a  b  c`).
/// - `PreferSpaces`: use spaces when it fits, fall back to block layout.
/// - `Comma`: always separate with commas (e.g. `[ a, b, c`).
/// - `PreferComma` (default): use commas when it fits, fall back to block layout.
/// - `None`: never pack string arrays onto one line.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl TjsonOptions {
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
            ..Self::default()
        }
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
    pub fn bare_strings(mut self, bare_strings: BareStyle) -> Self {
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

    /// Controls how arrays where every element is a string are packed onto a single line.
    /// Has no effect on arrays that contain any non-string values. Default is `PreferComma`.
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

    /// When true, emit `\ ` fold continuations for wide table cells. Off by default —
    /// the spec notes that table folds are almost always a bad idea.
    pub fn table_fold(mut self, table_fold: bool) -> Self {
        self.table_fold = table_fold;
        self
    }

    /// Controls whether wide tables are repositioned toward the left margin using `/< />`
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
}

impl Default for TjsonOptions {
    fn default() -> Self {
        Self {
            start_indent: 0,
            force_markers: false,
            bare_strings: BareStyle::Prefer,
            bare_keys: BareStyle::Prefer,
            inline_objects: true,
            inline_arrays: true,
            string_array_style: StringArrayStyle::PreferComma,
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
                    $($camel => return Ok(Some($variant)),)+
                    _ => {}
                }
                // Fall back to PascalCase via serde
                serde_json::from_value(serde_json::Value::String(s.clone()))
                    .map(Some)
                    .map_err(|_| serde::de::Error::unknown_variant(&s, &[$($camel),+]))
            }
        };
    }

    camel_option_de!(bare_style, super::BareStyle,
        "prefer" => super::BareStyle::Prefer,
        "none"   => super::BareStyle::None,
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
}

/// A camelCase-deserializable options bag for WASM/JS and test configs.
/// Not part of the public Rust API — use [`TjsonOptions`] directly in Rust code.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TjsonConfig {
    canonical: bool,
    force_markers: Option<bool>,
    wrap_width: Option<usize>,
    #[serde(deserialize_with = "camel_de::bare_style")]
    bare_strings: Option<BareStyle>,
    #[serde(deserialize_with = "camel_de::bare_style")]
    bare_keys: Option<BareStyle>,
    inline_objects: Option<bool>,
    inline_arrays: Option<bool>,
    multiline_strings: Option<bool>,
    #[serde(deserialize_with = "camel_de::multiline_style")]
    multiline_style: Option<MultilineStyle>,
    multiline_min_lines: Option<usize>,
    multiline_max_lines: Option<usize>,
    tables: Option<bool>,
    table_fold: Option<bool>,
    #[serde(deserialize_with = "camel_de::table_unindent_style")]
    table_unindent_style: Option<TableUnindentStyle>,
    table_min_rows: Option<usize>,
    table_min_columns: Option<usize>,
    table_min_similarity: Option<f32>,
    table_column_max_width: Option<usize>,
    #[serde(deserialize_with = "camel_de::string_array_style")]
    string_array_style: Option<StringArrayStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    fold: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    number_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    string_bare_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    string_quoted_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::fold_style")]
    string_multiline_fold_style: Option<FoldStyle>,
    #[serde(deserialize_with = "camel_de::indent_glyph_style")]
    indent_glyph_style: Option<IndentGlyphStyle>,
    #[serde(deserialize_with = "camel_de::indent_glyph_marker_style")]
    indent_glyph_marker_style: Option<IndentGlyphMarkerStyle>,
    kv_pack_multiple: Option<usize>,
}

impl From<TjsonConfig> for TjsonOptions {
    fn from(c: TjsonConfig) -> Self {
        let mut opts = if c.canonical { TjsonOptions::canonical() } else { TjsonOptions::default() };
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
        opts
    }
}

/// A parsed TJSON value. Mirrors the JSON type system with the same six variants.
///
/// Numbers are stored as strings to preserve exact representation. Objects are stored as
/// an ordered `Vec` of key-value pairs, which allows duplicate keys at the data structure
/// level (though JSON and TJSON parsers typically deduplicate them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TjsonValue {
    /// JSON `null`.
    Null,
    /// JSON boolean.
    Bool(bool),
    /// JSON number.
    Number(serde_json::Number),
    /// JSON string.
    String(String),
    /// JSON array.
    Array(Vec<TjsonValue>),
    /// JSON object, as an ordered list of key-value pairs.
    Object(Vec<(String, TjsonValue)>),
}

impl From<JsonValue> for TjsonValue {
    fn from(value: JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(value),
            JsonValue::Number(value) => Self::Number(value),
            JsonValue::String(value) => Self::String(value),
            JsonValue::Array(values) => {
                Self::Array(values.into_iter().map(Self::from).collect())
            }
            JsonValue::Object(map) => Self::Object(
                map.into_iter()
                    .map(|(key, value)| (key, Self::from(value)))
                    .collect(),
            ),
        }
    }
}

impl TjsonValue {

    fn parse_with(input: &str, options: ParseOptions) -> Result<Self> {
        Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
    }

    /// Render this value as a TJSON string using the given options.
    ///
    /// Currently this is effectively infallible in practice — when options conflict or
    /// content cannot be laid out ideally (e.g. `wrap_width` too narrow with folding
    /// disabled), the renderer overflows rather than failing. The `Result` return type
    /// is intentional and forward-looking: a future option like `fail_on_overflow`
    /// could request strict layout enforcement and return an error rather than overflowing.
    /// Keeping `Result` here avoids a breaking API change when that option is added.
    /// At that point `Error` would likely gain a dedicated variant for layout constraint
    /// failures, distinct from the existing `Error::Render` (malformed data).
    pub fn to_tjson_with(&self, options: TjsonOptions) -> Result<String> {
        Renderer::render(self, &options)
    }

    /// Convert this value to a `serde_json::Value`. If the value contains duplicate object keys,
    /// only the last value for each key is kept (serde_json maps deduplicate on insert).
    ///
    /// ```
    /// use tjson::TjsonValue;
    ///
    /// let json: serde_json::Value = serde_json::json!({"name": "Alice"});
    /// let tjson = TjsonValue::from(json.clone());
    /// assert_eq!(tjson.to_json().unwrap(), json);
    /// ```
    pub fn to_json(&self) -> Result<JsonValue, Error> {
        Ok(match self {
            Self::Null => JsonValue::Null,
            Self::Bool(value) => JsonValue::Bool(*value),
            Self::Number(value) => JsonValue::Number(value.clone()),
            Self::String(value) => JsonValue::String(value.clone()),
            Self::Array(values) => JsonValue::Array(
                values
                    .iter()
                    .map(TjsonValue::to_json)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Object(entries) => {
                let mut map = JsonMap::new();
                for (key, value) in entries {
                    map.insert(key.clone(), value.to_json()?);
                }
                JsonValue::Object(map)
            }
        })
    }
}

impl serde::Serialize for TjsonValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self {
            Self::Null => serializer.serialize_unit(),
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Number(n) => n.serialize(serializer),
            Self::String(s) => serializer.serialize_str(s),
            Self::Array(values) => {
                let mut seq = serializer.serialize_seq(Some(values.len()))?;
                for v in values {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Self::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (k, v) in entries {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

impl fmt::Display for TjsonValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = Renderer::render(self, &TjsonOptions::default()).map_err(|_| fmt::Error)?;
        f.write_str(&s)
    }
}

/// ```
/// let v: tjson::TjsonValue = "  name: Alice".parse().unwrap();
/// assert!(matches!(v, tjson::TjsonValue::Object(_)));
/// ```
impl std::str::FromStr for TjsonValue {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse_with(s, ParseOptions::default())
    }
}

/// A parse error with source location and optional source line context.
///
/// The `Display` implementation formats the error as `line N, column M: message` and,
/// when source context is available, appends the source line and a caret pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseError {
    line: usize,
    column: usize,
    message: String,
    source_line: Option<String>,
}

impl ParseError {
    fn new(line: usize, column: usize, message: impl Into<String>, source_line: Option<String>) -> Self {
        Self {
            line,
            column,
            message: message.into(),
            source_line,
        }
    }

    /// 1-based line number where the error occurred.
    pub fn line(&self) -> usize { self.line }
    /// 1-based column number where the error occurred.
    pub fn column(&self) -> usize { self.column }
    /// Human-readable error message.
    pub fn message(&self) -> &str { &self.message }
    /// The source line text, if available, for display with a caret pointer.
    pub fn source_line(&self) -> Option<&str> { self.source_line.as_deref() }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}: {}", self.line, self.column, self.message)?;
        if let Some(src) = &self.source_line {
            write!(f, "\n  {}\n  {:>width$}", src, "^", width = self.column)?;
        }
        Ok(())
    }
}

impl StdError for ParseError {}

/// The error type for all TJSON operations.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// A parse error with source location.
    Parse(ParseError),
    /// A JSON serialization or deserialization error from serde_json.
    Json(serde_json::Error),
    /// A render error (e.g. invalid number representation).
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

/// Convenience `Result` type with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn parse_str_with_options(input: &str, options: ParseOptions) -> Result<TjsonValue> {
    Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
}

#[cfg(test)]
fn render_string(value: &TjsonValue) -> Result<String> {
    render_string_with_options(value, TjsonOptions::default())
}

fn render_string_with_options(value: &TjsonValue, options: TjsonOptions) -> Result<String> {
    Renderer::render(value, &options)
}

/// Parse a TJSON string and deserialize it into `T` using serde.
///
/// ```
/// #[derive(serde::Deserialize, PartialEq, Debug)]
/// struct Person { name: String, city: String }
///
/// let p: Person = tjson::from_str("  name: Alice  city: London").unwrap();
/// assert_eq!(p, Person { name: "Alice".into(), city: "London".into() });
/// ```
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T> {
    from_tjson_str_with_options(input, ParseOptions::default())
}

fn from_tjson_str_with_options<T: DeserializeOwned>(
    input: &str,
    options: ParseOptions,
) -> Result<T> {
    let value = parse_str_with_options(input, options)?;
    let json = value.to_json()?;
    Ok(serde_json::from_value(json)?)
}

/// Serialize `value` to a TJSON string using default options.
///
/// ```
/// #[derive(serde::Serialize)]
/// struct Person { name: &'static str }
///
/// let s = tjson::to_string(&Person { name: "Alice" }).unwrap();
/// assert_eq!(s, "  name: Alice");
/// ```
pub fn to_string<T: Serialize>(value: &T) -> Result<String> {
    to_string_with(value, TjsonOptions::default())
}

/// Serialize `value` to a TJSON string using the given options.
///
/// ```
/// let s = tjson::to_string_with(&vec![1, 2, 3], tjson::TjsonOptions::default()).unwrap();
/// assert_eq!(s, "  1, 2, 3");
/// ```
pub fn to_string_with<T: Serialize>(
    value: &T,
    options: TjsonOptions,
) -> Result<String> {
    let json = serde_json::to_value(value)?;
    let value = TjsonValue::from(json);
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
        let expanded = expand_indent_adjustments(&normalized);
        let mut parser = Self {
            lines: expanded.split('\n').map(str::to_owned).collect(),
            line: 0,
            start_indent,
        };
        parser.skip_ignorable_lines()?;
        if parser.line >= parser.lines.len() {
            return Err(ParseError::new(1, 1, "empty input", None));
        }
        let value = parser.parse_root_value()?;
        parser.skip_ignorable_lines()?;
        if parser.line < parser.lines.len() {
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

    fn parse_root_value(&mut self) -> std::result::Result<TjsonValue, ParseError> {
        let line = self
            .current_line()
            .ok_or_else(|| ParseError::new(1, 1, "empty input", None))?
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
        let columns = self.parse_table_header(header, elem_indent)?;
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
            // Collect fold continuation lines: `/ ` marker at pair_indent (elem_indent - 2),
            // two characters to the left of the opening `|` per spec.
            // Blank lines and `//` comments between a partial row and its continuation are
            // skipped. A parser would also be within its rights to reject them.
            let pair_indent = elem_indent.saturating_sub(2);
            let mut row_owned = row.to_owned();
            loop {
                // Peek past ignorable lines to find the next meaningful line.
                let mut offset = 1usize;
                loop {
                    let Some(peek) = self.lines.get(self.line + offset) else { break; };
                    let trimmed = peek.trim_start_matches(' ');
                    if trimmed.starts_with("//") {
                        offset += 1;
                    } else {
                        break;
                    }
                }
                let Some(next_line) = self.lines.get(self.line + offset) else {
                    break;
                };
                let next_indent = count_leading_spaces(next_line);
                if next_indent != pair_indent {
                    break;
                }
                let next_content = &next_line[pair_indent..];
                if !next_content.starts_with("/ ") {
                    break;
                }
                // Consume ignorable lines then the continuation line.
                for i in 1..offset {
                    self.ensure_line_has_no_tabs(self.line + i)?;
                }
                self.line += offset;
                self.ensure_line_has_no_tabs(self.line)?;
                row_owned.push_str(&next_content[2..]);
            }
            rows.push(self.parse_table_row(&columns, &row_owned, elem_indent)?);
            self.line += 1;
        }
        if rows.is_empty() {
            return Err(self.error_current("table arrays must contain at least one row"));
        }
        Ok(TjsonValue::Array(rows))
    }

    fn parse_table_header(&self, row: &str, indent: usize) -> std::result::Result<Vec<String>, ParseError> {
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

    fn parse_table_header_key(&self, cell: &str, col: usize) -> std::result::Result<String, ParseError> {
        if let Some(end) = parse_bare_key_prefix(cell)
            && end == cell.len() {
                return Ok(cell.to_owned());
            }
        if let Some((value, end)) = parse_json_string_prefix(cell)
            && end == cell.len() {
                return Ok(value);
            }
        Err(self.error_at_line(self.line, col, "invalid table header key"))
    }

    fn parse_table_row(
        &self,
        columns: &[String],
        row: &str,
        indent: usize,
    ) -> std::result::Result<TjsonValue, ParseError> {
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
        for (index, key) in columns.iter().enumerate() {
            let cell = cells[index].trim_end();
            if cell.is_empty() {
                continue;
            }
            entries.push((key.clone(), self.parse_table_cell_value(cell)?));
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
        if let Some(value) = cell.strip_prefix(' ') {
            if !is_allowed_bare_string(value) {
                return Err(self.error_at_line(self.line, 1, "invalid bare string in table cell"));
            }
            return Ok(TjsonValue::String(value.to_owned()));
        }
        if let Some((value, end)) = parse_json_string_prefix(cell)
            && end == cell.len() {
                return Ok(TjsonValue::String(value));
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
        if let Ok(n) = JsonNumber::from_str(cell) {
            return Ok(TjsonValue::Number(n));
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
                let content = line[indent..].to_owned();
                let msg = if content.starts_with("/>") {
                    format!("misplaced /> indent offset glyph: found at column {}, expected at column {}", indent + 1, pair_indent + 1)
                } else if content.starts_with("/ ") {
                    format!("misplaced fold marker: found at column {}, expected at column {}", indent + 1, pair_indent + 1)
                } else {
                    "expected an object entry at this indent".to_owned()
                };
                return Err(self.error_current(msg));
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
        let mut rest = content.to_owned();
        let mut entries = Vec::new();
        loop {
            let (key, after_colon) = self.parse_key(&rest, pair_indent)?;
            rest = after_colon;

            if rest.is_empty() {
                self.line += 1;
                let value = self.parse_value_after_key(pair_indent)?;
                entries.push((key, value));
                return Ok(entries);
            }

            let (value, consumed) =
                self.parse_inline_value(&rest, pair_indent, ArrayLineValueContext::ObjectValue)?;
            entries.push((key, value));

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
        // Fold after colon: value starts on a "/ " continuation line at pair_indent.
        // Spec: key and basic value are folded as a single unit; fold marker is allowed
        // immediately after the ":" (preferred), treating the junction at pair_indent+2 indent.
        if indent == pair_indent && content.starts_with("/ ") {
            let continuation_content = &content[2..];
            let (value, consumed) = self.parse_inline_value(
                continuation_content, pair_indent, ArrayLineValueContext::ObjectValue,
            )?;
            if consumed.is_some() {
                self.line += 1;
            }
            return Ok(value);
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
            // Bare strings have a leading space, so they sit at elem_indent+1.
            if indent == elem_indent + 1 && line.as_bytes().get(elem_indent) == Some(&b' ') {
                let content = &line[elem_indent..];
                self.parse_array_line_content(content, elem_indent, elements)?;
                continue;
            }
            if indent != elem_indent {
                return Err(self.error_current("invalid indent level: array elements must be indented by exactly two spaces"));
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
        let deepest_parent_indent = line_indent + 2 * markers.len().saturating_sub(1);

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
                    value = TjsonValue::Array(wrapped);
                }
                return Ok(value);
            }
        }

        let mut value = match *markers.last().unwrap() {
            ContainerKind::Array => {
                let mut elements = Vec::new();
                if is_minimal_json_candidate(rest) {
                    elements.push(self.parse_minimal_json_line(rest)?);
                    self.line += 1;
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                } else {
                    self.parse_array_line_content(rest, deepest_parent_indent + 2, &mut elements)?;
                    self.parse_array_tail(deepest_parent_indent, &mut elements)?;
                }
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

    /// Parse an object key, returning `(key_string, rest_after_colon)`.
    /// Handles fold continuations (`/ `) for both bare keys and JSON string keys.
    fn parse_key(
        &mut self,
        content: &str,
        fold_indent: usize,
    ) -> std::result::Result<(String, String), ParseError> {
        // Bare key on this line
        if let Some(end) = parse_bare_key_prefix(content) {
            if content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((content[..end].to_owned(), content[end + ':'.len_utf8()..].to_owned()));
            }
            // Bare key fills the whole line — look for fold continuations
            if end == content.len() {
                let mut key_acc = content[..end].to_owned();
                let mut next = self.line + 1;
                loop {
                    let Some(fold_line) = self.lines.get(next).cloned() else {
                        break;
                    };
                    let fi = count_leading_spaces(&fold_line);
                    if fi != fold_indent {
                        break;
                    }
                    let rest = &fold_line[fi..];
                    if !rest.starts_with("/ ") {
                        break;
                    }
                    let cont = &rest[2..];
                    next += 1;
                    if let Some(colon_pos) = cont.find(':') {
                        key_acc.push_str(&cont[..colon_pos]);
                        self.line = next - 1; // point to last fold line; caller will +1
                        return Ok((key_acc, cont[colon_pos + ':'.len_utf8()..].to_owned()));
                    }
                    key_acc.push_str(cont);
                }
            }
        }
        // JSON string key on this line
        if let Some((value, end)) = parse_json_string_prefix(content)
            && content.get(end..).is_some_and(|rest| rest.starts_with(':')) {
                return Ok((value, content[end + ':'.len_utf8()..].to_owned()));
            }
        // JSON string key that doesn't close on this line — look for fold continuations
        if content.starts_with('"') && parse_json_string_prefix(content).is_none() {
            let mut json_acc = content.to_owned();
            let mut next = self.line + 1;
            loop {
                let Some(fold_line) = self.lines.get(next).cloned() else {
                    break;
                };
                let fi = count_leading_spaces(&fold_line);
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
                        return Ok((value, json_acc[end + ':'.len_utf8()..].to_owned()));
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
                if content.starts_with(" `") {
                    let value = self.parse_multiline_string(content, line_indent)?;
                    return Ok((TjsonValue::String(value), None));
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
                    loop {
                        let Some(fold_line) = self.lines.get(next) else {
                            break;
                        };
                        let fi = count_leading_spaces(fold_line);
                        if fi != line_indent {
                            break;
                        }
                        let rest = &fold_line[fi..];
                        if !rest.starts_with("/ ") {
                            break;
                        }
                        acc.push_str(&rest[2..]);
                        next += 1;
                        fold_count += 1;
                    }
                    if fold_count > 0 {
                        self.line = next;
                        return Ok((TjsonValue::String(acc), None));
                    }
                }
                Ok((TjsonValue::String(value.to_owned()), Some(end)))
            }
            '"' => {
                if let Some((value, end)) = parse_json_string_prefix(content) {
                    return Ok((TjsonValue::String(value), Some(end)));
                }
                let value = self.parse_folded_json_string(content, line_indent)?;
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
                // Check for fold continuations when the number fills the rest of the line
                if end == content.len() {
                    let mut acc = token.to_owned();
                    let mut next = self.line + 1;
                    let mut fold_count = 0usize;
                    loop {
                        let Some(fold_line) = self.lines.get(next) else { break; };
                        let fi = count_leading_spaces(fold_line);
                        if fi != line_indent { break; }
                        let rest = &fold_line[fi..];
                        if !rest.starts_with("/ ") { break; }
                        acc.push_str(&rest[2..]);
                        next += 1;
                        fold_count += 1;
                    }
                    if fold_count > 0 {
                        let n = JsonNumber::from_str(&acc)
                            .map_err(|_| self.error_current(format!("invalid JSON number after folding: \"{acc}\"")))?;
                        self.line = next;
                        return Ok((TjsonValue::Number(n), None));
                    }
                }
                let n = JsonNumber::from_str(token)
                    .map_err(|_| self.error_current(format!("invalid JSON number: \"{token}\"")))?;
                Ok((TjsonValue::Number(n), Some(end)))
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

        match glyph {
            "```" => self.parse_triple_backtick_body(local_eol, &closer, opener_line),
            "``" => self.parse_double_backtick_body(local_eol, &closer, opener_line),
            "`" => self.parse_single_backtick_body(line_indent, local_eol, &closer, opener_line),
            _ => unreachable!(),
        }
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
            let fi = count_leading_spaces(&line);
            if fi != fold_indent {
                return Err(self.error_at_line(start_line, fold_indent + 1, "unterminated JSON string"));
            }
            let rest = &line[fi..];
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
    ) -> std::result::Result<TjsonValue, ParseError> {
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
        Ok(TjsonValue::from(value))
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
        self.lines.get(self.line + 1).is_some_and(|l| {
            let fi = count_leading_spaces(l);
            fi == expected_indent && l[fi..].starts_with("/ ")
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
        ParseError::new(line_index + 1, column, message, self.lines.get(line_index).map(|l| l.to_owned()))
    }
}

enum PackedToken {
    /// A flat inline token string (number, null, bool, short string, empty array/object).
    /// Also carries the original value for lone-overflow fold fallback.
    Inline(String, TjsonValue),
    /// A block element (multiline string, nonempty array, nonempty object) that interrupts
    /// packing. Carries the original value; rendered lazily at the right continuation indent.
    Block(TjsonValue),
}

struct Renderer;

impl Renderer {
    fn render(value: &TjsonValue, options: &TjsonOptions) -> Result<String> {
        let lines = Self::render_root(value, options, options.start_indent)?;
        Ok(lines.join("\n"))
    }

    fn render_root(
        value: &TjsonValue,
        options: &TjsonOptions,
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
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        let pair_indent = parent_indent + 2;
        let mut lines = Vec::new();
        let mut packed_line = String::new();

        for (key, value) in entries {
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
        value: &TjsonValue,
        pair_indent: usize,
        options: &TjsonOptions,
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
        value: &TjsonValue,
        pair_indent: usize,
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::String(s) => Self::render_string_lines(s, pair_indent, 2, options),
            TjsonValue::Number(n) => {
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
        value: &TjsonValue,
        pair_indent: usize,
        key_fold_enabled: bool,
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Array(values) if !values.is_empty() => {
                if effective_tables(options)
                    && let Some(table_lines) = Self::render_table(values, pair_indent, options)? {
                        if let Some(target_indent) = table_unindent_target(pair_indent, &table_lines, options) {
                            let Some(offset_lines) = Self::render_table(values, target_indent, options)? else {
                                return Err(crate::Error::Render(
                                    "table eligible at natural indent failed to re-render at offset indent".into(),
                                ));
                            };
                            let key_line = format!("{}{}", spaces(pair_indent), key_text);
                            let mut lines = indent_glyph_open_lines(&key_line, pair_indent, options);
                            if effective_force_markers(options) {
                                let elem_indent = target_indent + 2;
                                let first = offset_lines.first().ok_or_else(|| Error::Render("empty table".to_owned()))?;
                                let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".to_owned()))?;
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
                            let first = table_lines.first().ok_or_else(|| Error::Render("empty table".to_owned()))?;
                            let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".to_owned()))?;
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
                        TjsonValue::Array(a) => a.is_empty(),
                        TjsonValue::Object(o) => o.is_empty(),
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
            TjsonValue::Object(entries) if !entries.is_empty() => {
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
                let scalar_lines = if let TjsonValue::String(s) = value {
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
        values: &[TjsonValue],
        parent_indent: usize,
        options: &TjsonOptions,
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
        options: &TjsonOptions,
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
        values: &[TjsonValue],
        marker_indent: usize,
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        if effective_tables(options)
            && let Some(lines) = Self::render_table(values, marker_indent, options)? {
                // Always prepend "[ " — render_explicit_array always needs its marker,
                // whether the elements render as a table or in any other form.
                let elem_indent = marker_indent + 2;
                let first = lines.first().ok_or_else(|| Error::Render("empty table".to_owned()))?;
                let stripped = first.get(elem_indent..).ok_or_else(|| Error::Render("failed to align table marker".to_owned()))?;
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
        options: &TjsonOptions,
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
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Array(values) if !values.is_empty() => {
                if should_use_indent_glyph(value, elem_indent, options) {
                    let mut lines = vec![format!("{}[ /<", spaces(elem_indent))];
                    if values.first().is_some_and(needs_explicit_array_marker) {
                        lines.extend(Self::render_explicit_array(values, 2, options)?);
                    } else {
                        lines.extend(Self::render_array_children(values, 2, options)?);
                    }
                    lines.push(format!("{} />", spaces(elem_indent)));
                    return Ok(lines);
                }
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
        options: &TjsonOptions,
    ) -> Result<Vec<String>> {
        match value {
            TjsonValue::Null => Ok(vec![format!("{}null", spaces(indent))]),
            TjsonValue::Bool(value) => Ok(vec![format!(
                "{}{}",
                spaces(indent),
                if *value { "true" } else { "false" }
            )]),
            TjsonValue::Number(value) => {
                let s = value.to_string();
                if let Some(lines) = fold_number(&s, indent, 0, options.number_fold_style, options.wrap_width) {
                    return Ok(lines);
                }
                Ok(vec![format!("{}{}", spaces(indent), s)])
            }
            TjsonValue::String(value) => Self::render_string_lines(value, indent, 0, options),
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
        first_line_extra: usize,
        options: &TjsonOptions,
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
        value: &TjsonValue,
        options: &TjsonOptions,
    ) -> Result<Option<String>> {
        let Some(value_text) = Self::render_scalar_token(value, options)? else {
            return Ok(None);
        };
        Ok(Some(format!("{}:{}", render_key(key, options), value_text)))
    }

    fn render_scalar_token(value: &TjsonValue, options: &TjsonOptions) -> Result<Option<String>> {
        let rendered = match value {
            TjsonValue::Null => "null".to_owned(),
            TjsonValue::Bool(value) => {
                if *value {
                    "true".to_owned()
                } else {
                    "false".to_owned()
                }
            }
            TjsonValue::Number(value) => value.to_string(),
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

        Ok(Some(rendered))
    }

    fn render_packed_array_lines(
        values: &[TjsonValue],
        first_prefix: String,
        continuation_indent: usize,
        options: &TjsonOptions,
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

        let tokens = Self::render_packed_array_tokens(values, options)?;
        Self::render_packed_token_lines(tokens, first_prefix, continuation_indent, false, options)
    }

    fn render_string_array_lines(
        values: &[TjsonValue],
        first_prefix: String,
        continuation_indent: usize,
        options: &TjsonOptions,
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
        values: &[TjsonValue],
        options: &TjsonOptions,
    ) -> Result<Vec<PackedToken>> {
        let mut tokens = Vec::new();
        for value in values {
            let token = match value {
                // Multiline strings are block elements — cannot be packed inline.
                TjsonValue::String(text) if text.contains('\n') || text.contains('\r') => {
                    PackedToken::Block(value.clone())
                }
                // Nonempty arrays and objects are block elements.
                TjsonValue::Array(vals) if !vals.is_empty() => PackedToken::Block(value.clone()),
                TjsonValue::Object(entries) if !entries.is_empty() => {
                    PackedToken::Block(value.clone())
                }
                // Inline string: force JSON quoting for comma-like chars to avoid parse ambiguity.
                TjsonValue::String(text) => {
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
        value: &TjsonValue,
        continuation_indent: usize,
        first_line_extra: usize,
        options: &TjsonOptions,
    ) -> Result<Option<Vec<String>>> {
        match value {
            TjsonValue::String(s) => {
                let lines =
                    Self::render_string_lines(s, continuation_indent, first_line_extra, options)?;
                Ok(if lines.len() > 1 { Some(lines) } else { None })
            }
            TjsonValue::Number(n) => {
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
        options: &TjsonOptions,
    ) -> Result<Option<Vec<String>>> {
        if tokens.is_empty() {
            return Ok(Some(vec![first_prefix]));
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
                        TjsonValue::String(s) => {
                            Self::render_string_lines(s, continuation_indent, 0, options)?
                        }
                        TjsonValue::Array(vals) if !vals.is_empty() => {
                            Self::render_explicit_array(vals, continuation_indent, options)?
                        }
                        TjsonValue::Object(entries) if !entries.is_empty() => {
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
        values: &[TjsonValue],
        parent_indent: usize,
        options: &TjsonOptions,
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
            // Check that shared keys appear in the same relative order as in the first row.
            let row_keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
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
        // Bail out if any column's content exceeds table_column_max_width.
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
            let min_row_width = 2 + widths.iter().sum::<usize>() + widths.len() + 1;
            if min_row_width > w {
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
                // Check if any cell exceeds table_column_max_width and fold if so.
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
                        lines.push(format!("{}\\ {}", fold_prefix, after));
                        continue;
                    }
                }
            }

            lines.push(row_line);
        }

        Ok(Some(lines))
    }

    fn render_table_cell_token(
        value: &TjsonValue,
        options: &TjsonOptions,
    ) -> Result<Option<String>> {
        Ok(match value {
            TjsonValue::Null => Some("null".to_owned()),
            TjsonValue::Bool(value) => Some(if *value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }),
            TjsonValue::Number(value) => Some(value.to_string()),
            TjsonValue::String(value) => {
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
                None,
            ));
        }
        if is_forbidden_literal_tjson_char(ch) {
            return Err(ParseError::new(
                line,
                column,
                format!("forbidden character U+{:04X} must be escaped", ch as u32),
                None,
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

// Expands /< /> indent-adjustment glyphs before parsing.
//
// /< appears as the value in "key: /<" and resets the visible indent to n=0,
// meaning subsequent lines are rendered as if at the document root (visual
// indent 0).  The actual nesting depth is unchanged.
//
// /> must be alone on the line (with optional leading/trailing spaces) and
// restores the previous indent context.
//
// Preprocessing converts shifted lines back to their real indent so the main
// parser never sees /< or />.
fn expand_indent_adjustments(input: &str) -> String {
    if !input.contains(" /<") {
        return input.to_owned();
    }

    let mut output_lines: Vec<String> = Vec::with_capacity(input.lines().count() + 4);
    // Stack entries: (offset, expected_close_file_indent).
    // offset_stack.last() is the current offset; effective = file_indent + offset.
    // The base entry uses usize::MAX as a sentinel (no /< to close at the root level).
    let mut offset_stack: Vec<(usize, usize)> = vec![(0, usize::MAX)];
    // When a line ends with ':' and no value, it may be the first half of an own-line
    // /< open. Hold it here; flush it as a regular line if the next line is not " /<".
    let mut pending_key_line: Option<String> = None;

    for raw_line in input.split('\n') {
        let (current_offset, expected_close) = *offset_stack.last().unwrap();

        // /> – restoration glyph: must be exactly spaces(expected_close_file_indent) + " />".
        // Any other indentation is not a close glyph and falls through as a regular line.
        if offset_stack.len() > 1
            && raw_line.len() == expected_close + 3
            && raw_line[..expected_close].bytes().all(|b| b == b' ')
            && &raw_line[expected_close..] == " />"
        {
            if let Some(held) = pending_key_line.take() { output_lines.push(held); }
            offset_stack.pop();
            continue; // consume the line without emitting it
        }

        // Own-line /< – a line whose trimmed content is exactly " /<" following a pending key.
        // The /< must be at pair_indent (= pending key's file_indent) spaces + " /<".
        let trimmed = raw_line.trim_end();
        if let Some(ref held) = pending_key_line {
            let key_file_indent = count_leading_spaces(held);
            if trimmed.len() == key_file_indent + 3
                && trimmed[..key_file_indent].bytes().all(|b| b == b' ')
                && &trimmed[key_file_indent..] == " /<"
            {
                // Treat as if the held key line had " /<" appended.
                let eff_indent = key_file_indent + current_offset;
                let content = &held[key_file_indent..]; // "key:"
                output_lines.push(format!("{}{}", spaces(eff_indent), content));
                offset_stack.push((eff_indent, key_file_indent));
                pending_key_line = None;
                continue;
            }
            // Not a /< — flush the held key line as a regular line.
            output_lines.push(pending_key_line.take().unwrap());
        }

        // /< – adjustment glyph: the trimmed line ends with " /<" and what
        // precedes it ends with ':' (confirming this is a key-value context,
        // not a multiline-string body or other content).
        let trimmed_end = trimmed;
        if let Some(without_glyph) = trimmed_end.strip_suffix(" /<")
            && without_glyph.trim_end().ends_with(':') {
                let file_indent = count_leading_spaces(raw_line);
                let eff_indent = file_indent + current_offset;
                let content = &without_glyph[file_indent..];
                output_lines.push(format!("{}{}", spaces(eff_indent), content));
                offset_stack.push((eff_indent, file_indent));
                continue;
        }

        // Key-only line (ends with ':' after trimming, no value after the colon):
        // may be the first half of an own-line /<. Hold it for one iteration.
        if trimmed_end.ends_with(':') && !trimmed_end.trim_start().contains(' ') {
            // Preserve any active offset re-indentation in the held form.
            let held = if current_offset == 0 || raw_line.trim().is_empty() {
                raw_line.to_owned()
            } else {
                let file_indent = count_leading_spaces(raw_line);
                let eff_indent = file_indent + current_offset;
                let content = &raw_line[file_indent..];
                format!("{}{}", spaces(eff_indent), content)
            };
            pending_key_line = Some(held);
            continue;
        }

        // Regular line: re-indent if there is an active offset.
        if current_offset == 0 || raw_line.trim().is_empty() {
            output_lines.push(raw_line.to_owned());
        } else {
            let file_indent = count_leading_spaces(raw_line);
            let eff_indent = file_indent + current_offset;
            let content = &raw_line[file_indent..];
            output_lines.push(format!("{}{}", spaces(eff_indent), content));
        }
    }
    // Flush any trailing pending key line.
    if let Some(held) = pending_key_line.take() { output_lines.push(held); }

    // split('\n') produces a trailing "" for inputs that end with '\n'.
    // Joining that back with '\n' naturally reproduces the trailing newline,
    // so no explicit suffix is needed.
    output_lines.join("\n")
}

fn count_leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn spaces(count: usize) -> String {
    " ".repeat(count)
}

fn effective_inline_objects(options: &TjsonOptions) -> bool {
    options.inline_objects
}

fn effective_inline_arrays(options: &TjsonOptions) -> bool {
    options.inline_arrays
}

fn effective_force_markers(options: &TjsonOptions) -> bool {
    options.force_markers
}

fn effective_tables(options: &TjsonOptions) -> bool {
    options.tables
}

// Returns the target parent_indent to re-render the table at when /< /> glyphs should be
// used, or None if no unindenting should occur.
//
// `natural_lines` are the table lines as rendered at pair_indent (spaces(pair_indent+2) prefix).
fn table_unindent_target(pair_indent: usize, natural_lines: &[String], options: &TjsonOptions) -> Option<usize> {
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
fn subtree_line_count(value: &TjsonValue) -> usize {
    match value {
        TjsonValue::Array(v) if !v.is_empty() => v.iter().map(subtree_line_count).sum::<usize>() + 1,
        TjsonValue::Object(e) if !e.is_empty() => {
            e.iter().map(|(_, v)| subtree_line_count(v) + 1).sum()
        }
        _ => 1,
    }
}

/// Rough count of content bytes in a subtree. Used to weight volume in `ByteWeighted` mode.
fn subtree_byte_count(value: &TjsonValue) -> usize {
    match value {
        TjsonValue::String(s) => s.len(),
        TjsonValue::Number(n) => n.to_string().len(),
        TjsonValue::Bool(b) => if *b { 4 } else { 5 },
        TjsonValue::Null => 4,
        TjsonValue::Array(v) => v.iter().map(subtree_byte_count).sum(),
        TjsonValue::Object(e) => e.iter().map(|(k, v)| k.len() + subtree_byte_count(v)).sum(),
    }
}

/// Maximum nesting depth of non-empty containers below this value.
/// Empty arrays/objects count as 0 (simple values).
fn subtree_max_depth(value: &TjsonValue) -> usize {
    match value {
        TjsonValue::Array(v) if !v.is_empty() => {
            1 + v.iter().map(subtree_max_depth).max().unwrap_or(0)
        }
        TjsonValue::Object(e) if !e.is_empty() => {
            1 + e.iter().map(|(_, v)| subtree_max_depth(v)).max().unwrap_or(0)
        }
        _ => 0,
    }
}

/// Returns true if a `/<` indent-offset glyph should be emitted for `value` at `pair_indent`.
fn should_use_indent_glyph(value: &TjsonValue, pair_indent: usize, options: &TjsonOptions) -> bool {
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
fn indent_glyph_open_lines(key_line: &str, pair_indent: usize, options: &TjsonOptions) -> Vec<String> {
    match options.indent_glyph_marker_style {
        IndentGlyphMarkerStyle::Compact => vec![format!("{}: /<", key_line)],
        IndentGlyphMarkerStyle::Separate /*| IndentGlyphMarkerStyle::Marked*/ => vec![
            format!("{}:", key_line),
            format!("{} /<", spaces(pair_indent)),
        ],
    }
}

fn fits_wrap(options: &TjsonOptions, line: &str) -> bool {
    match options.wrap_width {
        Some(0) | None => true,
        Some(width) => line.chars().count() <= width,
    }
}

fn pick_preferred_string_array_layout(
    preferred: Option<Vec<String>>,
    fallback: Option<Vec<String>>,
    options: &TjsonOptions,
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

fn string_array_layout_score(lines: &[String], options: &TjsonOptions) -> (usize, usize, usize) {
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

fn is_valid_minimal_json(content: &str) -> Result<(), usize> {
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

fn render_key(key: &str, options: &TjsonOptions) -> String {
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
        //|| first == '|'
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

/// Returns true if the line starts with zero or more whitespace chars then the given char.
fn line_starts_with_ws_then(line: &str, ch: char) -> bool {
    let trimmed = line.trim_start_matches(|c: char| c.is_whitespace());
    trimmed.starts_with(ch)
}

/// Split a multiline-string body part into segments for fold continuations.
/// Returns the original text as a single segment if no fold is needed.
/// Segments: first is the line body, rest are fold continuations (without the `/ ` prefix).
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
fn safe_json_split(s: &str, split_at: usize) -> usize {
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
fn find_number_fold_point(s: &str, avail: usize, auto_mode: bool) -> usize {
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Letter,
    Digit,
    /// Punctuation that prefers to trail at the end of a line: `.` `,` `/` `-` `_` `~` `@` `:`.
    StickyEnd,
    Other,
}

fn char_class(ch: char) -> CharClass {
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
fn find_bare_fold_point(s: &str, lookahead: Option<char>) -> usize {
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
fn count_preceding_backslashes(bytes: &[u8], pos: usize) -> usize {
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
fn find_json_fold_point(s: &str) -> usize {
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
fn render_folding_quotes(value: &str, indent: usize, options: &TjsonOptions) -> Vec<String> {
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
fn split_table_row_for_fold(row: &str, max_len: usize) -> Option<(String, String)> {
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

fn is_comma_like(ch: char) -> bool {
    matches!(ch, ',' | '\u{FF0C}' | '\u{FE50}')
}

fn is_quote_like(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::InitialPunctuation | GeneralCategory::FinalPunctuation
    ) || matches!(ch, '"' | '\'' | '`')
}

/// matches a literal '|' pipe or a PIPELIKE CHARACTER
/// PIPELIKE CHARACTER in spec:  PIPELIKE CHARACTER DEFINITION A pipelike character is U+007C (VERTICAL LINE) or any character in the following set: U+00A6, U+01C0, U+2016, U+2223, U+2225, U+254E, U+2502, U+2503, U+2551, U+FF5C, U+FFE4
fn is_pipe_like(ch: char) -> bool {
    matches!(
        ch, '|' | '\u{00a6}' | '\u{01c0}' | '\u{2016}' | '\u{2223}' | '\u{2225}' | '\u{254e}' | '\u{2502}' | '\u{2503}' | '\u{2551}' | '\u{ff5c}' | '\u{ffe4}'
    )
}
fn is_reserved_word(s: &str) -> bool {
    matches!(s, "true" | "false" | "null" | "[]" | "{}" | "\"\"") // "" is logically reserved but unreachable: '"' is quote-like and forbidden as a bare string first/last char
}
#[cfg(test)]
mod tests {
    use super::*;

    fn json(input: &str) -> JsonValue {
        serde_json::from_str(input).unwrap()
    }

    fn tjson_value(input: &str) -> TjsonValue {
        TjsonValue::from(json(input))
    }

    fn parse_str(input: &str) -> Result<TjsonValue> {
        input.parse()
    }

    #[test]
    fn parses_basic_scalar_examples() {
        assert_eq!(
            parse_str("null").unwrap().to_json().unwrap(),
            json("null")
        );
        assert_eq!(
            parse_str("5").unwrap().to_json().unwrap(),
            json("5")
        );
        assert_eq!(
            parse_str(" a").unwrap().to_json().unwrap(),
            json("\"a\"")
        );
        assert_eq!(
            parse_str("[]").unwrap().to_json().unwrap(),
            json("[]")
        );
        assert_eq!(
            parse_str("{}").unwrap().to_json().unwrap(),
            json("{}")
        );
    }

    #[test]
    fn parses_comments_and_marker_examples() {
        let input = "// comment\n  a:5\n// comment\n  x:\n    [ [ 1\n      { b: text";
        let expected = json("{\"a\":5,\"x\":[[1],{\"b\":\"text\"}]}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    // ---- Folding tests ----

    // JSON string folding

    #[test]
    fn parses_folded_json_string_example() {
        let input =
            "\"foldingat\n/ onlyafew\\r\\n\n/ characters\n/ hereusing\n/ somejson\n/ escapes\\\\\"";
        let expected = json("\"foldingatonlyafew\\r\\ncharactershereusingsomejsonescapes\\\\\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_json_string_as_object_value() {
        // JSON string fold inside an object value
        let input = "  note:\"hello \n  / world\"";
        let expected = json("{\"note\":\"hello world\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_json_string_multiple_continuations() {
        // Three fold lines
        let input = "\"one\n/ two\n/ three\n/ four\"";
        let expected = json("\"onetwothreefour\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_json_string_with_indent() {
        // Fold continuation with leading spaces (trimmed before `/ `)
        let input = "  key:\"hello \n  / world\"";
        let expected = json("{\"key\":\"hello world\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    // Bare string folding

    #[test]
    fn parses_folded_bare_string_root() {
        // Root bare string folded across two lines
        let input = " hello\n/ world";
        let expected = json("\"helloworld\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_bare_string_as_object_value() {
        // Bare string value folded
        let input = "  note: hello\n  / world";
        let expected = json("{\"note\":\"helloworld\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_bare_string_multiple_continuations() {
        let input = "  note: one\n  / two\n  / three";
        let expected = json("{\"note\":\"onetwothree\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_bare_string_preserves_space_after_fold_marker() {
        // Content after `/ ` starts with a space — that space becomes part of string
        let input = "  note: hello\n  /  world";
        let expected = json("{\"note\":\"hello world\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    // Key folding

    #[test]
    fn parses_folded_bare_key() {
        // A long bare key folded across two lines
        let input = "  averylongkey\n  / continuation: value";
        let expected = json("{\"averylongkeycontinuation\":\"value\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_folded_json_key() {
        // A long quoted key folded across two lines
        let input = "  \"averylongkey\n  / continuation\": value";
        let expected = json("{\"averylongkeycontinuation\":\"value\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    // Table cell folding

    #[test]
    fn parses_table_with_folded_cell() {
        // A table row where one cell is folded onto the next line using backslash continuation
        let input = concat!(
            "  |name     |score |\n",
            "  | Alice   |100   |\n",
            "  | Bob with a very long\n",
            "/ name    |200   |\n",
            "  | Carol   |300   |",
        );
        let expected = json(
            "[{\"name\":\"Alice\",\"score\":100},{\"name\":\"Bob with a very longname\",\"score\":200},{\"name\":\"Carol\",\"score\":300}]"
        );
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_table_with_folded_cell_no_trailing_pipe() {
        // Table fold where the continuation line lacks a trailing pipe
        let input = concat!(
            "  |name     |value |\n",
            "  | short   |1     |\n",
            "  | this is really long\n",
            "/ continuation|2     |",
        );
        let expected = json(
            "[{\"name\":\"short\",\"value\":1},{\"name\":\"this is really longcontinuation\",\"value\":2}]"
        );
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_triple_backtick_multiline_string() {
        // ``` type: content at col 0, mandatory closing glyph
        let input = "  note: ```\nfirst\nsecond\n  indented\n   ```";
        let expected = json("{\"note\":\"first\\nsecond\\n  indented\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_triple_backtick_crlf_multiline_string() {
        // ``` type with \r\n local EOL indicator
        let input = "  note: ```\\r\\n\nfirst\nsecond\n  indented\n   ```\\r\\n";
        let expected = json("{\"note\":\"first\\r\\nsecond\\r\\n  indented\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_double_backtick_multiline_string() {
        // `` type: pipe-guarded content lines, mandatory closing glyph
        let input = " ``\n| first\n| second\n ``";
        let expected = json("\"first\\nsecond\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_double_backtick_with_explicit_lf_indicator() {
        let input = " ``\\n\n| first\n| second\n ``\\n";
        let expected = json("\"first\\nsecond\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_double_backtick_crlf_multiline_string() {
        // `` type with \r\n local EOL indicator
        let input = " ``\\r\\n\n| first\n| second\n ``\\r\\n";
        let expected = json("\"first\\r\\nsecond\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_double_backtick_with_fold() {
        // `` type with fold continuation line
        let input = " ``\n| first line that is \n/ continued here\n| second\n ``";
        let expected = json("\"first line that is continued here\\nsecond\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_single_backtick_multiline_string() {
        // ` type: content at n+2, mandatory closing glyph
        let input = "  note: `\n    first\n    second\n    indented\n   `";
        let expected = json("{\"note\":\"first\\nsecond\\nindented\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_single_backtick_with_fold() {
        // ` type with fold continuation
        let input = "  note: `\n    first line that is \n  / continued here\n    second\n   `";
        let expected = json("{\"note\":\"first line that is continued here\\nsecond\"}");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_single_backtick_with_leading_spaces_in_content() {
        // ` type preserves leading spaces after stripping n+2
        let input = " `\n  first\n    indented two extra\n  last\n `";
        let expected = json("\"first\\n  indented two extra\\nlast\"");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_triple_backtick_without_closing_glyph() {
        let input = "  note: ```\nfirst\nsecond";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn rejects_double_backtick_without_closing_glyph() {
        let input = " ``\n| first\n| second";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn rejects_single_backtick_without_closing_glyph() {
        let input = "  note: `\n    first\n    second";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn rejects_double_backtick_body_without_pipe() {
        let input = " ``\njust some text\n| second\n ``";
        assert!(parse_str(input).is_err());
    }

    #[test]
    fn parses_table_array_example() {
        let input = "  |a  |b   |c      |\n  |1  | x  |true   |\n  |2  | y  |false  |\n  |3  | z  |null   |";
        let expected = json(
            "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
        );
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
            expected
        );
    }

    #[test]
    fn parses_minimal_json_inside_array_example() {
        let input = "  [{\"a\":{\"b\":null},\"c\":3}]";
        let expected = json("[[{\"a\":{\"b\":null},\"c\":3}]]");
        assert_eq!(
            parse_str(input).unwrap().to_json().unwrap(),
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
        // Default: Bold style → `` with body at col 2
        let rendered =
            render_string(&tjson_value("{\"note\":\"first\\nsecond\\n  indented\"}")).unwrap();
        assert_eq!(
            rendered,
            "  note: ``\n| first\n| second\n|   indented\n   ``"
        );
    }

    #[test]
    fn renders_crlf_multiline_string_example() {
        // CrLf: Bold style with \r\n suffix
        let rendered = render_string(&tjson_value(
            "{\"note\":\"first\\r\\nsecond\\r\\n  indented\"}",
        ))
        .unwrap();
        assert_eq!(
            rendered,
            "  note: ``\\r\\n\n| first\n| second\n|   indented\n   ``\\r\\n"
        );
    }

    #[test]
    fn renders_single_backtick_root_string() {
        // Floating: indent=0: glyph is " `", body at indent+2 (2 spaces)
        let value = TjsonValue::String("line one\nline two".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert_eq!(rendered, " `\n  line one\n  line two\n `");
    }

    #[test]
    fn renders_single_backtick_shallow_key() {
        // Floating: pair_indent=2: glyph "   `", body at 4 spaces
        let rendered = render_string_with_options(
            &tjson_value("{\"note\":\"line one\\nline two\"}"),
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert_eq!(rendered, "  note: `\n    line one\n    line two\n   `");
    }

    #[test]
    fn renders_single_backtick_deep_key() {
        // Floating: pair_indent=4: glyph "     `", body at 6 spaces
        let rendered = render_string_with_options(
            &tjson_value("{\"outer\":{\"inner\":\"line one\\nline two\"}}"),
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert_eq!(
            rendered,
            "  outer:\n    inner: `\n      line one\n      line two\n     `"
        );
    }

    #[test]
    fn renders_single_backtick_three_lines() {
        // Floating: three content lines, deeper nesting — pair_indent=6, body at 8 spaces
        let rendered = render_string_with_options(
            &tjson_value("{\"a\":{\"b\":{\"c\":\"x\\ny\\nz\"}}}"),
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert_eq!(
            rendered,
            "  a:\n    b:\n      c: `\n        x\n        y\n        z\n       `"
        );
    }

    #[test]
    fn renders_double_backtick_with_bold_style() {
        // MultilineStyle::Bold → always `` with body at col 2
        let value = TjsonValue::String("line one\nline two".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                multiline_style: MultilineStyle::Bold,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, " ``\n| line one\n| line two\n ``");
    }

    #[test]
    fn renders_triple_backtick_with_fullwidth_style() {
        // MultilineStyle::Transparent → ``` with body at col 0
        let value = TjsonValue::String("normal line\nsecond line".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                multiline_style: MultilineStyle::Transparent,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, " ```\nnormal line\nsecond line\n ```");
    }

    #[test]
    fn renders_triple_backtick_falls_back_to_bold_when_pipe_heavy() {
        // Transparent falls back to Bold when content is pipe-heavy
        let value = TjsonValue::String("| piped\n| also piped\nnormal".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                multiline_style: MultilineStyle::Transparent,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert!(rendered.contains(" ``"), "expected `` fallback, got: {rendered}");
    }

    #[test]
    fn transparent_never_folds_body_lines_regardless_of_wrap() {
        // ``` bodies must never have / continuations — it's against spec.
        // Even with a very narrow wrap width and a long body line, no / appears.
        let long_line = "a".repeat(200);
        let value = TjsonValue::String(format!("{long_line}\nsecond line"));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .multiline_style(MultilineStyle::Transparent)
                .string_multiline_fold_style(FoldStyle::Auto),
        ).unwrap();
        // Falls back to Bold when body would need folding? Either way: no / inside the body.
        // Strip opener and closer lines and check no fold marker in body.
        let body_lines: Vec<&str> = rendered.lines()
            .filter(|l| !l.trim_start().starts_with("```") && !l.trim_start().starts_with("``"))
            .collect();
        for line in &body_lines {
            assert!(!line.trim_start().starts_with("/ "), "``` body must not have fold continuations: {rendered}");
        }
    }

    #[test]
    fn transparent_with_string_multiline_fold_style_auto_still_no_fold() {
        // Explicitly setting fold style to Auto on a Transparent multiline must not fold.
        // The note in the doc says it's ignored for Transparent.
        let value = TjsonValue::String("short\nsecond".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .multiline_style(MultilineStyle::Transparent)
                .string_multiline_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("```"), "should use triple backtick: {rendered}");
        assert!(!rendered.contains("/ "), "Transparent must never fold: {rendered}");
    }

    #[test]
    fn floating_falls_back_to_bold_when_line_count_exceeds_max() {
        // 11 lines > multiline_max_lines default of 10 → fall back from ` to ``
        let value = TjsonValue::String("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert!(rendered.starts_with(" ``"), "expected `` fallback for >10 lines, got: {rendered}");
    }

    #[test]
    fn floating_falls_back_to_bold_when_line_overflows_width() {
        // A content line longer than wrap_width - indent - 2 triggers fallback
        let long_line = "x".repeat(80); // exactly 80 chars: indent=0 + 2 = 82 > wrap_width=80
        let value = TjsonValue::String(format!("short\n{long_line}"));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert!(rendered.starts_with(" ``"), "expected `` fallback for overflow, got: {rendered}");
    }

    #[test]
    fn floating_renders_single_backtick_when_lines_fit() {
        // Only 2 lines, short content — stays as `
        let value = TjsonValue::String("normal line\nsecond line".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Floating, ..TjsonOptions::default() },
        ).unwrap();
        assert!(rendered.starts_with(" `\n"), "expected ` glyph, got: {rendered}");
        assert!(!rendered.contains("| "), "should not have pipe markers");
    }

    #[test]
    fn light_uses_single_backtick_when_safe() {
        let value = TjsonValue::String("short\nsecond".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Light, ..TjsonOptions::default() },
        )
        .unwrap();
        assert!(rendered.starts_with(" `\n"), "expected ` glyph, got: {rendered}");
    }

    #[test]
    fn light_stays_single_backtick_on_overflow() {
        // Width overflow does NOT trigger fallback for Light — stays as `
        let long = "x".repeat(80);
        let value = TjsonValue::String(format!("short\n{long}"));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Light, ..TjsonOptions::default() },
        )
        .unwrap();
        assert!(rendered.starts_with(" `\n"), "Light should stay as `, got: {rendered}");
        assert!(!rendered.contains("``"), "Light must not escalate to `` on overflow");
    }

    #[test]
    fn light_stays_single_backtick_on_too_many_lines() {
        // Too many lines does NOT trigger fallback for Light — stays as `
        let value = TjsonValue::String("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Light, ..TjsonOptions::default() },
        )
        .unwrap();
        assert!(rendered.starts_with(" `\n"), "Light should stay as `, got: {rendered}");
        assert!(!rendered.contains("``"), "Light must not escalate to `` on line count");
    }

    #[test]
    fn light_falls_back_to_bold_on_dangerous_content() {
        // Pipe-heavy content IS dangerous → Light falls back to ``
        let value = TjsonValue::String("| piped\n| also piped\nnormal".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::Light, ..TjsonOptions::default() },
        )
        .unwrap();
        assert!(rendered.starts_with(" ``"), "Light should fall back to `` for pipe-heavy content, got: {rendered}");
    }

    #[test]
    fn folding_quotes_uses_json_string_for_eol_strings() {
        let value = TjsonValue::String("first line\nsecond line".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::FoldingQuotes, ..TjsonOptions::default() },
        )
        .unwrap();
        assert!(rendered.starts_with(" \"") || rendered.starts_with("\""),
            "expected JSON string, got: {rendered}");
        assert!(!rendered.contains('`'), "FoldingQuotes must not use multiline glyphs");
    }

    #[test]
    fn folding_quotes_single_line_strings_unchanged() {
        // No EOL → FoldingQuotes does not apply, normal bare string rendering
        let value = TjsonValue::String("hello world".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions { multiline_style: MultilineStyle::FoldingQuotes, ..TjsonOptions::default() },
        )
        .unwrap();
        assert_eq!(rendered, " hello world");
    }

    #[test]
    fn folding_quotes_folds_long_eol_string() {
        // A string with EOL that encodes long enough to need folding.
        // JSON encoding of "long string with spaces that needs folding\nsecond" = 52 chars,
        // overrun=12 > 25% of 40=10 → fold is triggered (has spaces for fold points).
        let value = TjsonValue::String("long string with spaces that needs folding\nsecond".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                multiline_style: MultilineStyle::FoldingQuotes,
                wrap_width: Some(40),
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation, got: {rendered}");
        assert!(!rendered.contains('`'), "must not use multiline glyphs");
    }

    #[test]
    fn folding_quotes_skips_fold_when_overrun_within_25_percent() {
        // String whose JSON encoding slightly exceeds wrap_width=40 but by less than 25% (10).
        // FoldingQuotes always folds at \n boundaries regardless of line length.
        let value = TjsonValue::String("abcdefghijklmnopqrstuvwxyz123456\nsecond".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                multiline_style: MultilineStyle::FoldingQuotes,
                wrap_width: Some(40),
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "\"abcdefghijklmnopqrstuvwxyz123456\\n\n/ second\"");
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
    fn pipe_heavy_content_falls_back_to_double_backtick() {
        // >10% of lines start with whitespace then | → use `` instead of `
        // 2 out of 3 lines start with |, which is >10%
        let value = TjsonValue::String("| line one\n| line two\nnormal line".to_owned());
        let rendered = render_string(&value).unwrap();
        assert!(rendered.contains(" ``"), "expected `` glyph, got: {rendered}");
        assert!(rendered.contains("| | line one"), "expected piped body");
    }

    #[test]
    fn triple_backtick_collision_falls_back_to_double_backtick() {
        // A content line starting with backtick triggers the backtick_start heuristic → use ``
        // (` ``` ` starts with a backtick, so backtick_start is true)
        let value = TjsonValue::String(" ```\nsecond line".to_owned());
        let rendered = render_string(&value).unwrap();
        assert!(rendered.contains(" ``"), "expected `` glyph, got: {rendered}");
    }

    #[test]
    fn backtick_content_falls_back_to_double_backtick() {
        // A content line starting with whitespace then any backtick forces fallback from ` to ``
        // (visually confusing for humans even if parseable)
        let value = TjsonValue::String("normal line\n  `` something".to_owned());
        let rendered = render_string(&value).unwrap();
        assert!(rendered.contains(" ``"), "expected `` glyph, got: {rendered}");
        assert!(rendered.contains("| normal line"), "expected pipe-guarded body");
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
    fn table_rejected_when_shared_keys_have_different_order() {
        // {"a":1,"b":2} has keys [a, b]; {"b":3,"a":4} has keys [b, a].
        // Rendering as a table would silently reorder keys on round-trip — hard stop.
        let value = tjson_value(
            "[{\"a\":1,\"b\":2,\"c\":3},{\"b\":4,\"a\":5,\"c\":6},{\"a\":7,\"b\":8,\"c\":9}]",
        );
        let rendered = render_string(&value).unwrap();
        assert!(!rendered.contains('|'), "should not render as table when key order differs: {rendered}");
    }

    #[test]
    fn table_allowed_when_rows_have_subset_of_keys() {
        // Row 2 is missing "c" — that's fine, it's sparse not reordered.
        let value = tjson_value(
            "[{\"a\":1,\"b\":2,\"c\":3},{\"a\":4,\"b\":5},{\"a\":6,\"b\":7,\"c\":8}]",
        );
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default().table_min_similarity(0.5),
        ).unwrap();
        assert!(rendered.contains('|'), "should render as table when rows are a subset: {rendered}");
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
    fn packs_explicit_nested_arrays_and_objects_kv1() {
        let value = tjson_value(
            "{\"nested\":[[1,2],[3,4]],\"rows\":[{\"a\":1,\"b\":2},{\"c\":3,\"d\":4}]}",
        );
        let rendered = render_string_with_options(&value, TjsonOptions::default().kv_pack_multiple(1).unwrap()).unwrap();
        assert_eq!(
            rendered,
            "  nested:\n  [ [ 1, 2\n    [ 3, 4\n  rows:\n  [ { a:1  b:2\n    { c:3  d:4"
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
            "  nested:\n  [ [ 1, 2\n    [ 3, 4\n  rows:\n  [ { a:1    b:2\n    { c:3    d:4"
        );
    }

    #[test]
    fn wraps_long_packed_arrays_before_falling_back_to_multiline() {
        let value =
            tjson_value("{\"data\":[100,200,300,400,500,600,700,800,900,1000,1100,1200,1300]}");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                wrap_width: Some(40),
                ..TjsonOptions::default()
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
            TjsonOptions {
                bare_strings: BareStyle::None,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "  greeting:\"hello world\"\n  items:  \"alpha\", \"beta\""
        );
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn bare_keys_none_quotes_keys_in_objects_and_tables_kv1() {
        let object_value = tjson_value("{\"alpha\":1,\"beta key\":2}");
        let rendered_object = render_string_with_options(
            &object_value,
            TjsonOptions {
                bare_keys: BareStyle::None,
                kv_pack_multiple: 1,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered_object, "  \"alpha\":1  \"beta key\":2");
    }

    #[test]
    fn bare_keys_none_quotes_keys_in_objects_and_tables() {
        let object_value = tjson_value("{\"alpha\":1,\"beta key\":2}");
        let rendered_object = render_string_with_options(
            &object_value,
            TjsonOptions {
                bare_keys: BareStyle::None,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered_object, "  \"alpha\":1    \"beta key\":2");

        let table_value = tjson_value(
            "{\"rows\":[{\"alpha\":1,\"beta\":2},{\"alpha\":3,\"beta\":4},{\"alpha\":5,\"beta\":6}]}",
        );
        let rendered_table = render_string_with_options(
            &table_value,
            TjsonOptions {
                bare_keys: BareStyle::None,
                table_min_columns: 2,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered_table,
            "  \"rows\":\n    |\"alpha\"  |\"beta\"  |\n    |1        |2       |\n    |3        |4       |\n    |5        |6       |"
        );
        let reparsed = parse_str(&rendered_table)
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(reparsed, table_value.to_json().unwrap());
    }

    #[test]
    fn force_markers_applies_to_root_and_key_nested_single_levels_kv1() {
        let value =
            tjson_value("{\"a\":5,\"6\":\"fred\",\"xy\":[],\"de\":{},\"e\":[1],\"o\":{\"k\":2}}");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                force_markers: true,
                kv_pack_multiple: 1,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "{ a:5  6: fred  xy:[]  de:{}\n  e:  1\n  o:\n  { k:2"
        );
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn force_markers_applies_to_root_and_key_nested_single_levels() {
        let value =
            tjson_value("{\"a\":5,\"6\":\"fred\",\"xy\":[],\"de\":{},\"e\":[1],\"o\":{\"k\":2}}");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                force_markers: true,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            rendered,
            "{ a:5    6: fred    xy:[]    de:{}\n  e:  1\n  o:\n  { k:2"
        );
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn force_markers_applies_to_root_arrays() {
        let value = tjson_value("[1,2,3]");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                force_markers: true,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "[ 1, 2, 3");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn force_markers_suppresses_table_rendering_for_array_containers() {
        let value = tjson_value("[{\"a\":1,\"b\":2},{\"a\":3,\"b\":4},{\"a\":5,\"b\":6}]");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                force_markers: true,
                table_min_columns: 2,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "[ |a  |b  |\n  |1  |2  |\n  |3  |4  |\n  |5  |6  |");
    }

    #[test]
    fn string_array_style_spaces_forces_space_packing() {
        let value = tjson_value("{\"items\":[\"alpha\",\"beta\",\"gamma\"]}");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                string_array_style: StringArrayStyle::Spaces,
                ..TjsonOptions::default()
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
            TjsonOptions {
                string_array_style: StringArrayStyle::None,
                ..TjsonOptions::default()
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
            TjsonOptions {
                string_array_style: StringArrayStyle::Comma,
                wrap_width: Some(18),
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        let prefer_comma = render_string_with_options(
            &value,
            TjsonOptions {
                string_array_style: StringArrayStyle::PreferComma,
                wrap_width: Some(18),
                ..TjsonOptions::default()
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
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn spaces_style_quotes_comma_strings_and_round_trips() {
        let value = tjson_value("{\"items\":[\"apples, oranges\",\"pears, plums\"]}");
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                string_array_style: StringArrayStyle::Spaces,
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert_eq!(rendered, "  items:  \"apples, oranges\"  \"pears, plums\"");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn canonical_rendering_disables_tables_and_inline_packing() {
        let value = tjson_value(
            "[{\"a\":1,\"b\":\"x\",\"c\":true},{\"a\":2,\"b\":\"y\",\"c\":false},{\"a\":3,\"b\":\"z\",\"c\":null}]",
        );
        let rendered = render_string_with_options(&value, TjsonOptions::canonical())
            .unwrap();
        assert!(!rendered.contains('|'));
        assert!(!rendered.contains(", "));
    }

    // --- Fold style tests ---
    // Fixed and None have deterministic output — exact assertions.
    // Auto tests use strings with exactly one reasonable fold point (one space between
    // two equal-length words) so the fold position is unambiguous.

    #[test]
    fn bare_fold_none_does_not_fold() {
        // "aaaaa bbbbb" at wrap=15 overflows (line would be 17 chars), but None means no fold.
        let value = TjsonValue::from(json(r#"{"k":"aaaaa bbbbb"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(15))
                .string_bare_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/ "), "None fold style must not fold: {rendered}");
    }

    #[test]
    fn bare_fold_fixed_folds_at_wrap_width() {
        // "aaaaabbbbbcccccdddd" (19 chars, no spaces), wrap=20.
        // Line "  k: aaaaabbbbbcccccdddd" = 24 chars > 20.
        // first_avail = 20-2(indent)-1(space)-2(k:) = 15.
        // Fixed splits at 15: first="aaaaabbbbbccccc", cont="dddd".
        let value = TjsonValue::from(json(r#"{"k":"aaaaabbbbbcccccdddd"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_bare_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "Fixed must fold: {rendered}");
        assert!(!rendered.contains("/ ") || rendered.lines().count() == 2, "exactly one fold: {rendered}");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aaaaabbbbbcccccdddd"}"#));
    }

    #[test]
    fn bare_fold_auto_folds_at_single_space() {
        // "aaaaa bbbbbccccc": single space at pos 5, total 16 chars.
        // wrap=20: first_avail = 20-2(indent)-1(space)-2(k:) = 15. 16 > 15 → must fold.
        // Auto folds before the space: "aaaaa" / " bbbbbccccc".
        let value = TjsonValue::from(json(r#"{"k":"aaaaa bbbbbccccc"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_bare_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert_eq!(rendered, "  k: aaaaa\n  /  bbbbbccccc");
    }

    #[test]
    fn bare_fold_auto_folds_at_word_boundary_slash() {
        // "aaaaa/bbbbbccccc": StickyEnd→Letter boundary after '/' at pos 6, total 16 chars.
        // No spaces → P2 fires: fold after '/', slash trails the line.
        // wrap=20: first_avail=15. 16 > 15 → must fold. Fold at pos 6: first="aaaaa/".
        let value = TjsonValue::from(json(r#"{"k":"aaaaa/bbbbbccccc"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_bare_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        assert!(rendered.contains("aaaaa/\n"), "slash must trail the line: {rendered}");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aaaaa/bbbbbccccc"}"#));
    }

    #[test]
    fn bare_fold_auto_prefers_space_over_word_boundary() {
        // "aa/bbbbbbbbb cccc": slash at pos 2, space at pos 11, total 17 chars.
        // wrap=20: first_avail=15. 17 > 15 → must fold. Space at pos 11 ≤ 15 → fold at 11.
        // Space pass runs first and finds pos 11 — fold before space: "aa/bbbbbbbbb" / " cccc".
        let value = TjsonValue::from(json(r#"{"k":"aa/bbbbbbbbb cccc"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_bare_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        // Must fold at the space, not at the slash
        assert!(rendered.contains("aa/bbbbbbbbb\n"), "must fold at space not slash: {rendered}");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aa/bbbbbbbbb cccc"}"#));
    }

    #[test]
    fn quoted_fold_auto_folds_at_word_boundary_slash() {
        // bare_strings=None forces quoting. "aaaaa/bbbbbcccccc" has one slash boundary.
        // encoded = "\"aaaaa/bbbbbcccccc\"" = 19 chars. wrap=20, indent=2, key+colon=2 → first_avail=16.
        // 19 > 16 → folds. Word boundary before '/' at inner pos 5. Slash → unambiguous.
        let value = TjsonValue::from(json(r#"{"k":"aaaaa/bbbbbcccccc"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .bare_strings(BareStyle::None)
                .string_quoted_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aaaaa/bbbbbcccccc"}"#));
    }

    #[test]
    fn quoted_fold_none_does_not_fold() {
        // bare_strings=None and bare_keys=None force quoting of both key and value.
        // wrap=20 overflows ("\"kk\": \"aaaaabbbbbcccccdddd\"" = 27 chars), but fold style None means no fold.
        let value = TjsonValue::from(json(r#"{"kk":"aaaaabbbbbcccccdddd"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .bare_strings(BareStyle::None)
                .bare_keys(BareStyle::None)
                .string_quoted_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(rendered.contains('"'), "must be quoted");
        assert!(!rendered.contains("/ "), "None fold style must not fold: {rendered}");
    }

    #[test]
    fn quoted_fold_fixed_folds_and_roundtrips() {
        // bare_strings=None forces quoting. "aaaaabbbbbcccccdd" encoded = "\"aaaaabbbbbcccccdd\"" = 19 chars.
        // wrap=20, indent=2, key "k"+colon = 2 → first_avail = 20-2-2 = 16. 19 > 16 → folds.
        let value = TjsonValue::from(json(r#"{"k":"aaaaabbbbbcccccdd"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .bare_strings(BareStyle::None)
                .string_quoted_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "Fixed must fold: {rendered}");
        assert!(!rendered.contains('`'), "must be a JSON string fold, not multiline");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aaaaabbbbbcccccdd"}"#));
    }

    #[test]
    fn quoted_fold_auto_folds_at_single_space() {
        // bare_strings=None forces quoting. "aaaaa bbbbbccccc" has one space at pos 5.
        // encoded "\"aaaaa bbbbbccccc\"" = 18 chars. wrap=20, indent=2, key+colon=2 → first_avail=16.
        // 18 > 16 → folds. Auto folds before the space.
        let value = TjsonValue::from(json(r#"{"k":"aaaaa bbbbbccccc"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .bare_strings(BareStyle::None)
                .string_quoted_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "Auto must fold: {rendered}");
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"k":"aaaaa bbbbbccccc"}"#));
    }

    #[test]
    fn multiline_fold_none_does_not_fold_body_lines() {
        // Body line overflows wrap but None means no fold inside multiline body.
        let value = TjsonValue::String("aaaaabbbbbcccccdddddeeeeefff\nsecond".to_owned());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_multiline_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(rendered.contains('`'), "must be multiline");
        assert!(rendered.contains("aaaaabbbbbcccccddddd"), "body must not be folded: {rendered}");
    }

    #[test]
    fn fold_style_none_on_all_types_produces_no_fold_continuations() {
        // With all fold styles None, no / continuations should appear anywhere.
        let value = TjsonValue::from(json(r#"{"a":"aaaaa bbbbbccccc","b":"x,y,z abcdefghij"}"#));
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(20))
                .string_bare_fold_style(FoldStyle::None)
                .string_quoted_fold_style(FoldStyle::None)
                .string_multiline_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/ "), "no fold continuations expected: {rendered}");
    }

    #[test]
    fn number_fold_none_does_not_fold() {
        // number_fold_style None: long number is never folded even when it overflows wrap.
        let value = TjsonValue::Number("123456789012345678901234".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/ "), "expected no fold: {rendered}");
        assert!(rendered.contains("123456789012345678901234"), "must contain full number: {rendered}");
    }

    #[test]
    fn number_fold_fixed_splits_between_digits() {
        // 24 digits, wrap=20, indent=0 → avail=20. Fixed splits at pos 20.
        let value = TjsonValue::Number("123456789012345678901234".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("123456789012345678901234".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_auto_prefers_decimal_point() {
        // "1234567890123456789.01" (22 chars, '.' at pos 19), wrap=20, avail=20.
        // rfind('.') in first 20 chars = pos 19. Fold before '.'.
        // First line ends with the integer part.
        let value = TjsonValue::Number("1234567890123456789.01".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert!(first_line.ends_with("1234567890123456789"), "should fold before `.`: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("1234567890123456789.01".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_auto_prefers_exponent() {
        // "1.23456789012345678e+97" (23 chars, 'e' at pos 19), wrap=20, avail=20.
        // rfind('.') or 'e'/'E' in first 20 chars: '.' at 1, 'e' at 19 → picks 'e' (rightmost).
        // First line: "1.23456789012345678", continuation: "/ e+97".
        let value = TjsonValue::Number("1.23456789012345678e+97".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert!(first_line.ends_with("1.23456789012345678"), "should fold before `e`: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("1.23456789012345678e+97".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_auto_folds_before_decimal_point() {
        // "1234567890123456789.01" (22 chars, '.' at pos 19), wrap=20, avail=20.
        // rfind('.') in first 20 = pos 19. Fold before '.'.
        // First line: "1234567890123456789", continuation: "/ .01".
        let value = TjsonValue::Number("1234567890123456789.01".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert!(first_line.ends_with("1234567890123456789"),
            "should fold before '.': {rendered}");
        let cont_line = rendered.lines().nth(1).unwrap();
        assert!(cont_line.starts_with("/ ."),
            "continuation must start with '/ .': {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("1234567890123456789.01".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_auto_folds_before_exponent() {
        // "1.23456789012345678e+97" (23 chars, 'e' at pos 19), wrap=20, avail=20.
        // rfind('e') in first 20 chars = pos 19. Fold before 'e'.
        // First line: "1.23456789012345678", continuation: "/ e+97".
        let value = TjsonValue::Number("1.23456789012345678e+97".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert!(first_line.ends_with("1.23456789012345678"),
            "should fold before 'e': {rendered}");
        let cont_line = rendered.lines().nth(1).unwrap();
        assert!(cont_line.starts_with("/ e"),
            "continuation must start with '/ e': {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("1.23456789012345678e+97".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_fixed_splits_at_wrap_boundary() {
        // 21 digits, wrap=20, indent=0: avail=20. Fixed splits exactly at pos 20.
        // First line: "12345678901234567890", continuation: "/ 1".
        let value = TjsonValue::Number("123456789012345678901".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert_eq!(first_line, "12345678901234567890",
            "fixed fold must split exactly at wrap=20: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("123456789012345678901".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn number_fold_auto_falls_back_to_digit_split() {
        // 24 digits, no '.'/`e`: auto falls back to digit-boundary split.
        // wrap=20, indent=0 → avail=20. Split at pos 20 (digit-digit boundary).
        let value = TjsonValue::Number("123456789012345678901234".parse().unwrap());
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(20))
                .number_fold_style(FoldStyle::Auto),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let first_line = rendered.lines().next().unwrap();
        assert_eq!(first_line, "12345678901234567890",
            "auto fallback must split at digit boundary at wrap=20: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap();
        assert_eq!(reparsed, TjsonValue::Number("123456789012345678901234".parse().unwrap()),
            "roundtrip must recover original number");
    }

    #[test]
    fn bare_key_fold_fixed_folds_and_roundtrips() {
        // Key "abcdefghijklmnopqrst" (20 chars) + ":" = 21, indent=0, wrap=15.
        // Only one place to fold: at the wrap boundary between two key chars.
        let value = TjsonValue::from(json(r#"{"abcdefghijklmnopqrst":1}"#));
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(15))
                .string_bare_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"abcdefghijklmnopqrst":1}"#),
            "roundtrip must recover original key");
    }

    #[test]
    fn bare_key_fold_none_does_not_fold() {
        // Same long key but fold style None — must not fold.
        let value = TjsonValue::from(json(r#"{"abcdefghijklmnopqrst":1}"#));
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(15))
                .string_bare_fold_style(FoldStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/ "), "expected no fold: {rendered}");
    }

    #[test]
    fn quoted_key_fold_fixed_folds_and_roundtrips() {
        // bare_keys=None forces quoting. Key "abcdefghijklmnop" (16 chars),
        // quoted = "\"abcdefghijklmnop\"" = 18 chars, indent=0, wrap=15.
        // Single fold at the wrap boundary.
        let value = TjsonValue::from(json(r#"{"abcdefghijklmnop":1}"#));
        let rendered = value.to_tjson_with(
            TjsonOptions::default()
                .wrap_width(Some(15))
                .bare_keys(BareStyle::None)
                .string_quoted_fold_style(FoldStyle::Fixed),
        ).unwrap();
        assert!(rendered.contains("/ "), "expected fold continuation: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap().to_json().unwrap();
        assert_eq!(reparsed, json(r#"{"abcdefghijklmnop":1}"#),
            "roundtrip must recover original key");
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
            let rendered = render_string(&TjsonValue::from(value.clone())).unwrap();
            let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
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
        let json = value.to_json().unwrap();
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
        let json_value = value.to_json().unwrap();
        assert_eq!(json_value, json("{\"dup\":2,\"keep\":3}"));
    }

    // ---- /< /> indent-offset tests ----

    #[test]
    fn expand_indent_adjustments_noops_when_no_glyph_present() {
        let input = "  a:1\n  b:2\n";
        assert_eq!(expand_indent_adjustments(input), input);
    }

    #[test]
    fn expand_indent_adjustments_removes_opener_and_re_indents_content() {
        // pair_indent=2 ("  outer: /<"), then table at visual 2 → actual 4.
        let input = "  outer: /<\n  |a  |b  |\n  | x  | y  |\n   />\n  sib:1\n";
        let result = expand_indent_adjustments(input);
        // "  outer: /<" → "  outer:" (offset pushed = 2)
        // "  |a  |b  |" at file-indent 2 → effective 4 → "    |a  |b  |"
        // "   />" → pop, discarded
        // "  sib:1" → offset=0, unchanged
        let expected = "  outer:\n    |a  |b  |\n    | x  | y  |\n  sib:1\n";
        assert_eq!(result, expected);
    }

    #[test]
    fn expand_indent_adjustments_handles_nested_opener() {
        // Two stacked /< contexts.
        let input = "  a: /<\n  b: /<\n  c:1\n   />\n  d:2\n   />\n  e:3\n";
        let result = expand_indent_adjustments(input);
        // After "  a: /<": offset=2
        // "  b: /<" at file-indent 2 → eff=4, emit "    b:", push offset=4
        // "  c:1" at file-indent 2 → eff=6 → "      c:1"
        // "   />" → pop offset to 2
        // "  d:2" at file-indent 2 → eff=4 → "    d:2"
        // "   />" → pop offset to 0
        // "  e:3" unchanged
        let expected = "  a:\n    b:\n      c:1\n    d:2\n  e:3\n";
        assert_eq!(result, expected);
    }

    #[test]
    fn parses_indent_offset_table() {
        // pair_indent=4 ("    h: /<"), table at visual 2 → actual 6.
        let input = concat!(
            "  outer:\n",
            "    h: /<\n",
            "  |name  |score  |\n",
            "  | Alice  |100  |\n",
            "  | Bob    |200  |\n",
            "  | Carol  |300  |\n",
            "     />\n",
            "    sib: value\n",
        );
        let value = parse_str(input).unwrap().to_json().unwrap();
        let expected = serde_json::json!({
            "outer": {
                "h": [
                    {"name": "Alice",  "score": 100},
                    {"name": "Bob",    "score": 200},
                    {"name": "Carol",  "score": 300},
                ],
                "sib": "value"
            }
        });
        assert_eq!(value, expected);
    }

    #[test]
    fn parses_indent_offset_deep_nesting() {
        // Verify that a second /< context stacks correctly and /> restores it.
        let input = concat!(
            "  a:\n",
            "    b: /<\n",
            "  c: /<\n",
            "  d:99\n",
            "   />\n",
            "  e:42\n",
            "     />\n",
            "  f:1\n",
        );
        let value = parse_str(input).unwrap().to_json().unwrap();
        // After both /> pops, offset returns to 0, so "  f:1" is at pair_indent 2 —
        // a sibling of "a", not inside "b".
        let expected = serde_json::json!({
            "a": {"b": {"c": {"d": 99}, "e": 42}},
            "f": 1
        });
        assert_eq!(value, expected);
    }

    #[test]
    fn renderer_uses_indent_offset_for_deep_tables_that_overflow() {
        // 8 levels deep → pair_indent=16, n*5=80 >= w=80.
        // Table is wide enough to overflow at natural indent but fit at offset.
        let deep_table_json = r#"{
            "a":{"b":{"c":{"d":{"e":{"f":{"g":{"h":[
                {"c1":"really long value 1","c2":"somewhat long val 1","c3":"another long val 12"},
                {"c1":"row two c1 value","c2":"row two c2 value","c3":"row two c3 value"},
                {"c1":"row three c1 val","c2":"row three c2 val","c3":"row three c3 val"}
            ]}}}}}}}}
        "#;
        let value = TjsonValue::from(serde_json::from_str::<JsonValue>(deep_table_json).unwrap());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                wrap_width: Some(80),
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert!(
            rendered.contains(" /<"),
            "expected /< in rendered output:\n{rendered}"
        );
        assert!(
            rendered.contains("/>"),
            "expected /> in rendered output:\n{rendered}"
        );
        // Round-trip: parse the rendered output and verify it matches.
        let reparsed = parse_str(&rendered).unwrap().to_json().unwrap();
        let original = value.to_json().unwrap();
        assert_eq!(reparsed, original);
    }

    #[test]
    fn renderer_does_not_use_indent_offset_with_unlimited_wrap() {
        let deep_table_json = r#"{
            "a":{"b":{"c":{"d":{"e":{"f":{"g":{"h":[
                {"c1":"really long value 1","c2":"somewhat long val 1","c3":"another long val 12"},
                {"c1":"row two c1 value","c2":"row two c2 value","c3":"row two c3 value"},
                {"c1":"row three c1 val","c2":"row three c2 val","c3":"row three c3 val"}
            ]}}}}}}}}
        "#;
        let value = TjsonValue::from(serde_json::from_str::<JsonValue>(deep_table_json).unwrap());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                wrap_width: None, // unlimited
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert!(
            !rendered.contains(" /<"),
            "expected no /< with unlimited wrap:\n{rendered}"
        );
    }

    // --- TableUnindentStyle tests ---
    // Uses a 3-level-deep table that overflows at its natural indent but fits at 0.
    // pair_indent = 6 (3 nesting levels × 2), table rows are ~60 chars wide.

    fn deep3_table_value() -> TjsonValue {
        TjsonValue::from(serde_json::from_str::<JsonValue>(r#"{
            "a":{"b":{"c":[
                {"col1":"value one here","col2":"value two here","col3":"value three here"},
                {"col1":"row two col1","col2":"row two col2","col3":"row two col3"},
                {"col1":"row three c1","col2":"row three c2","col3":"row three c3"}
            ]}}}"#).unwrap())
    }

    #[test]
    fn table_unindent_style_none_never_uses_glyphs() {
        // None: never unindent even if table overflows. No /< /> in output.
        let rendered = render_string_with_options(
            &deep3_table_value(),
            TjsonOptions::default()
                .wrap_width(Some(50))
                .table_unindent_style(TableUnindentStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/<"), "None must not use indent glyphs: {rendered}");
    }

    #[test]
    fn table_unindent_style_left_always_uses_glyphs_when_fits_at_zero() {
        // Left: always push to indent 0 even when table fits at natural indent.
        // Use unlimited width so table fits naturally, but Left still unindents.
        let rendered = render_string_with_options(
            &deep3_table_value(),
            TjsonOptions::default()
                .wrap_width(None)
                .table_unindent_style(TableUnindentStyle::Left),
        ).unwrap();
        assert!(rendered.contains("/<"), "Left must always use indent glyphs: {rendered}");
        let reparsed = rendered.parse::<TjsonValue>().unwrap().to_json().unwrap();
        assert_eq!(reparsed, deep3_table_value().to_json().unwrap());
    }

    #[test]
    fn table_unindent_style_auto_uses_glyphs_only_on_overflow() {
        let value = deep3_table_value();
        // With wide wrap: table fits at natural indent → no glyphs.
        let wide = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(None)
                .table_unindent_style(TableUnindentStyle::Auto),
        ).unwrap();
        assert!(!wide.contains("/<"), "Auto must not use glyphs when table fits: {wide}");

        // With narrow wrap (60): table rows are 65 chars, overflows. data_width=57 ≤ 60 → fits at 0.
        let narrow = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(60))
                .table_unindent_style(TableUnindentStyle::Auto),
        ).unwrap();
        assert!(narrow.contains("/<"), "Auto must use glyphs on overflow: {narrow}");
        let reparsed = narrow.parse::<TjsonValue>().unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn table_unindent_style_floating_pushes_minimum_needed() {
        // Floating: push left only enough to fit, not all the way to 0.
        // pair_indent=6, table data_width ≈ 58 chars. With wrap=70:
        // natural width = 6+2+58=66 ≤ 70 → fits → no glyphs.
        // With wrap=60: natural=66 > 60, but data_width=58 > 60-2=58 → exactly fits at target=0.
        // Use wrap=65: natural=66 > 65, target = 65-58-2=5 < 6=n → unindents to 5 (not 0).
        let value = deep3_table_value();
        let rendered = render_string_with_options(
            &value,
            TjsonOptions::default()
                .wrap_width(Some(65))
                .table_unindent_style(TableUnindentStyle::Floating),
        ).unwrap();
        // Should use glyphs but NOT go all the way to indent 0.
        // If it goes to 0, rows start at indent 2 ("  |col1...").
        // If floating, rows are at indent > 2.
        if rendered.contains("/<") {
            let row_line = rendered.lines().find(|l| l.contains('|') && !l.contains("/<") && !l.contains("/>")).unwrap_or("");
            let row_indent = row_line.len() - row_line.trim_start().len();
            assert!(row_indent > 2, "Floating must not push all the way to indent 0: {rendered}");
        }
        let reparsed = rendered.parse::<TjsonValue>().unwrap().to_json().unwrap();
        assert_eq!(reparsed, value.to_json().unwrap());
    }

    #[test]
    fn table_unindent_style_none_with_indent_glyph_none_also_no_glyphs() {
        // Both None: definitely no glyphs. Belt and suspenders.
        let rendered = render_string_with_options(
            &deep3_table_value(),
            TjsonOptions::default()
                .wrap_width(Some(50))
                .table_unindent_style(TableUnindentStyle::None)
                .indent_glyph_style(IndentGlyphStyle::None),
        ).unwrap();
        assert!(!rendered.contains("/<"), "must not use indent glyphs: {rendered}");
    }

    #[test]
    fn table_unindent_style_left_independent_of_indent_glyph_none() {
        // indent_glyph_style=None disables object glyphs but does not block table unindent.
        let rendered = render_string_with_options(
            &deep3_table_value(),
            TjsonOptions::default()
                .wrap_width(None)
                .table_unindent_style(TableUnindentStyle::Left)
                .indent_glyph_style(IndentGlyphStyle::None),
        ).unwrap();
        assert!(rendered.contains("/<"), "table_unindent_style=Left must still fire with indent_glyph_style=None: {rendered}");
    }

    #[test]
    fn renderer_does_not_use_indent_offset_when_indent_is_small() {
        // pair_indent=2 → n*5=10 < w=80, so offset should never apply.
        let json_str = r#"{"h":[
            {"c1":"really long value 1","c2":"somewhat long val 1","c3":"another long val 12"},
            {"c1":"row two c1 value","c2":"row two c2 value","c3":"row two c3 value"},
            {"c1":"row three c1 val","c2":"row three c2 val","c3":"row three c3 val"}
        ]}"#;
        let value = TjsonValue::from(serde_json::from_str::<JsonValue>(json_str).unwrap());
        let rendered = render_string_with_options(
            &value,
            TjsonOptions {
                wrap_width: Some(80),
                ..TjsonOptions::default()
            },
        )
        .unwrap();
        assert!(
            !rendered.contains(" /<"),
            "expected no /< when indent is small:\n{rendered}"
        );
    }

    #[test]
    fn tjson_config_camel_case_enums() {
        // multi-word camelCase variants
        let c: TjsonConfig = serde_json::from_str(r#"{"stringArrayStyle":"preferSpaces","multilineStyle":"boldFloating"}"#).unwrap();
        assert_eq!(c.string_array_style, Some(StringArrayStyle::PreferSpaces));
        assert_eq!(c.multiline_style, Some(MultilineStyle::BoldFloating));

        // PascalCase still works
        let c: TjsonConfig = serde_json::from_str(r#"{"stringArrayStyle":"PreferComma","multilineStyle":"FoldingQuotes"}"#).unwrap();
        assert_eq!(c.string_array_style, Some(StringArrayStyle::PreferComma));
        assert_eq!(c.multiline_style, Some(MultilineStyle::FoldingQuotes));

        // single-word lowercase (BareStyle, FoldStyle, IndentGlyphStyle, TableUnindentStyle, IndentGlyphMarkerStyle)
        let c: TjsonConfig = serde_json::from_str(r#"{
            "bareStrings": "prefer",
            "numberFoldStyle": "auto",
            "indentGlyphStyle": "fixed",
            "tableUnindentStyle": "floating",
            "indentGlyphMarkerStyle": "compact"
        }"#).unwrap();
        assert_eq!(c.bare_strings, Some(BareStyle::Prefer));
        assert_eq!(c.number_fold_style, Some(FoldStyle::Auto));
        assert_eq!(c.indent_glyph_style, Some(IndentGlyphStyle::Fixed));
        assert_eq!(c.table_unindent_style, Some(TableUnindentStyle::Floating));
        assert_eq!(c.indent_glyph_marker_style, Some(IndentGlyphMarkerStyle::Compact));
    }
}
