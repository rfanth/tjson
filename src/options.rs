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


#[non_exhaustive]
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct ParseOptions {
    start_indent: usize,
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
    pub(crate) bare_strings: BareStyle,
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
}

impl Default for RenderOptions {
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
/// Not part of the public Rust API — use [`RenderOptions`] directly in Rust code.
#[doc(hidden)]
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TjsonConfig {
    pub(crate) canonical: bool,
    pub(crate) force_markers: Option<bool>,
    pub(crate) wrap_width: Option<usize>,
    #[serde(deserialize_with = "camel_de::bare_style")]
    pub(crate) bare_strings: Option<BareStyle>,
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
        opts
    }
}

