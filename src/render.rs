use std::marker::PhantomData;

use crate::document::{Comment, Placement};
use crate::options::{BareStyle, StringStyle, FoldStyle, IndentGlyphMarkerStyle, IndentGlyphMode, MultilineStyle, StringArrayStyle, TableUnindentStyle, RenderOptions, SPEC_FORMS, MIN_FOLD_CONTINUATION, indent_glyph_mode};
use crate::position::{Columns, FileIndent, Glyph, LEVEL, Marker, Spaces, indent_marked};
use crate::tree::{BareForm, KeyForm, MultilineFlavor, NodeRef, StringForm, Tree};
use crate::value::{BareString, StrMeta, TableBareString};
use crate::util::*;
use crate::parse::MultilineLocalEol;

/// A borrowed node guaranteed to be neither a nonempty array nor a nonempty object —
/// the category of values that can appear inline in TJSON (table cells, packed tokens, etc.).
/// Strings carry their recorded form so honoring survives the flattening to a token.
#[derive(Clone, Copy)]
pub(crate) enum BasicValue<'a> {
    Null,
    Bool(bool),
    Number(&'a crate::number::Number),
    String(&'a str, Option<StringForm>),
    EmptyArray,
    EmptyObject,
}

/// How a string will be rendered. See [`string_rendering`].
///
/// Not a grouping by kind: the two multiline arms are the same shape chosen two
/// different ways, and they render differently because an honored flavour bypasses
/// the width and line-count preferences that style-driven selection applies. What
/// the arms share is that each answers to one fold style -- bare to
/// `string_bare_fold_style`, quoted to `string_quoted_fold_style`, either multiline
/// to `string_multiline_fold_style`, and folding quotes to none at all, that form
/// being the folding itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StringRendering {
    Bare(BareForm),
    Quoted,
    FoldingQuotes,
    HonoredMultiline(MultilineFlavor),
    StyleMultiline,
}

/// How this string will be rendered.
///
/// A free function because nothing in it touches the tree type: it was briefly a
/// method, and the giveaway was the call site naming an arbitrary `Renderer<Value>`
/// purely to reach it.
///
/// Extracted from [`Renderer::render_string_lines`], which dispatches on the
/// result, so the decision has one definition. Which fold style governs a value
/// follows from it, and that was the thing a caller outside the renderer needed
/// and could not ask -- so it guessed, and the guess read the *key's* style to
/// decide about a value.
/// Whether `foldingQuotes` governs this value.
///
/// Asked in two places that must agree: the honored-form arm, which has to know not
/// to preempt it, and the style dispatch below. They disagreed, which is how a
/// document rendered with `foldingQuotes` came back unfolded -- the recorded
/// `Quoted` answered first and the style never got to speak.
///
/// Pinned by `document_is_a_fixed_point` in `tests/fuzz.rs`.
fn folding_quotes_governs(meta: &StrMeta, options: &RenderOptions) -> bool {
    matches!(options.multiline_style, MultilineStyle::FoldingQuotes)
        && meta.has_eol
        && meta.eol_type.is_some()
}

fn string_rendering(
    value: &str,
    form: Option<StringForm>,
    meta: &StrMeta,
    options: &RenderOptions,
) -> StringRendering {
    // Honored forms first, each subject to the same safety check the renderer
    // applies before trusting one.
    match resolve_string_form(form, options) {
        // `Quoted` says the string was written in quotes. It does not say whether
        // the newlines inside it were folded, because fold state is deliberately not
        // recorded -- "Folded JSON strings record as `Quoted`". `foldingQuotes`
        // produces quoted output too, so honoring the form here would answer a
        // question the record does not carry, and the fold would vanish on every
        // rerender. Where the style governs, it decides; the honored form is still
        // satisfied, since what it asks for is quotes and that is what it gets.
        Some(StringForm::Quoted) if !folding_quotes_governs(meta, options) => {
            return StringRendering::Quoted;
        }
        Some(StringForm::Bare(bare)) if meta.is_bare_eligible => {
            return StringRendering::Bare(bare_opener_for(Some(bare), options));
        }
        Some(StringForm::Multiline(flavor))
            if meta.has_eol && meta.eol_type.is_some() && !meta.has_forbidden_literal =>
        {
            return StringRendering::HonoredMultiline(flavor);
        }
        _ => {}
    }
    if folding_quotes_governs(meta, options) {
        return StringRendering::FoldingQuotes;
    }
    if options.multiline_strings
        && !meta.has_forbidden_literal
        && meta.has_eol
        && let Some(local_eol) = meta.eol_type
    {
        let eols = match local_eol {
            MultilineLocalEol::Lf => value.split('\n').count(),
            MultilineLocalEol::CrLf => value.split("\r\n").count(),
        }
        .saturating_sub(1);
        if eols >= options.multiline_min_lines.max(1) {
            return StringRendering::StyleMultiline;
        }
    }
    if options.bare_strings != StringStyle::Quoted && meta.is_bare_eligible {
        return StringRendering::Bare(bare_opener_for(None, options));
    }
    StringRendering::Quoted
}

/// May this value be placed on a `/ ` continuation line of its own?
///
/// Named for the question rather than for a fold style, because two of the arms
/// have no style behind them: folding quotes consults none -- that form *is* the
/// folding -- and a bool cannot fold at all yet may still be placed below a key
/// that folded. Returning a `FoldStyle` here meant handing back `Auto` for both,
/// a style nobody set, and the sole caller only ever compared it to `None`.
///
/// `place_value` used to be *told* this, by a `bool` computed at the key's site
/// from the **key's** fold style -- an anonymous parameter that could not look
/// wrong, and did not, until a number was rendered after a long bare key.
fn may_take_a_continuation(bv: BasicValue<'_>, options: &RenderOptions) -> bool {
    let governing = match bv {
        BasicValue::Number(_) => options.number_fold_style,
        BasicValue::String(value, form) => {
            let meta = StrMeta::new(value);
            match string_rendering(value, form, &meta, options) {
                StringRendering::Bare(_) => options.string_bare_fold_style,
                StringRendering::Quoted => options.string_quoted_fold_style,
                StringRendering::HonoredMultiline(_) | StringRendering::StyleMultiline => {
                    options.string_multiline_fold_style
                }
                // The form is the folding; it continues whatever any style says.
                StringRendering::FoldingQuotes => return true,
            }
        }
        // Null, booleans and the empty containers cannot fold -- there is nowhere
        // in `true` to break. But *placing* one on a `/ ` line is a different
        // question from folding it, and the answer is yes when folding is in use
        // at all: after a folded key, moving a four-character value below is what
        // keeps the line inside the margin. Answering `None` here because a bool
        // cannot fold is the same conflation that put the key's style in charge of
        // the value's placement.
        // Null, booleans and the empty containers cannot fold, but *placing* one
        // is a different question from folding it -- see below.
        _ => {
            let any = [
                options.number_fold_style,
                options.string_bare_fold_style,
                options.string_quoted_fold_style,
                options.string_multiline_fold_style,
            ]
            .into_iter()
            .any(|style| style != FoldStyle::None);
            return any;
        }
    };
    governing != FoldStyle::None
}

fn basic_value<T: Tree>(value: &T) -> Option<BasicValue<'_>> {
    match value.node() {
        NodeRef::Null => Some(BasicValue::Null),
        NodeRef::Bool(b) => Some(BasicValue::Bool(b)),
        NodeRef::Number(n) => Some(BasicValue::Number(n)),
        NodeRef::String(s) => Some(BasicValue::String(s, value.string_form())),
        NodeRef::Array([]) => Some(BasicValue::EmptyArray),
        NodeRef::Object([]) => Some(BasicValue::EmptyObject),
        NodeRef::Array(_) | NodeRef::Object(_) => None,
    }
}

// ---- Fact-vs-policy resolution chokepoints ----
//
// Every place the renderer consults a recorded fact goes through one of these, so a
// future per-node policy layer (if it ever earns its existence) has exactly one home
// per decision instead of a scatter of call sites.

fn resolve_string_form(form: Option<StringForm>, options: &RenderOptions) -> Option<StringForm> {
    if options.honor_string_forms { form } else { None }
}

/// The glyph a bare string's opening quote is written with. The only place in
/// the renderer that names either character.
///
/// Both are one column, and that is the whole contract: the opening quote is a
/// space, and `_` is an overlay drawn on that space so a reader can see it. It
/// cannot move the text it opens, so no width, fold point, column, or packing
/// decision may ever consult it -- the sole thing the form decides is which of
/// these two characters gets written. Anything upstream that measures a bare
/// string measures it with a one-column opener and never asks which one.
fn bare_opener_glyph(form: BareForm) -> char {
    match form {
        BareForm::Marked => '_',
        BareForm::Plain => ' ',
    }
}

/// Which opener a bare string wears here.
///
/// A recorded form wins when forms are being honored, because which opener a
/// person wrote is a choice they made and not a fact about the data -- and a
/// `Document` exists to hold what a person wrote, inconsistencies included.
/// Consistency is a promise the generator makes about the openers it invents,
/// not one it enforces over openers it was given: with no recorded form, and so
/// for every string reached from a `Value` or from JSON, the global style
/// decides alone and decides the same way every time.
fn bare_opener_for(form: Option<BareForm>, options: &RenderOptions) -> BareForm {
    match form {
        Some(recorded) => recorded,
        None if options.bare_strings == StringStyle::Marked => BareForm::Marked,
        None => BareForm::Plain,
    }
}

/// A bare string with its opening quote in front, built by hand.
///
/// `format!` is measurably the wrong tool here: this runs once per bare string,
/// and a 46 MB document is millions of them. A `push` and a `push_str` into a
/// right-sized buffer do no formatting work at all, where the macro dispatches
/// through `Display` for each piece.
fn opened_bare(form: BareForm, value: &str) -> String {
    let opener = bare_opener_glyph(form);
    let mut out = String::with_capacity(opener.len_utf8() + value.len());
    out.push(opener);
    out.push_str(value);
    out
}

fn resolve_key_form(form: Option<KeyForm>, options: &RenderOptions) -> Option<KeyForm> {
    if options.honor_key_forms { form } else { None }
}

/// `Some(forced)` when a table should be attempted (`forced` bypasses the size and
/// similarity heuristics, never the physical checks); `None` when tables are off the
/// table for this array.
fn resolve_table_attempt(opinion: Option<bool>, options: &RenderOptions) -> Option<bool> {
    let opinion = if options.honor_tables { opinion } else { None };
    match opinion {
        Some(false) => None,
        Some(true) => Some(true),
        None if options.tables => Some(false),
        None => None,
    }
}

/// Emit comment lines. `Left` comments pin to column 0; `AtLevel` comments sit at the
/// subject's indent. No-op when comment rendering is off.
fn emit_comments(
    comments: &[Comment],
    subject_indent: FileIndent,
    options: &RenderOptions,
    out: &mut Vec<String>,
) {
    if !options.render_comments {
        return;
    }
    for comment in comments {
        match comment.placement() {
            Placement::Left => out.push(comment.text().to_owned()),
            Placement::AtLevel => out.push(format!("{}{}", subject_indent.spaces(), comment.text())),
        }
    }
}

/// Render a key honoring its recorded form when policy allows and the form is safe;
/// the global `bare_keys` policy decides otherwise.
pub(crate) fn render_key_form(key: &str, form: Option<KeyForm>, options: &RenderOptions) -> String {
    match resolve_key_form(form, options) {
        Some(KeyForm::Bare)
            if parse_bare_key_prefix(key, &SPEC_FORMS).is_some_and(|end| end == key.len()) =>
        {
            key.to_owned()
        }
        Some(KeyForm::Quoted) => render_json_string(key),
        _ => render_key(key, options),
    }
}

fn effective_inline_objects(options: &RenderOptions) -> bool {
    options.inline_objects
}

fn effective_inline_arrays(options: &RenderOptions) -> bool {
    options.inline_arrays
}

fn effective_force_markers(options: &RenderOptions) -> bool {
    options.force_markers
}

// Returns the target parent_indent to re-render the table at when /< /> glyphs should be
// used, or None if no unindenting should occur.
//
// `natural_lines` are the table lines as rendered at pair_indent, one level in.
fn table_unindent_target(
    pair_indent: FileIndent,
    natural_lines: &[String],
    options: &RenderOptions,
) -> Option<FileIndent> {
    let n = pair_indent;
    // Measured in columns, not bytes. `l.len()` stood here, and every comparison
    // below weighs this against `wrap_width`, which is a column budget: a table of
    // CJK measured about three times its real width, so it was judged not to fit
    // at indent 0 and kept its indent, while the identical table in Latin text was
    // pushed left. That is layout moving with the encoding of the content.
    let max_natural =
        natural_lines.iter().map(|l| Columns::of(l)).max().unwrap_or(Columns::ZERO);
    // data_width: widest line with the natural indent stripped
    let data_width = max_natural.saturating_sub(n.width() + Columns::new(LEVEL));

    match options.table_unindent_style {
        TableUnindentStyle::None => None,

        TableUnindentStyle::Left => {
            // Always push to indent 0, unless already there.
            if n == FileIndent::ROOT { None } else {
                // Check it fits at 0 (data_width <= w, or unlimited width).
                let fits =
                    options.wrap_width.map(|w| data_width <= Columns::new(w)).unwrap_or(true);
                if fits { Some(FileIndent::ROOT) } else { None }
            }
        }

        TableUnindentStyle::Auto => {
            // Push to indent 0 only when table overflows at natural indent.
            // With unlimited width, never unindent.
            let w = Columns::new(options.wrap_width?);
            let overflows_natural = max_natural > w;
            let fits_at_zero = data_width <= w;
            if overflows_natural && fits_at_zero { Some(FileIndent::ROOT) } else { None }
        }

        TableUnindentStyle::Floating => {
            // Push left by the minimum amount needed to fit within wrap_width.
            // With unlimited width, never unindent.
            let w = Columns::new(options.wrap_width?);
            if max_natural <= w {
                return None; // already fits, no need to move
            }
            // Find the minimum parent_indent such that data_width + (parent_indent.deeper(1)) <= w.
            // data_width is fixed; we need parent_indent.deeper(1) + data_width <= w.
            // minimum parent_indent = 0 if data_width + 2 <= w, else can't help.
            if data_width + Columns::new(2) <= w {
                // Find smallest parent_indent that makes table fit.
                let target = FileIndent::new(
                    w.saturating_sub(data_width + Columns::new(LEVEL)).columns(),
                );
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
fn subtree_line_count<T: Tree>(value: &T) -> usize {
    match value.node() {
        NodeRef::Array(v) if !v.is_empty() => v.iter().map(subtree_line_count).sum::<usize>() + 1,
        NodeRef::Object(e) if !e.is_empty() => {
            e.iter().map(|entry| subtree_line_count(T::entry_value(entry)) + 1).sum()
        }
        _ => 1,
    }
}

/// Rough count of content bytes in a subtree. Used to weight volume in `ByteWeighted` mode.
fn subtree_byte_count<T: Tree>(value: &T) -> usize {
    match value.node() {
        NodeRef::String(s) => s.len(),
        NodeRef::Number(n) => n.to_string().len(),
        NodeRef::Bool(b) => if b { 4 } else { 5 },
        NodeRef::Null => 4,
        NodeRef::Array(v) => v.iter().map(subtree_byte_count).sum(),
        NodeRef::Object(e) => e
            .iter()
            .map(|entry| T::entry_key(entry).len() + subtree_byte_count(T::entry_value(entry)))
            .sum(),
    }
}

/// Maximum nesting depth of non-empty containers below this value.
/// Empty arrays/objects count as 0 (simple values).
fn subtree_max_depth<T: Tree>(value: &T) -> usize {
    match value.node() {
        NodeRef::Array(v) if !v.is_empty() => {
            1 + v.iter().map(subtree_max_depth).max().unwrap_or(0)
        }
        NodeRef::Object(e) if !e.is_empty() => {
            1 + e
                .iter()
                .map(|entry| subtree_max_depth(T::entry_value(entry)))
                .max()
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// Returns true if a `/<` indent-offset glyph should be emitted for `value` at `pair_indent`.
fn should_use_indent_glyph<T: Tree>(value: &T, pair_indent: FileIndent, options: &RenderOptions) -> bool {
    let Some(w) = options.wrap_width else { return false; };
    let fold_floor = || {
        let max_depth = subtree_max_depth(value);
        pair_indent.deeper(max_depth).width().columns()
            >= w.saturating_sub(MIN_FOLD_CONTINUATION + 2)
    };
    match indent_glyph_mode(options) {
        IndentGlyphMode::None => false,
        IndentGlyphMode::Fixed => pair_indent.width().columns() >= w / 2,
        IndentGlyphMode::IndentWeighted(threshold) => {
            if fold_floor() { return true; }
            let line_count = subtree_line_count(value);
            (pair_indent.width().columns() * line_count) as f64 >= threshold * (w * w) as f64
        }
        IndentGlyphMode::ByteWeighted(threshold) => {
            if fold_floor() { return true; }
            let byte_count = subtree_byte_count(value);
            (pair_indent.width().columns() * byte_count) as f64 >= threshold * (w * w) as f64
        }
    }
}

/// How a value sits relative to its key's line.
///
/// Every key-value layout the renderer can produce is one of these three, and which
/// one it is is a decision the layout makes -- never something recovered afterwards
/// by inspecting the rendered text. This replaced exactly that: the folded-key path
/// used to re-render the pair with the key *unfolded*, strip `key:` back off the
/// first line, and guess from the remainder. A feature that gave a value a new kind
/// of first line could then break a caller that never mentioned it, which is how
/// `key: /<` reached a branch whose comment said it was unreachable.
///
/// Every space in these strings belongs to the value: one column for a bare
/// string's opening quote, one for the glyph's spec-mandated leading space, two for
/// an inline array's start -- the same two it would occupy as indentation on a line
/// of its own. The pair contributes no separator of its own, so there is no gap to
/// carry separately.
enum ValuePlacement {
    /// Entirely on the key's line: `k: 42`, `k:[]`, `k:  1, 2, 3`.
    OnKeyLine { after_colon: String },
    /// Opens on the key's line and continues below: `k: /<` … ` />`, a multiline
    /// string's glyph and body, or a folded string's first segment and its `/ `
    /// continuations.
    OpensOnKeyLine { opener: String, below: Vec<String> },
    /// Nothing after the colon; the value's lines begin underneath.
    Below { lines: Vec<String> },
}

/// The configured wrap width as a budget.
///
/// The single crossing out of the options bag, which holds the width as a plain
/// number because it is configuration and public. Everything downstream of here
/// carries [`Columns`], so no fold path has to remember which unit it was handed.
fn wrap_budget(options: &RenderOptions) -> Option<Columns> {
    options.wrap_width.map(Columns::new)
}

fn fits_wrap(options: &RenderOptions, line: &str) -> bool {
    match options.wrap_width {
        Some(0) | None => true,
        Some(width) => Columns::of(line) <= Columns::new(width),
    }
}

/// How much a string is allowed to share a line with, under the current style.
///
/// The three splitting styles form a ladder, and every rung takes one more thing
/// away from strings while saying nothing at all about scalars.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StringPacking {
    /// `prefer-spaces`: a string packs with whatever its run allows.
    Free,
    /// `spaces`: a string is never comma packed, so an unbareable one stands
    /// alone, but bare-able ones still space pack with each other.
    NoComma,
    /// `none`: no string shares a line with anything.
    Never,
}


pub(crate) fn render_key(key: &str, options: &RenderOptions) -> String {
    if options.bare_keys == BareStyle::Prefer
        && parse_bare_key_prefix(key, &SPEC_FORMS).is_some_and(|end| end == key.len())
    {
        key.to_owned()
    } else {
        render_json_string(key)
    }
}


pub(crate) fn needs_explicit_array_marker<T: Tree>(value: &T) -> bool {
    matches!(value.node(), NodeRef::Array(values) if !values.is_empty())
        || matches!(value.node(), NodeRef::Object(entries) if !entries.is_empty())
}


fn split_multiline_fold(text: &str, avail: Columns, style: FoldStyle) -> Vec<&str> {
    if Columns::of(text) <= avail || avail == Columns::ZERO {
        return vec![text];
    }
    let mut segments = Vec::new();
    let mut rest = text;
    loop {
        if Columns::of(rest) <= avail {
            segments.push(rest);
            break;
        }
        let split_at = match style {
            FoldStyle::Auto => {
                // Find the last space before avail that is not a single consecutive space
                // (spec: bare strings may not fold immediately after a single space, but
                // multiline folds are within the body text so we just prefer spaces).
                let cut = floor_safe_fold_point(rest, avail);
                let candidate = &rest[..cut];
                // Find last space boundary
                if let Some(pos) = candidate.rfind(' ') {
                    if pos > 0 { pos } else { cut }
                } else {
                    cut
                }
            }
            FoldStyle::Fixed | FoldStyle::None => floor_safe_fold_point(rest, avail),
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
/// The shape every fold shares: one first line, then continuations behind `/ `.
///
/// Bare strings, bare keys, numbers and JSON strings all walk the same loop —
/// take a budget, choose a split, emit a segment, drop to the continuation
/// budget, repeat. Only three things actually differ between them, and each is
/// a hook:
///
/// * `avail_for` — the budget for the segment about to be cut. Only JSON
///   strings use it, to reserve a column for their closing quote on the final
///   segment; everything else hands the budget straight back.
/// * `choose_split` — where to cut, given the remaining text, the budget, and
///   whether this is the first line. Returning 0 means "nowhere good", and the
///   remainder is emitted whole.
/// * `emit` — how to render one segment, given whether it is first and whether
///   it is last.
///
/// The loop lives here rather than in each caller because getting it right is
/// fiddly and getting it slightly wrong is invisible: an off-by-one in the
/// continuation budget, or a first/last flag read at the wrong moment, still
/// produces output that parses and round trips. One copy can be checked once.
fn fold_lines(
    value: &str,
    first_avail: Columns,
    cont_avail: Columns,
    mut avail_for: impl FnMut(&str, Columns) -> Columns,
    mut choose_split: impl FnMut(&str, Columns, bool) -> usize,
    mut emit: impl FnMut(&str, bool, bool) -> String,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut rest = value;
    let mut first = true;
    let mut current = first_avail;

    loop {
        let avail = avail_for(rest, current);
        // Whether this is the final segment is settled before emitting it, so
        // a caller that needs to close something (a quote) knows in time.
        // A caller says where it *wants* to cut; whether a cut is *allowed*
        // there is not its business. Spec: folding in the middle of a data
        // character or a visible character is forbidden in every context, so
        // every proposed split is backed off to the nearest legal one here,
        // once, rather than each fold path re-deriving the rule.
        let split = if Columns::of(rest) <= avail {
            0
        } else {
            floor_legal_split(rest, choose_split(rest, avail, first))
        };

        if split == 0 {
            lines.push(emit(rest, first, true));
            break;
        }

        lines.push(emit(&rest[..split], first, false));
        rest = &rest[split..];
        first = false;
        current = cont_avail;
    }

    lines
}

fn fold_bare_string(
    value: &str,
    indent: FileIndent,
    first_line_extra: Columns,
    opener_form: BareForm,
    style: FoldStyle,
    wrap_width: Option<Columns>,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    // First-line budget: indent + 1 (the one-sided opening quote) + first_line_extra.
    // The 1 is a constant and stays one: `opener_form` reaches the emitter below
    // and nothing else, because which glyph opens the string cannot move it. See
    // `bare_opener_glyph`.
    let first_avail = w.saturating_sub(indent.width() + Columns::new(1) + first_line_extra);
    if Columns::of(value) <= first_avail {
        return None; // fits on one line, no fold needed
    }
    let cont_avail = w.saturating_sub(indent.width() + Columns::new(Marker::Fold.width()));
    if cont_avail < Columns::new(MIN_FOLD_CONTINUATION) {
        return None; // too little room for useful continuation content
    }
    let ind = indent.spaces();

    let lines = fold_lines(
        value,
        first_avail,
        cont_avail,
        |_rest, current| current,
        |rest, avail, first| {
            match style {
                // Spec: "a bare string may never be folded immediately after a
                // single consecutive space", which is what the lookahead is for.
                FoldStyle::Auto => {
                    let candidate = &rest[..floor_safe_fold_point(rest, avail)];
                    let lookahead = rest[candidate.len()..].chars().next();
                    let at = find_bare_fold_point(candidate, lookahead);
                    // On a continuation with no good boundary, a hard cut still
                    // beats overflowing the margin.
                    if at == 0 && !first {
                        floor_safe_fold_point(rest, avail)
                    } else {
                        at
                    }
                }
                FoldStyle::Fixed | FoldStyle::None => floor_safe_fold_point(rest, avail),
            }
        },
        |segment, first, _last| {
            if first {
                format!("{}{}{}", ind, bare_opener_glyph(opener_form), segment)
            } else {
                format!("{ind}{}{segment}", Marker::Fold.text())
            }
        },
    );

    if lines.len() <= 1 { None } else { Some(lines) }
}

/// What a key's fold carries on its final line. Passed to the folder rather than
/// appended after it: a folder that budgets for a suffix it does not write is
/// trusting its caller to append exactly that much, and nothing holds the two
/// together. Handing over the text makes the reservation and the writing one fact.
const KEY_COLON: &str = ":";

/// Fold a bare key (no leading space) into continuation lines, the last of which
/// carries `tail`.
///
/// `None` when no fold is needed, none is possible, or the style forbids one.
fn fold_bare_key(
    key: &str,
    pair_indent: FileIndent,
    style: FoldStyle,
    wrap_width: Option<Columns>,
    tail: &str,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    if matches!(style, FoldStyle::None) {
        return None;
    }
    // Key plus whatever the caller appends fits -- no fold needed.
    //
    // Stated as the tail's own width rather than as a strict `<`. The `<` reserved
    // exactly one column, which is right for `:` and right for nothing else, so
    // the guard held only as long as no caller passed a longer tail.
    if Columns::of(key) + Columns::of(tail) <= w.saturating_sub(pair_indent.width()) {
        return None;
    }
    let first_avail = w.saturating_sub(pair_indent.width());
    let cont_avail = w.saturating_sub(pair_indent.width() + Columns::new(Marker::Fold.width()));
    if cont_avail < Columns::new(MIN_FOLD_CONTINUATION) {
        return None;
    }
    let ind = pair_indent.spaces();

    let lines = fold_lines(
        key,
        first_avail,
        cont_avail,
        // Whichever segment ends up last carries whatever the caller appends, so its
        // budget is `tail_reserve` columns shorter than the others. Charged here
        // rather than to every line, which would fold earlier than necessary.
        |rest, current| {
            if Columns::of(rest) <= current {
                current.saturating_sub(Columns::of(tail))
            } else {
                current
            }
        },
        |rest, avail, _first| match style {
            FoldStyle::Auto => {
                let candidate = &rest[..floor_safe_fold_point(rest, avail)];
                let lookahead = rest[candidate.len()..].chars().next();
                find_bare_fold_point(candidate, lookahead)
            }
            FoldStyle::Fixed | FoldStyle::None => floor_safe_fold_point(rest, avail),
        },
        |segment, first, last| {
            let tail = if last { tail } else { "" };
            if first {
                format!("{}{}{}", ind, segment, tail)
            } else {
                format!("{ind}{}{segment}{tail}", Marker::Fold.text())
            }
        },
    );

    if lines.len() <= 1 { None } else { Some(lines) }
}

/// Fold a number across continuation lines.
///
/// `None` when it fits, when the style forbids folding, or when a continuation
/// would have too little room to be worth one.
fn fold_number(
    value: &str,
    indent: FileIndent,
    first_line_extra: Columns,
    style: FoldStyle,
    wrap_width: Option<Columns>,
) -> Option<Vec<String>> {
    if matches!(style, FoldStyle::None) {
        return None;
    }
    let w = wrap_width?;
    let first_avail = w.saturating_sub(indent.width() + first_line_extra);
    if Columns::of(value) <= first_avail {
        return None; // fits on one line
    }
    let cont_avail = w.saturating_sub(indent.width() + Columns::new(Marker::Fold.width()));
    if cont_avail < Columns::new(MIN_FOLD_CONTINUATION) {
        return None;
    }
    let auto_mode = matches!(style, FoldStyle::Auto);
    let ind = indent.spaces();

    Some(fold_lines(
        value,
        first_avail,
        cont_avail,
        |_rest, current| current,
        |rest, avail, _first| find_number_fold_point(rest, avail, auto_mode),
        |segment, first, _last| {
            if first {
                format!("{}{}", ind, segment)
            } else {
                format!("{ind}{}{segment}", Marker::Fold.text())
            }
        },
    ))
}

/// Fold a JSON string across continuation lines, quotes included.
///
/// The delimiters are stripped before folding and put back by the emitter, which
/// is the only part that knows about them -- so the budget arithmetic never has
/// to remember whether the quotes are in the text it is measuring.
fn fold_json_string(
    value: &str,
    indent: FileIndent,
    first_line_extra: Columns,
    style: FoldStyle,
    wrap_width: Option<Columns>,
    tail: &str,
) -> Option<Vec<String>> {
    let w = wrap_width?;
    let encoded = render_json_string(value);
    let first_avail = w.saturating_sub(indent.width() + first_line_extra);
    // `tail` counts here, not just inside the fold loop below. This asked whether
    // the value fits and answered for text one character shorter than the line
    // that gets written -- so a key exactly filling the margin reported "no fold
    // needed", and the caller's `:` then pushed the line one column over. The
    // whole reason the tail is handed to this function rather than appended after
    // it is that the reservation and the writing should be one fact; the early
    // return was the one place that still separated them.
    if Columns::of(&encoded) + Columns::of(tail) <= first_avail {
        return None; // fits on one line, tail included
    }
    let cont_avail = w.saturating_sub(indent.width() + Columns::new(Marker::Fold.width()));
    if cont_avail < Columns::new(MIN_FOLD_CONTINUATION) {
        return None;
    }
    // Work on the content between the delimiters; the quotes are put back by
    // the emitter, which is the only part that knows about them.
    let inner = &encoded[1..encoded.len() - 1];
    let ind = indent.spaces();

    let lines = fold_lines(
        inner,
        first_avail.saturating_sub(Columns::new(1)), // the opening `"` costs a column
        cont_avail,
        // The final segment has to leave room for the closing `"`, plus whatever the
        // caller appends after it -- a key's colon, nothing for a plain value.
        |rest, current| {
            if Columns::of(rest) <= current {
                current.saturating_sub(Columns::new(1) + Columns::of(tail))
            } else {
                current
            }
        },
        |rest, avail, _first| match style {
            // Spec: fold BEFORE unescaped space runs.
            FoldStyle::Auto => {
                let candidate = &rest[..floor_safe_fold_point(rest, avail)];
                find_json_fold_point(candidate)
            }
            FoldStyle::Fixed | FoldStyle::None => {
                safe_json_split(rest, floor_safe_fold_point(rest, avail))
            }
        },
        |segment, first, last| match (first, last) {
            (true, true) => format!("{}\"{}\"{}", ind, segment, tail),
            (true, false) => format!("{}\"{}", ind, segment),
            (false, true) => format!("{ind}{}{segment}\"{tail}", Marker::Fold.text()),
            (false, false) => format!("{ind}{}{segment}", Marker::Fold.text()),
        },
    );

    if lines.len() <= 1 { None } else { Some(lines) }
}

/// Count consecutive backslashes immediately before `pos` in `bytes`.
fn render_folding_quotes(value: &str, indent: FileIndent, options: &RenderOptions) -> Vec<String> {
    let ind = indent.spaces();
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
            if inner.is_empty() {
                // A trailing empty piece (the value ends with the EOL) would emit
                // `/ "` -- a fold marker with no data character after it, which the
                // spec forbids: "Fold indicators must be both preceded and followed
                // by at least one data character." Close on the previous line instead.
                lines.last_mut().expect("first piece always pushes a line").push('"');
            } else {
                lines.push(format!("{ind}{}{inner}\"", Marker::Fold.text()));
            }
        } else {
            lines.push(format!("{ind}{}{inner}{nl}", Marker::Fold.text()));
        }
        // Width-fold within this piece if the line is still too wide
        // and string_multiline_fold_style is not None.
        if !matches!(options.string_multiline_fold_style, FoldStyle::None)
            && let Some(w) = options.wrap_width {
                let last = lines.last().unwrap();
                if Columns::of(last) > Columns::new(w) {
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
// Clone/Copy by hand: the derive would demand `T: Clone`/`T: Copy`, but the variants
// only ever hold borrows of `T`, which are Copy for any `T`.
enum PackedToken<'a, T: Tree> {
    /// A flat inline token (null, bool, number, short string, empty array/object).
    /// Rendered on demand from the BasicValue.
    Inline(BasicValue<'a>),
    /// A block element (multiline string, nonempty array, nonempty object) that interrupts
    /// packing. Borrows the original value; rendered lazily at the right continuation indent.
    Block(&'a T),
}

impl<'a, T: Tree> Clone for PackedToken<'a, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, T: Tree> Copy for PackedToken<'a, T> {}

/// Zero-sized generic renderer: every `Self::` call carries the tree type, so the
/// walking code reads exactly as it did when it was `Value`-only.
pub(crate) struct Renderer<T: Tree>(PhantomData<T>);

impl<T: Tree> Renderer<T> {
    pub(crate) fn render(value: &T, options: &RenderOptions) -> String {
        let mut lines = Vec::new();
        emit_comments(value.comments_before(), FileIndent::new(options.start_indent), options, &mut lines);
        lines.extend(Self::render_root(value, options, FileIndent::new(options.start_indent)));
        emit_comments(value.trailing_comments(), FileIndent::ROOT, options, &mut lines);
        lines.join(options.eol.as_str())
    }

    fn render_root(
        value: &T,
        options: &RenderOptions,
        start_indent: FileIndent,
    ) -> Vec<String> {
        match value.node() {
            NodeRef::Null => Self::render_scalar_lines(BasicValue::Null, start_indent, Columns::ZERO, options),
            NodeRef::Bool(b) => Self::render_scalar_lines(BasicValue::Bool(b), start_indent, Columns::ZERO, options),
            NodeRef::Number(n) => Self::render_scalar_lines(BasicValue::Number(n), start_indent, Columns::ZERO, options),
            NodeRef::String(s) => {
                Self::render_scalar_lines(BasicValue::String(s, value.string_form()), start_indent, Columns::ZERO, options)
            }
            NodeRef::Array([]) => {
                Self::render_scalar_lines(BasicValue::EmptyArray, start_indent, Columns::ZERO, options)
            }
            NodeRef::Object([]) => {
                Self::render_scalar_lines(BasicValue::EmptyObject, start_indent, Columns::ZERO, options)
            }
            NodeRef::Array(values) if effective_force_markers(options) => {
                Self::render_explicit_array(values, start_indent, value.table_opinion(), options)
            }
            NodeRef::Array(values) => {
                Self::render_implicit_array(values, start_indent, value.table_opinion(), options)
            }
            NodeRef::Object(entries) if effective_force_markers(options) => {
                Self::render_explicit_object(entries, start_indent, options)
            }
            NodeRef::Object(entries) => {
                Self::render_implicit_object(entries, start_indent, options)
            }
        }
    }

    fn render_implicit_object(
        entries: &[T::Entry],
        parent_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        let pair_indent = parent_indent.deeper(1);
        let mut lines = Vec::new();
        let mut packed_line = String::new();

        for entry in entries {
            let key = T::entry_key(entry);
            let key_form = T::entry_key_form(entry);
            let value = T::entry_value(entry);
            // A commented entry starts a fresh line: the comment was on its own line in
            // the source by definition, so a break existed there. Entries after it may
            // resume packing onto the commented entry's line.
            let entry_comments = T::entry_comments(entry);
            if options.render_comments && !entry_comments.is_empty() {
                if !packed_line.is_empty() {
                    lines.push(std::mem::take(&mut packed_line));
                }
                emit_comments(entry_comments, pair_indent, options, &mut lines);
            }
            if effective_inline_objects(options)
                && let Some(token) = Self::render_inline_object_token(key, key_form, value, options) {
                    let candidate = if packed_line.is_empty() {
                        format!("{}{}", pair_indent.spaces(), token)
                    } else {
                        format!("{packed_line}{}{token}", Spaces::new(options.kv_pack_multiple * LEVEL))
                    };
                    if fits_wrap(options, &candidate) {
                        // Inlining takes away the value's own line, so its comments
                        // move to the line the pair lands on -- above it, after the
                        // key's, since the key is to its left. Emitted only once the
                        // inline is committed: on the fall-through below the value
                        // still gets a line and `render_object_entry` emits them
                        // against it instead.
                        if options.render_comments {
                            emit_comments(value.comments_before(), pair_indent.deeper(1), options, &mut lines);
                        }
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
            lines.extend(Self::render_object_entry(key, key_form, value, pair_indent, options));
        }

        if !packed_line.is_empty() {
            lines.push(packed_line);
        }
        lines
    }

    /// Render one key-value pair at `pair_indent`.
    ///
    /// Owns the key's line -- folded or not -- and asks [`Self::place_value`] what the
    /// value does with it. The folded and unfolded cases differ only in what the key
    /// line holds and which column the value would start at; the composition below is
    /// the same for both, which is what keeps a folded key from being a second layout
    /// engine with its own opinions.
    fn render_object_entry(
        key: &str,
        key_form: Option<KeyForm>,
        value: &T,
        pair_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        let key_text = render_key_form(key, key_form, options);
        // Whether the key rendered bare decides which fold machinery applies; judging
        // the *result* keeps honored forms and global policy on one code path.
        let is_bare = !key_text.starts_with('"');

        // Key fold lines -- the last one gets ":" appended before the value.
        // Bare keys use string_bare_fold_style; quoted keys use string_quoted_fold_style.
        // Only the first (standalone) key on a line is ever folded; inline-packed keys
        // are not candidates (they are rendered via render_inline_object_token, not here).
        //
        // `None` here means "no fold needed" as much as it means "folding is off" --
        // `fold_bare_key` returns it when the key already fits, the same way
        // `fold_bare_string` does. So the ordinary unfolded pair is the `None` arm.
        let key_fold: Option<Vec<String>> =
            if is_bare && options.string_bare_fold_style != FoldStyle::None {
                fold_bare_key(&key_text, pair_indent, options.string_bare_fold_style, wrap_budget(options), KEY_COLON)
            } else if !is_bare && options.string_quoted_fold_style != FoldStyle::None {
                fold_json_string(
                    key,
                    pair_indent,
                    Columns::ZERO,
                    options.string_quoted_fold_style,
                    wrap_budget(options),
                    KEY_COLON,
                )
            } else {
                None
            };

        // The key's line through its colon, and the column the value would start at.
        // For a folded key that is the last continuation line rather than the first
        // line -- which is the whole point: every width decision below is then made
        // against the line that will actually be emitted, not against a hypothetical
        // unfolded one.
        let key_folded = key_fold.is_some();
        let (mut lines, key_line) = match key_fold {
            None => (Vec::new(), format!("{}{}:", pair_indent.spaces(), key_text)),
            Some(mut fold_lines) => {
                // The folder already put the colon on this line, and budgeted for it.
                let last = fold_lines.pop().expect("a key fold always yields at least one line");
                (fold_lines, last)
            }
        };
        let value_column = Columns::of(&key_line);

        // Hoisting above "the key's line" means above the *whole* key. A folded key
        // has already put its earlier continuation lines into `lines`, and a comment
        // may never sit inside a fold -- so these go in front of all of it. Unfolded,
        // `lines` is empty and this is an ordinary append.
        //
        // Landing next to the key's own comments does not merge with them: those sit
        // at the key's column and these at `n+2`, and the column is what says which
        // one a comment belongs to.
        let hoist_above_key = |lines: &mut Vec<String>| {
            let mut hoisted = Vec::new();
            emit_comments(value.comments_before(), pair_indent.deeper(1), options, &mut hoisted);
            lines.splice(0..0, hoisted);
        };

        match Self::place_value(value, pair_indent, value_column, key_folded, options) {
            // A key and its value are two things, so each carries its own comments,
            // and every arm here has to say where the value's go -- all four of them.
            // Only `Below` leaves the value a line of its own to sit above. The rest
            // put it on the key's line, so its comments hoist above that line.
            //
            // They are emitted one level in from the key either way, because a
            // comment's indent names the depth it refers to and a value is always
            // `+2` from its key. At the key's own level they would re-read as the
            // key's on the next parse.
            ValuePlacement::OnKeyLine { after_colon } => {
                hoist_above_key(&mut lines);
                lines.push(key_line + &after_colon);
            }
            ValuePlacement::OpensOnKeyLine { opener, below } => {
                hoist_above_key(&mut lines);
                lines.push(key_line + &opener);
                lines.extend(below);
            }
            ValuePlacement::Below { lines: below } => {
                lines.push(key_line);
                // Here the value does get its own line, so its comments sit between
                // the key and the value rather than above the pair.
                emit_comments(value.comments_before(), pair_indent, options, &mut lines);
                lines.extend(below);
            }
        }
        lines
    }

    /// Render a scalar value's lines for use as fold-after-colon continuation(s).
    /// The first line charges itself `Marker::Fold`'s width, the "/ " prefix it will
    /// carry, so that
    /// content is correctly fitted to `wrap_width - pair_indent - 2 - (leading space if bare)`.
    /// The caller prefixes the first element's content (after stripping `pair_indent`) with "/ ".
    fn render_scalar_value_continuation_lines(
        value: BasicValue<'_>,
        pair_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        match value {
            BasicValue::String(s, form) => Self::render_string_lines(
                s,
                form,
                pair_indent,
                Columns::new(Marker::Fold.width()),
                options,
            ),
            BasicValue::Number(n) => {
                let ns = n.to_string();
                fold_number(
                    &ns,
                    pair_indent,
                    Columns::new(Marker::Fold.width()),
                    options.number_fold_style,
                    wrap_budget(options),
                )
                    .unwrap_or_else(|| vec![format!("{}{}", pair_indent.spaces(), ns)])
            }
            BasicValue::Null => vec![format!("{}null", pair_indent.spaces())],
            BasicValue::Bool(b) => vec![format!("{}{}", pair_indent.spaces(), if b { "true" } else { "false" })],
            BasicValue::EmptyArray => vec![format!("{}[]", pair_indent.spaces())],
            BasicValue::EmptyObject => vec![format!("{}{{}}", pair_indent.spaces())],
        }
    }

    /// Wrap `body` in `/<` ... `/>`, placing the opener where the marker style says.
    ///
    /// `Compact` puts it on the key's line and `Separate` on its own, and that is the
    /// whole of the decision -- it does not consult the width, and it does not care
    /// whether the key folded. A compact opener on a folded key's last continuation
    /// reads fine and reparses, so there is nothing here for the margin to overrule.
    fn place_indent_glyph(
        body: Vec<String>,
        pair_indent: FileIndent,
        value_column: Columns,
        key_folded: bool,
        options: &RenderOptions,
    ) -> ValuePlacement {
        // Settle the arrangement before touching `body`, so it is moved once and
        // never copied. Sharing the separate branch between the two arms as a
        // closure reads better and deep-clones every line of the rendered subtree
        // to do it, once per node.
        let on_key_line = match options.indent_glyph_marker_style {
            // `: /<` is a reading cue, not decoration: the colon says which key is
            // moving and the glyph says its contents are what moved. Worth running
            // past the margin to keep -- but not worth folding a key that had no
            // other reason to fold, because then the cue costs a line break in the
            // key itself, which is the more expensive thing to make a reader follow.
            IndentGlyphMarkerStyle::Compact => {
                let overruns = options
                    .wrap_width
                    .is_some_and(|w| value_column + Columns::of(Glyph::IndentOpen.text()) > Columns::new(w));
                key_folded || !overruns
            }
            IndentGlyphMarkerStyle::Separate => false,
        };

        let closer = Glyph::IndentClose.at(pair_indent.spaces());
        if on_key_line {
            let mut below = body;
            below.push(closer);
            ValuePlacement::OpensOnKeyLine { opener: Glyph::IndentOpen.text().to_owned(), below }
        } else {
            let mut lines = Vec::with_capacity(body.len() + 2);
            lines.push(Glyph::IndentOpen.at(pair_indent.spaces()));
            lines.extend(body);
            lines.push(closer);
            ValuePlacement::Below { lines }
        }
    }

    /// Lay out `value` for a key at `pair_indent`, given the column its first character
    /// would occupy -- that is, the length of the key's line through its colon.
    ///
    /// Renders no key. The caller owns the key's line, because only the caller knows
    /// whether the key folded and where its last continuation ends. Whether an
    /// overflowing scalar may take a `/ ` continuation of its own is asked of the
    /// value here, through [`may_take_a_continuation`].
    fn place_value(
        value: &T,
        pair_indent: FileIndent,
        value_column: Columns,
        key_folded: bool,
        options: &RenderOptions,
    ) -> ValuePlacement {
        match value.node() {
            NodeRef::Array(values) if !values.is_empty() => {
                if let Some(forced) = resolve_table_attempt(value.table_opinion(), options)
                    && let Some(table_lines) = Self::render_table(values, pair_indent, forced, options) {
                        if let Some(target_indent) = table_unindent_target(pair_indent, &table_lines, options) {
                            let Some(offset_lines) = Self::render_table(values, target_indent, forced, options) else {
                                unreachable!("table re-render at offset indent always succeeds");
                            };
                            let mut body = Vec::new();
                            if effective_force_markers(options) {
                                let elem_indent = target_indent.deeper(1);
                                let first = offset_lines.first()
                                    .expect("render_table always returns at least a header line");
                                let stripped = elem_indent.strip(first)
                                    .expect("table line starts at elem_indent");
                                body.push(indent_marked(target_indent.deeper(1), Marker::Array) + stripped);
                                body.extend(offset_lines.into_iter().skip(1));
                            } else {
                                body.extend(offset_lines);
                            }
                            return Self::place_indent_glyph(body, pair_indent, value_column, key_folded, options);
                        }
                        let mut lines = Vec::new();
                        if effective_force_markers(options) {
                            let elem_indent = pair_indent.deeper(1);
                            let first = table_lines.first()
                                .expect("render_table always returns at least a header line");
                            let stripped = elem_indent.strip(first)
                                .expect("table line starts at elem_indent");
                            lines.push(indent_marked(pair_indent.deeper(1), Marker::Array) + stripped);
                            lines.extend(table_lines.into_iter().skip(1));
                        } else {
                            lines.extend(table_lines);
                        }
                        return ValuePlacement::Below { lines };
                    }

                if should_use_indent_glyph(value, pair_indent, options) {
                    let body = if values.first().is_some_and(needs_explicit_array_marker) {
                        Self::render_explicit_array(values, FileIndent::ROOT.deeper(1), value.table_opinion(), options)
                    } else {
                        Self::render_array_children(values, FileIndent::ROOT.deeper(1), options)
                    };
                    return Self::place_indent_glyph(body, pair_indent, value_column, key_folded, options);
                }

                if effective_inline_arrays(options) {
                    let all_simple = values.iter().all(|v| match v.node() {
                        NodeRef::Array(a) => a.is_empty(),
                        NodeRef::Object(o) => o.is_empty(),
                        _ => true,
                    });
                    if all_simple {
                        // Array starter 2/3, inline variant -- but only when the whole
                        // array fits on the key's line. Once it wraps, the first row
                        // starts further right than every row below it, so it holds
                        // fewer elements than the rows under it and no column can line
                        // up. Spec: "it usually looks better to just start on the next
                        // line if it doesn't all fit on one line with the key, so the
                        // default is to do that". Both remain legal to parse.
                        //
                        // The prefix is spaces rather than the key's text because this
                        // function never sees the key. Its *length* is all the packer
                        // needs, and slicing the result back off at `value_column` is
                        // exact rather than a search: we chose that prefix ourselves.
                        // The two extra columns are the array's own start position --
                        // the same two it would occupy as indentation on its own line.
                        // One line *and* within the margin. The line count alone was
                        // standing in for the width, which holds only while the
                        // packer would have wrapped had the elements not fitted --
                        // and a single element has nothing to wrap, so it came back
                        // as one line at any width and was taken as fitting. A
                        // one-element array then sat on the key's line past the
                        // margin while a three-element one correctly went below.
                        if let Some(packed) = Self::render_packed_array_lines(
                            values,
                            (value_column + Columns::new(LEVEL)).spaces().to_string(),
                            pair_indent.deeper(1),
                            options,
                        ) && packed.len() == 1
                            && fits_wrap(options, &packed[0])
                        {
                            let after_colon =
                                packed[0][value_column.spent_in(&packed[0])..].to_owned();
                            return ValuePlacement::OnKeyLine { after_colon };
                        }
                        // Not taken under `force_markers`. The array above fits
                        // on the key's line, so its `  ` opener is the inline
                        // start variant, which the specification exempts from
                        // marking. This one has already given up on that: the
                        // key sits alone and the elements begin at the child
                        // indent, which is an ordinary indent-level start and
                        // so is a level `force_markers` is meant to name. The
                        // marker it falls through to lands where the array
                        // actually begins, which is why this stays honest --
                        // an array that had started after the key could not be
                        // marked on a later line without claiming to start
                        // there.
                        if !effective_force_markers(options)
                            && let Some(packed) = Self::render_packed_array_lines(
                                values,
                                pair_indent.deeper(1).spaces().to_string(),
                                pair_indent.deeper(1),
                                options,
                            )
                        {
                            return ValuePlacement::Below { lines: packed };
                        }
                    }
                }

                let lines = if values.first().is_some_and(needs_explicit_array_marker)
                    || effective_force_markers(options)
                {
                    Self::render_explicit_array(values, pair_indent, value.table_opinion(), options)
                } else {
                    Self::render_array_children(values, pair_indent.deeper(1), options)
                };
                ValuePlacement::Below { lines }
            }
            NodeRef::Object(entries) if !entries.is_empty() => {
                if should_use_indent_glyph(value, pair_indent, options) {
                    let body = Self::render_implicit_object(entries, FileIndent::ROOT, options);
                    return Self::place_indent_glyph(body, pair_indent, value_column, key_folded, options);
                }

                let lines = if effective_force_markers(options) {
                    Self::render_explicit_object(entries, pair_indent, options)
                } else {
                    Self::render_implicit_object(entries, pair_indent, options)
                };
                ValuePlacement::Below { lines }
            }
            _ => {
                let bv = basic_value(value).expect("every remaining node is a basic value");
                // How much of this line the key has already spent. A folded key spends
                // its last continuation, an unfolded one spends `indent + key + ':'`.
                let first_line_extra = value_column.saturating_sub(pair_indent.width());
                let scalar_lines = Self::render_scalar_lines(bv, pair_indent, first_line_extra, options);
                let value_suffix = pair_indent.strip(&scalar_lines[0]).unwrap_or("");

                // If `key: value` would overrun the margin and folding is available, the
                // value takes a `/ ` line of its own instead of sharing the key's.
                //
                // Asked of the value. This read `fold_enabled`, computed at the key's
                // site from the **key's** fold style -- so a 60-digit number after a
                // 27-character bare key stayed on a 90-column line at a 40-column
                // margin, because bare-string *key* folding happened to be off. The
                // continuation it was never offered had 36 columns free.
                let assembled_len = value_column + Columns::of(value_suffix);
                if may_take_a_continuation(bv, options)
                    && let Some(w) = options.wrap_width
                        && assembled_len > Columns::new(w)
                        // ...and the value is what pushes it over. When the key's
                        // line does not fit on its own, moving the value below
                        // leaves that line just as long and spends an extra one to
                        // do it -- a one-digit value under a key that already
                        // overflows buys a single column for a whole line.
                        && value_column <= Columns::new(w) {
                            let cont_lines =
                                Self::render_scalar_value_continuation_lines(bv, pair_indent, options);
                            let first_cont = pair_indent.strip(&cont_lines[0]).unwrap_or("");
                            let mut lines = vec![(indent_marked(pair_indent.deeper(1), Marker::Fold) + first_cont)];
                            lines.extend(cont_lines.into_iter().skip(1));
                            return ValuePlacement::Below { lines };
                        }

                let after_colon = value_suffix.to_owned();
                if scalar_lines.len() == 1 {
                    ValuePlacement::OnKeyLine { after_colon }
                } else {
                    // A multiline string's glyph, or a folded string's first segment:
                    // the value opens here and the rest of it lives below.
                    ValuePlacement::OpensOnKeyLine {
                        opener: after_colon,
                        below: scalar_lines.into_iter().skip(1).collect(),
                    }
                }
            }
        }
    }

    fn render_implicit_array(
        values: &[T],
        parent_indent: FileIndent,
        opinion: Option<bool>,
        options: &RenderOptions,
    ) -> Vec<String> {
        if let Some(forced) = resolve_table_attempt(opinion, options)
            && let Some(lines) = Self::render_table(values, parent_indent, forced, options) {
                return lines;
            }

        if effective_inline_arrays(options) && !values.first().is_some_and(needs_explicit_array_marker)
            && let Some(lines) = Self::render_packed_array_lines(
                values,
                parent_indent.deeper(1).spaces().to_string(),
                parent_indent.deeper(1),
                options,
            ) {
                return lines;
            }

        let elem_indent = parent_indent.deeper(1);
        let element_lines: Vec<Vec<String>> = values
            .iter()
            .map(|value| Self::render_array_element(value, elem_indent, options))
            .collect();
        if values.first().is_some_and(needs_explicit_array_marker) {
            let mut lines = Vec::new();
            emit_comments(values[0].comments_before(), elem_indent, options, &mut lines);
            let first = &element_lines[0];
            let first_line = first.first()
                .expect("render_array_element always returns at least one line");
            let stripped = elem_indent.strip(first_line)
                .expect("array element line is indented at elem_indent");
            lines.push(indent_marked(parent_indent.deeper(1), Marker::Array) + stripped);
            lines.extend(first.iter().skip(1).cloned());
            for (value, extra) in values.iter().zip(element_lines.iter()).skip(1) {
                emit_comments(value.comments_before(), elem_indent, options, &mut lines);
                lines.extend(extra.clone());
            }
            lines
        } else {
            let mut lines = Vec::new();
            for (value, elem_lines) in values.iter().zip(element_lines) {
                emit_comments(value.comments_before(), elem_indent, options, &mut lines);
                lines.extend(elem_lines);
            }
            lines
        }
    }

    fn render_array_children(
        values: &[T],
        elem_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        let table_row_prefix = format!("{}|", elem_indent.spaces());
        for value in values {
            // prev_was_table is judged before comment lines go in: comment lines are
            // ignorable inside tables on reparse, so two tables separated only by a
            // comment would merge without the `[ ` marker the check below forces.
            let prev_was_table = lines.last().map(|l: &String| l.starts_with(&table_row_prefix)).unwrap_or(false);
            emit_comments(value.comments_before(), elem_indent, options, &mut lines);
            let elem_lines = Self::render_array_element(value, elem_indent, options);
            let curr_is_table = elem_lines.first().map(|l| l.starts_with(&table_row_prefix)).unwrap_or(false);
            if prev_was_table && curr_is_table {
                // Two consecutive tables: the second needs a `[ ` marker to separate them.
                let first = elem_lines.first().unwrap();
                let stripped = elem_indent.strip(&first).unwrap_or(""); // e.g. "|col  |..."
                lines.push(format!("{}{}{stripped}", elem_indent.shallower(1).spaces(), Marker::Array.text()));
                lines.extend(elem_lines.into_iter().skip(1));
            } else {
                lines.extend(elem_lines);
            }
        }
        lines
    }

    fn render_explicit_array(
        values: &[T],
        marker_indent: FileIndent,
        opinion: Option<bool>,
        options: &RenderOptions,
    ) -> Vec<String> {
        if let Some(forced) = resolve_table_attempt(opinion, options)
            && let Some(lines) = Self::render_table(values, marker_indent, forced, options) {
                // Always prepend "[ " — render_explicit_array always needs its marker,
                // whether the elements render as a table or in any other form.
                let elem_indent = marker_indent.deeper(1);
                let first = lines.first()
                    .expect("render_table always returns at least a header line");
                let stripped = elem_indent.strip(first)
                    .expect("table line starts at elem_indent");
                let mut out = vec![(indent_marked(marker_indent.deeper(1), Marker::Array) + stripped)];
                out.extend(lines.into_iter().skip(1));
                return out;
            }

        if effective_inline_arrays(options)
            && let Some(lines) = Self::render_packed_array_lines(
                values,
                indent_marked(marker_indent.deeper(1), Marker::Array),
                marker_indent.deeper(1),
                options,
            ) {
                return lines;
            }

        let elem_indent = marker_indent.deeper(1);
        let element_lines: Vec<Vec<String>> = values
            .iter()
            .map(|value| Self::render_array_element(value, elem_indent, options))
            .collect();
        let first = element_lines.first()
            .unwrap_or_else(|| unreachable!("render_explicit_array called with empty values"));
        let first_line = first.first()
            .expect("render_array_element always returns at least one line");
        let stripped = elem_indent.strip(first_line)
            .expect("array element line is indented at elem_indent");
        let mut lines = Vec::new();
        // A first element's comments precede the `[ ` line; on reparse they attach to
        // the container, which renders in the same position.
        emit_comments(values[0].comments_before(), elem_indent, options, &mut lines);
        lines.push(indent_marked(marker_indent.deeper(1), Marker::Array) + stripped);
        lines.extend(first.iter().skip(1).cloned());
        for (value, extra) in values.iter().zip(element_lines.iter()).skip(1) {
            emit_comments(value.comments_before(), elem_indent, options, &mut lines);
            lines.extend(extra.clone());
        }
        lines
    }

    fn render_explicit_object(
        entries: &[T::Entry],
        marker_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        let pair_indent = marker_indent.deeper(1);
        let implicit_lines = Self::render_implicit_object(entries, marker_indent, options);
        let first_line = implicit_lines.first()
            .expect("render_implicit_object with non-empty entries returns at least one line");
        let stripped = pair_indent.strip(first_line)
            .expect("implicit object line is indented at pair_indent");
        let mut lines = vec![(indent_marked(marker_indent.deeper(1), Marker::Object) + stripped)];
        lines.extend(implicit_lines.into_iter().skip(1));
        lines
    }

    fn render_array_element(
        value: &T,
        elem_indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        match value.node() {
            NodeRef::Array(values) if !values.is_empty() => {
                if should_use_indent_glyph(value, elem_indent, options) {
                    // ` /<` shifts the frame; it does not open a container. This
                    // element is an array, so its level has to be spelled by
                    // somebody -- and which of the two bodies below does it
                    // differs. `render_explicit_array` writes the `[ ` itself;
                    // `render_array_children` writes only the elements, and then
                    // nothing said the array was there. The children landed at the
                    // shifted origin as siblings of the *outer* array's elements,
                    // so `[["a"],["g",…]]` came back as `[["a"],"g",…]` -- a level
                    // gone, silently, in output that parses.
                    let (body, level_spelled_by_body) =
                        if values.first().is_some_and(|v| needs_explicit_array_marker(v)) {
                            (
                                Self::render_explicit_array(
                                    values,
                                    FileIndent::ROOT,
                                    value.table_opinion(),
                                    options,
                                ),
                                true,
                            )
                        } else {
                            (Self::render_array_children(values, FileIndent::ROOT, options), false)
                        };
                    // Where the glyph sits, and therefore where its closer must
                    // pair, moves with the marker when this line carries one.
                    let glyph_indent =
                        if level_spelled_by_body { elem_indent } else { elem_indent.deeper(1) };
                    let opener = if level_spelled_by_body {
                        Glyph::IndentOpen.at(elem_indent.spaces())
                    } else {
                        indent_marked(elem_indent.deeper(1), Marker::Array)
                            + Glyph::IndentOpen.text()
                    };
                    let mut lines = vec![opener];
                    lines.extend(body);
                    lines.push(Glyph::IndentClose.at(glyph_indent.spaces()));
                    return lines;
                }
                Self::render_explicit_array(values, elem_indent, value.table_opinion(), options)
            }
            NodeRef::Object(entries) if !entries.is_empty() => {
                Self::render_explicit_object(entries, elem_indent, options)
            }
            NodeRef::Null => Self::render_scalar_lines(BasicValue::Null, elem_indent, Columns::ZERO, options),
            NodeRef::Bool(b) => Self::render_scalar_lines(BasicValue::Bool(b), elem_indent, Columns::ZERO, options),
            NodeRef::Number(n) => Self::render_scalar_lines(BasicValue::Number(n), elem_indent, Columns::ZERO, options),
            NodeRef::String(s) => {
                Self::render_scalar_lines(BasicValue::String(s, value.string_form()), elem_indent, Columns::ZERO, options)
            }
            NodeRef::Array(_) => Self::render_scalar_lines(BasicValue::EmptyArray, elem_indent, Columns::ZERO, options),
            NodeRef::Object(_) => Self::render_scalar_lines(BasicValue::EmptyObject, elem_indent, Columns::ZERO, options),
        }
    }

    /// Render a scalar's lines starting at `indent`, where `first_line_extra` columns
    /// of that first line are already spent by whatever precedes it -- a key and its
    /// colon, a `/ ` marker, nothing at all.
    ///
    /// It takes that the same way [`Self::render_string_lines`] does, and for the same
    /// reason: a folder budgeting against a column the text does not start at gets the
    /// fold points wrong. Numbers used to reach it as a hardcoded zero, so a long number
    /// after a long key folded as though the key were not there.
    fn render_scalar_lines(
        value: BasicValue<'_>,
        indent: FileIndent,
        first_line_extra: Columns,
        options: &RenderOptions,
    ) -> Vec<String> {
        match value {
            BasicValue::Null => vec![format!("{}null", indent.spaces())],
            BasicValue::Bool(b) => vec![format!(
                "{}{}",
                indent.spaces(),
                if b { "true" } else { "false" }
            )],
            BasicValue::Number(n) => {
                let s = n.to_string();
                if let Some(lines) =
                    fold_number(&s, indent, first_line_extra, options.number_fold_style, wrap_budget(options))
                {
                    return lines;
                }
                vec![format!("{}{}", indent.spaces(), s)]
            }
            BasicValue::String(s, form) => {
                Self::render_string_lines(s, form, indent, first_line_extra, options)
            }
            BasicValue::EmptyArray => vec![format!("{}[]", indent.spaces())],
            BasicValue::EmptyObject => vec![format!("{}{{}}", indent.spaces())],
        }
    }

    fn render_string_lines(
        value: &str,
        form: Option<StringForm>,
        indent: FileIndent,
        first_line_extra: Columns,
        options: &RenderOptions,
    ) -> Vec<String> {
        if value.is_empty() {
            return vec![format!("{}\"\"", indent.spaces())];
        }
        let meta = StrMeta::new(value);
        // One dispatch, on the decision [`string_rendering`] owns. The arms used
        // to be a ladder of guards here, and the permission check had to re-derive the
        // same ladder to learn which style governs a value -- two readings of one
        // rule, which is how a value's placement came to be decided by the key's
        // fold style.
        match string_rendering(value, form, &meta, options) {
            StringRendering::Quoted => {
                Self::render_quoted_string_lines(value, indent, first_line_extra, options)
            }
            StringRendering::Bare(opener_form) => {
                if options.string_bare_fold_style != FoldStyle::None
                    && let Some(lines) = fold_bare_string(
                        value,
                        indent,
                        first_line_extra,
                        opener_form,
                        options.string_bare_fold_style,
                        wrap_budget(options),
                    )
                {
                    return lines;
                }
                vec![format!("{}{}", indent.spaces(), opened_bare(opener_form, value))]
            }
            StringRendering::HonoredMultiline(flavor) => {
                Self::render_multiline_flavor(value, flavor, indent, options)
            }
            StringRendering::FoldingQuotes => render_folding_quotes(value, indent, options),
            StringRendering::StyleMultiline => {
                let local_eol = meta.eol_type.expect("StyleMultiline requires a uniform EOL");
            let suffix = local_eol.opener_suffix();
            let parts: Vec<&str> = match local_eol {
                MultilineLocalEol::Lf => value.split('\n').collect(),
                MultilineLocalEol::CrLf => value.split("\r\n").collect(),
            };
                // The EOL count against `multiline_min_lines` is `string_rendering`'s
                // to check; reaching this arm is that check having passed.
                let fold_style = options.string_multiline_fold_style;
                let wrap = wrap_budget(options);

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
                    .map(|w| parts.iter().any(|p| indent.deeper(1).width() + Columns::of(p) > w))
                    .unwrap_or(false);

                // Whether line count exceeds the configured maximum
                let too_many_lines = options.multiline_max_lines > 0
                    && parts.len() > options.multiline_max_lines;

                let bold = |body_indent: FileIndent| {
                    Self::render_multiline_double_backtick(
                        &parts, indent, body_indent, suffix, fold_style, wrap,
                    )
                };

                match options.multiline_style {
                    MultilineStyle::Floating => {
                        // Fall back to `` when content is unsafe OR would exceed width/line-count
                        if forced_bold || overflows_at_natural || too_many_lines {
                            bold(FileIndent::ROOT)
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
                            bold(FileIndent::ROOT)
                        } else {
                            Self::render_multiline_single_backtick(
                                &parts, indent, suffix, fold_style, wrap,
                            )
                        }
                    }
                    MultilineStyle::Bold => bold(FileIndent::ROOT),
                    MultilineStyle::BoldFloating => {
                        let body = if forced_bold || overflows_at_natural { FileIndent::ROOT } else { indent };
                        bold(body)
                    }
                    // BoldLight never leaves the natural indent: overflow is accepted,
                    // and the pipe-guarded body has no content unsafe at any indent.
                    MultilineStyle::BoldLight => bold(indent),
                    MultilineStyle::Transparent => {
                        if forced_bold {
                            bold(FileIndent::ROOT)
                        } else {
                            Self::render_multiline_triple_backtick(&parts, indent, suffix)
                        }
                    }
                    MultilineStyle::FoldingQuotes => unreachable!(),
                }
            }
        }
    }

    /// Render a string as a JSON quoted string, folding when policy and width call for it.
    fn render_quoted_string_lines(
        value: &str,
        indent: FileIndent,
        first_line_extra: Columns,
        options: &RenderOptions,
    ) -> Vec<String> {
        if options.string_quoted_fold_style != FoldStyle::None
            && let Some(lines) =
                fold_json_string(value, indent, first_line_extra, options.string_quoted_fold_style, wrap_budget(options), "")
            {
                return lines;
            }
        vec![format!("{}{}", indent.spaces(), render_json_string(value))]
    }

    /// Render a multiline string in an honored concrete flavor. The content-safety
    /// fallbacks still apply — pipe-heavy or backtick-starting bodies force the
    /// pipe-guarded double-backtick form exactly as style-driven selection does —
    /// but the honored flavor bypasses the width/line-count preferences: the author
    /// chose this shape.
    fn render_multiline_flavor(
        value: &str,
        flavor: MultilineFlavor,
        indent: FileIndent,
        options: &RenderOptions,
    ) -> Vec<String> {
        let meta = StrMeta::new(value);
        let local_eol = meta.eol_type.expect("caller verified uniform EOLs");
        let suffix = local_eol.opener_suffix();
        let parts: Vec<&str> = match local_eol {
            MultilineLocalEol::Lf => value.split('\n').collect(),
            MultilineLocalEol::CrLf => value.split("\r\n").collect(),
        };
        let fold_style = options.string_multiline_fold_style;
        let wrap = wrap_budget(options);
        let pipe_heavy = {
            let pipe_count = parts.iter().filter(|p| line_starts_with_ws_then(p, '|')).count();
            !parts.is_empty() && pipe_count * 10 > parts.len()
        };
        let backtick_start = parts.iter().any(|p| line_starts_with_ws_then(p, '`'));
        let forced_bold = pipe_heavy || backtick_start;
        match flavor {
            MultilineFlavor::Single if !forced_bold => {
                Self::render_multiline_single_backtick(&parts, indent, suffix, fold_style, wrap)
            }
            MultilineFlavor::Triple if !forced_bold => {
                Self::render_multiline_triple_backtick(&parts, indent, suffix)
            }
            _ => Self::render_multiline_double_backtick(
                &parts,
                indent,
                FileIndent::ROOT,
                suffix,
                fold_style,
                wrap,
            ),
        }
    }

    /// Render a multiline string using ` (single backtick, unmarked body at indent+2).
    /// Body lines are at indent+2. Fold continuations (if enabled) at indent.
    /// No folding is allowed when fold_style is None.
    fn render_multiline_single_backtick(
        parts: &[&str],
        indent: FileIndent,
        suffix: &str,
        fold_style: FoldStyle,
        wrap_width: Option<Columns>,
    ) -> Vec<String> {
        let glyph = Glyph::MultilineSingle.at_with_suffix(indent.spaces(), suffix);
        let body_indent = indent.deeper(1);
        let fold_prefix = indent_marked(indent.deeper(1), Marker::Fold);
        let avail = wrap_width.map(|w| w.saturating_sub(body_indent.width()));
        let mut lines = vec![glyph.clone()];
        for part in parts {
            if fold_style != FoldStyle::None
                && let Some(avail_w) = avail
                    && Columns::of(part) > avail_w {
                        let segments = split_multiline_fold(part, avail_w, fold_style);
                        let mut first = true;
                        for seg in segments {
                            if first {
                                lines.push(format!("{}{}", body_indent.spaces(), seg));
                                first = false;
                            } else {
                                lines.push(format!("{}{}", fold_prefix, seg));
                            }
                        }
                        continue;
                    }
            lines.push(format!("{}{}", body_indent.spaces(), part));
        }
        lines.push(glyph);
        lines
    }

    /// Render a multiline string using `` (double backtick, pipe-guarded body).
    /// Body lines are at body_indent with `| ` prefix. Fold continuations at body_indent-2.
    fn render_multiline_double_backtick(
        parts: &[&str],
        indent: FileIndent,
        body_indent: FileIndent,
        suffix: &str,
        fold_style: FoldStyle,
        wrap_width: Option<Columns>,
    ) -> Vec<String> {
        let glyph = Glyph::MultilineDouble.at_with_suffix(indent.spaces(), suffix);
        // At the margin, not one level out from it. A `/ ` normally replaces the
        // last level of the indent, but a body line's indent is already spelled by
        // its `| ` -- so here the fold replaces the `| `, and stands where it
        // stands.
        //
        // `shallower(1)` sat here, and was wrong for every flavour. Three of them
        // pass `body_indent = ROOT`, where the subtraction saturates to zero and
        // lands on the right column anyway; only the flavour that keeps its body at
        // its natural indent showed it, by emitting a fold two columns left of its
        // own margin that the parser then refused.
        let fold_prefix = format!("{}{}", body_indent.spaces(), Marker::Fold.text());
        let avail = wrap_width
            .map(|w| w.saturating_sub(body_indent.width() + Columns::new(Marker::Body.width())));
        let mut lines = vec![glyph.clone()];
        for part in parts {
            if fold_style != FoldStyle::None
                && let Some(avail_w) = avail
                    && Columns::of(part) > avail_w {
                        let segments = split_multiline_fold(part, avail_w, fold_style);
                        let mut first = true;
                        for seg in segments {
                            if first {
                                lines.push(indent_marked(body_indent.deeper(1), Marker::Body) + seg);
                                first = false;
                            } else {
                                lines.push(format!("{}{}", fold_prefix, seg));
                            }
                        }
                        continue;
                    }
            lines.push(indent_marked(body_indent.deeper(1), Marker::Body) + part);
        }
        lines.push(glyph);
        lines
    }

    /// Render a multiline string using ``` (triple backtick, body at col 0).
    /// No folding is allowed in ``` format per spec.
    /// Currently not invoked by the default selection heuristic; available for explicit use.
    #[allow(dead_code)]
    fn render_multiline_triple_backtick(parts: &[&str], indent: FileIndent, suffix: &str) -> Vec<String> {
        let glyph = Glyph::MultilineTriple.at_with_suffix(indent.spaces(), suffix);
        let mut lines = vec![glyph.clone()];
        for part in parts {
            lines.push((*part).to_owned());
        }
        lines.push(glyph);
        lines
    }

    fn render_inline_object_token(
        key: &str,
        key_form: Option<KeyForm>,
        value: &T,
        options: &RenderOptions,
    ) -> Option<String> {
        let bv = match value.node() {
            NodeRef::String(s) if s.contains('\n') || s.contains('\r') => return None,
            _ => basic_value(value)?,
        };
        Some(format!(
            "{}:{}",
            render_key_form(key, key_form, options),
            Self::render_scalar_token(bv, options)
        ))
    }

    fn render_scalar_token(value: BasicValue<'_>, options: &RenderOptions) -> String {
        match value {
            BasicValue::Null => "null".to_owned(),
            BasicValue::Bool(b) => if b { "true".to_owned() } else { "false".to_owned() },
            BasicValue::Number(n) => n.to_string(),
            BasicValue::String(s, form) => match resolve_string_form(form, options) {
                Some(StringForm::Bare(form)) if BareString::new(s).is_some() => {
                    opened_bare(bare_opener_for(Some(form), options), s)
                }
                Some(StringForm::Quoted) => render_json_string(s),
                _ => {
                    if options.bare_strings != StringStyle::Quoted && BareString::new(s).is_some() {
                        opened_bare(bare_opener_for(None, options), s)
                    } else {
                        render_json_string(s)
                    }
                }
            },
            BasicValue::EmptyArray => "[]".to_owned(),
            BasicValue::EmptyObject => "{}".to_owned(),
        }
    }

    fn render_packed_array_lines(
        values: &[T],
        first_prefix: String,
        continuation_indent: FileIndent,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        if values.is_empty() {
            return Some(vec![format!("{first_prefix}[]")]);
        }

        // Every packed array routes through the style, not just all-string ones.
        // The question a packed line asks of an element is "can you be bare",
        // never "are you a string" -- a number simply answers no, the same as a
        // string that cannot go bare. Forking on `is_string` sent any array with
        // one number straight to array format 2, which quoted every string in it
        // and put the style out of reach entirely.
        Self::render_string_array_lines(values, first_prefix, continuation_indent, options)
    }

    fn render_string_array_lines(
        values: &[T],
        first_prefix: String,
        continuation_indent: FileIndent,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        // Every style below is a rule about a *line*, not about the array. A line
        // is packed or it is not; only a packed line has a format; and only array
        // format 2 costs a bare-able string its quotes. An array whose elements
        // land on separate lines can hold bare and quoted elements at once, since
        // each line picks its own format -- so "comma" never means "quote the
        // whole array", it means "pack lines with commas".
        //
        // Two layouts serve all five:
        //
        // - the *comma* layout packs every element into one comma-separated run,
        //   which is what forces bare-able strings into quotes;
        // - the *split* layout cuts the array into maximal runs of like elements
        //   and gives each run its own line, so bare runs stay bare.
        //
        // Every style restricts only what a *string* may share a line with, so an
        // array holding no strings is laid out identically under all five, and the
        // comma layout is that layout. Short-circuiting is not just tidiness: a
        // style that compares two layouts renders the array twice, and arrays
        // nest, so the doubling compounds -- a 30-deep array cost 2^30 renders and
        // looked like a hang. Strings never nest, so this catches every deep case.
        let comma_layout = |prefix: String| {
            let tokens = Self::render_packed_array_tokens(values);
            Self::render_packed_token_lines(tokens, prefix, continuation_indent, false, options)
        };

        if !values.iter().any(|value| value.is_string()) {
            return comma_layout(first_prefix);
        }

        let commas = comma_layout(first_prefix.clone());

        match options.string_array_style {
            // No string shares a line with anything. Scalars are not strings, so
            // they still pack -- this is the last rung of a ladder that only ever
            // takes things away from strings, and adding a string to an array
            // should not change how its numbers are laid out.
            StringArrayStyle::None => Self::render_split_array_lines(
                values,
                first_prefix,
                continuation_indent,
                StringPacking::Never,
                options,
            ),

            // Always pack lines with commas, accepting quotes on strings that had
            // no other reason to be quoted.
            StringArrayStyle::Comma => commas,

            // A string is never comma packed, bare-able or not, so an unbareable
            // one stands alone -- but bare-able ones still space pack together.
            StringArrayStyle::Spaces => Self::render_split_array_lines(
                values,
                first_prefix,
                continuation_indent,
                StringPacking::NoComma,
                options,
            ),

            // Prefer keeping strings bare over keeping the array compact. Runs of
            // unbareable elements still pack with commas, since that is the only
            // format available to them.
            StringArrayStyle::PreferSpaces => Self::render_split_array_lines(
                values,
                first_prefix,
                continuation_indent,
                StringPacking::Free,
                options,
            ),

            // Prefer commas over losing vertical space -- quotes are what you pay
            // to save a line, so pay only when a line is actually saved. A tie
            // buys nothing, so it goes to the bare form: an all-bare array packs
            // onto one line either way, and the comma version would just be the
            // same line wearing quotes.
            StringArrayStyle::PreferComma => {
                let split = Self::render_split_array_lines(
                    values,
                    first_prefix,
                    continuation_indent,
                    StringPacking::Free,
                    options,
                );
                match (commas, split) {
                    (Some(c), Some(s)) if c.len() < s.len() => Some(c),
                    (_, Some(s)) => Some(s),
                    (commas, None) => commas,
                }
            }
        }
    }

    /// Render an array as runs, one run per line group.
    ///
    /// Spec 0.5.0, Array Format: "Different lines in the representation of the
    /// same data array can pick 1), 2) or 3) without parse issues." That is what
    /// makes this possible -- an array is cut into maximal runs of like elements
    /// and each run gets its own line, bare runs as format 3 and the rest as
    /// format 2, so one element that cannot go bare does not cost the whole array
    /// its bare forms.
    ///
    /// Runs are positional, since array order is data -- the same elements in a
    /// different order cost a different number of lines.
    ///
    /// `packing` is what separates the three splitting styles. Those are generator
    /// options rather than spec rules, and each rung only ever takes something
    /// away from strings, never from anything else: scalars pack the same way
    /// under all three, so adding a string to an array does not change how its
    /// numbers are laid out.
    fn render_split_array_lines(
        values: &[T],
        first_prefix: String,
        continuation_indent: FileIndent,
        packing: StringPacking,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        let mut runs: Vec<(bool, &[T])> = Vec::new();
        let mut start = 0;
        while start < values.len() {
            let bare = Self::packs_as_bare_string(&values[start], options);
            let mut end = start + 1;
            while end < values.len()
                && Self::packs_as_bare_string(&values[end], options) == bare
            {
                end += 1;
            }
            runs.push((bare, &values[start..end]));
            start = end;
        }

        let mut lines: Vec<String> = Vec::new();
        for (bare, run) in runs {
            // A bare run is all strings, so `Never` explodes it and the other two
            // leave it space packed. A run that is not bare may hold scalars and
            // unbareable strings together: both `Never` and `NoComma` cut it again
            // on "is it a string", giving each string its own line while a stretch
            // of scalars stays packed, since neither rung speaks about scalars.
            let isolate = !matches!(
                (bare, packing),
                (_, StringPacking::Free) | (true, StringPacking::NoComma)
            );
            let groups: Vec<&[T]> = if !isolate {
                vec![run]
            } else if bare {
                (0..run.len()).map(|index| &run[index..index + 1]).collect()
            } else {
                let mut groups = Vec::new();
                let mut index = 0;
                while index < run.len() {
                    if run[index].is_string() {
                        groups.push(&run[index..index + 1]);
                        index += 1;
                    } else {
                        let start = index;
                        while index < run.len() && !run[index].is_string() {
                            index += 1;
                        }
                        groups.push(&run[start..index]);
                    }
                }
                groups
            };
            for group in groups {
                let prefix = if lines.is_empty() {
                    first_prefix.clone()
                } else {
                    continuation_indent.spaces().to_string()
                };
                let tokens = Self::render_packed_array_tokens(group);
                let group_lines = Self::render_packed_token_lines(
                    tokens,
                    prefix,
                    continuation_indent,
                    bare,
                    options,
                )?;
                lines.extend(group_lines);
            }
        }
        Some(lines)
    }

    /// Would this element render as a bare string in a packed array?
    ///
    /// Mirrors the decision `render_scalar_token` makes, so the two cannot drift.
    fn packs_as_bare_string(value: &T, options: &RenderOptions) -> bool {
        let NodeRef::String(s) = value.node() else { return false };
        if s.contains('\n') || s.contains('\r') {
            return false; // a multiline string is a block element, never packed
        }
        match resolve_string_form(value.string_form(), options) {
            Some(StringForm::Bare(_)) => BareString::new(s).is_some(),
            Some(StringForm::Quoted) => false,
            _ => options.bare_strings != StringStyle::Quoted && BareString::new(s).is_some(),
        }
    }

    /// Classify each element as an inline token or a block that owns its lines.
    ///
    /// Nothing here knows the line's format. Every string keeps its natural form,
    /// and `render_packed_token_lines` applies array format 2's quoting to the
    /// elements that actually end up sharing a line -- an element alone on a line
    /// was never packed, so nothing forced it to give up its bare form.
    fn render_packed_array_tokens<'v>(
        values: &'v [T],
    ) -> Vec<(&'v [Comment], PackedToken<'v, T>)> {
        let mut tokens = Vec::new();
        for value in values {
            let token = match value.node() {
                // Multiline strings are block elements — cannot be packed inline.
                NodeRef::String(text) if text.contains('\n') || text.contains('\r') => {
                    PackedToken::Block(value)
                }
                // Nonempty arrays and objects are block elements.
                NodeRef::Array(vals) if !vals.is_empty() => PackedToken::Block(value),
                NodeRef::Object(entries) if !entries.is_empty() => PackedToken::Block(value),
                // Scalars and empty containers become inline tokens.
                NodeRef::Null => PackedToken::Inline(BasicValue::Null),
                NodeRef::Bool(b) => PackedToken::Inline(BasicValue::Bool(b)),
                NodeRef::Number(n) => PackedToken::Inline(BasicValue::Number(n)),
                // Always the element's natural form. Forcing quotes here would be
                // deciding for the whole array something that belongs to a line:
                // an element that ends up alone on a line was never packed, so
                // nothing forced it to give up its bare form. The line builder
                // applies format 2's quoting to the elements that actually share
                // a line.
                NodeRef::String(s) => PackedToken::Inline(BasicValue::String(s, value.string_form())),
                NodeRef::Array(_) => PackedToken::Inline(BasicValue::EmptyArray),
                NodeRef::Object(_) => PackedToken::Inline(BasicValue::EmptyObject),
            };
            tokens.push((value.comments_before(), token));
        }
        tokens
    }

    /// Try to fold a lone-overflow inline token value into multiple lines.
    /// Returns `Some(lines)` (with 2+ lines) when fold succeeded, `None` when it didn't
    /// (value fits or fold is disabled / below MIN_FOLD_CONTINUATION).
    fn fold_packed_inline(
        value: BasicValue<'_>,
        continuation_indent: FileIndent,
        first_line_extra: Columns,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        match value {
            BasicValue::String(s, form) => {
                let lines = Self::render_string_lines(s, form, continuation_indent, first_line_extra, options);
                if lines.len() > 1 { Some(lines) } else { None }
            }
            BasicValue::Number(n) => {
                let ns = n.to_string();
                fold_number(
                    &ns,
                    continuation_indent,
                    first_line_extra,
                    options.number_fold_style,
                    wrap_budget(options),
                )
                .filter(|l| l.len() > 1)
            }
            _ => None,
        }
    }

    /// The same value forced to its quoted string form.
    ///
    /// Spec 0.5.0, Array Format 2: "BARE STRINGS ARE NOT ALLOWED". So any string
    /// *sharing* a comma packed line is quoted whatever it would have been alone.
    fn as_packed_comma_token(value: BasicValue<'_>) -> BasicValue<'_> {
        match value {
            BasicValue::String(s, _) => BasicValue::String(s, Some(StringForm::Quoted)),
            other => other,
        }
    }

    /// Finish a line of inline elements, rebuilding it if it holds only one.
    ///
    /// A line carrying a single element was never packed, so it has no separator
    /// and no format, and array format 2's bar on bare elements does not reach it.
    /// The element takes its natural form. Dropping the trailing comma with it is
    /// spec 0.5.0, Array Format: the trailing `,` "SHOULD never be used for the
    /// last data element of an array or a line with only one element of the data
    /// array (but it should still parse for both)".
    fn finish_inline_line(
        line: String,
        prefix: &str,
        count: usize,
        single: Option<BasicValue<'_>>,
        options: &RenderOptions,
    ) -> String {
        if count == 1
            && let Some(value) = single
        {
            return format!("{}{}", prefix, Self::render_scalar_token(value, options));
        }
        line
    }

    fn render_packed_token_lines(
        tokens: Vec<(&[Comment], PackedToken<'_, T>)>,
        first_prefix: String,
        continuation_indent: FileIndent,
        string_spaces_mode: bool,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        if tokens.is_empty() {
            return Some(vec![first_prefix]);
        }

        // If the prefix alone already fills or exceeds wrap_width, no token can fit inline.
        if let Some(w) = options.wrap_width
            && Columns::of(&first_prefix) >= Columns::new(w)
        {
            return None;
        }

        // Spaces mode is incompatible with block elements (which are never strings).
        if string_spaces_mode && tokens.iter().any(|(_, t)| matches!(t, PackedToken::Block(_))) {
            return None;
        }

        // A commented first element cannot sit on a merged prefix line ("key:  " or
        // "[ ") — its comment lines would have nowhere legal to go. Bail to block
        // layout, which emits comments correctly.
        if options.render_comments
            && tokens.first().is_some_and(|(comments, _)| !comments.is_empty())
            && !first_prefix.chars().all(|c| c == ' ')
        {
            return None;
        }

        let separator = if string_spaces_mode { "  " } else { ", " };
        let continuation_prefix = continuation_indent.spaces().to_string();

        // `current` is the line being built. `current_is_fresh` is true when nothing
        // has been appended to `current` yet (it holds only the line prefix).
        let mut current = first_prefix.clone();
        let mut current_is_fresh = true;
        let mut lines: Vec<String> = Vec::new();

        // Tracks what the line being built ended up holding, so a line that turns
        // out to carry a single element can be rebuilt from that element's natural
        // form. Until the line is flushed there is no way to know: whether a second
        // element joins depends on the wrap, which depends on widths, which is why
        // the decision cannot be made when the tokens are built.
        let mut current_prefix = first_prefix.clone();
        let mut current_count = 0usize;
        let mut current_single: Option<BasicValue<'_>> = None;

        // A comma packed line that is not the array's last gains a trailing comma
        // when it is flushed, which happens after the fit test -- so the width has
        // to account for it up front or every wrapped line runs one character long.
        let token_total = tokens.len();

        for (token_index, (comments, token)) in tokens.into_iter().enumerate() {
            // Reserve room for the trailing comma this line will gain if it is
            // flushed with elements still to come. The array's final line never
            // gets one, so nothing is reserved there.
            let comma_reserve = !string_spaces_mode && token_index + 1 < token_total;
            let fits = |line: &str| {
                if comma_reserve {
                    fits_wrap(options, &format!("{line},"))
                } else {
                    fits_wrap(options, line)
                }
            };
            // A commented element starts a new packed run: flush the current line,
            // emit the comment at the element's level, resume packing from here.
            // The rule is never to invent packing across a comment, and never to
            // destroy packing elsewhere.
            if options.render_comments && !comments.is_empty() {
                if !current_is_fresh {
                    if !string_spaces_mode && current_count > 1 {
                        current.push(',');
                    }
                    lines.push(Self::finish_inline_line(
                        current,
                        &current_prefix,
                        current_count,
                        current_single,
                        options,
                    ));
                    current = continuation_prefix.clone();
                    current_prefix = continuation_prefix.clone();
                    current_is_fresh = true;
                    current_count = 0;
                    current_single = None;
                }
                emit_comments(comments, continuation_indent, options, &mut lines);
            }
            match token {
                PackedToken::Block(value) => {
                    // Flush the current line if it has content, then render the block.
                    if !current_is_fresh {
                        if !string_spaces_mode && current_count > 1 {
                            current.push(',');
                        }
                        lines.push(Self::finish_inline_line(
                            current,
                            &current_prefix,
                            current_count,
                            current_single,
                            options,
                        ));
                        current_count = 0;
                        current_single = None;
                    }

                    let block_lines = match value.node() {
                        NodeRef::String(s) => {
                            Self::render_string_lines(
                    s,
                    value.string_form(),
                    continuation_indent,
                    Columns::ZERO,
                    options,
                )
                        }
                        NodeRef::Array(vals) if !vals.is_empty() => {
                            Self::render_explicit_array(vals, continuation_indent, value.table_opinion(), options)
                        }
                        NodeRef::Object(entries) if !entries.is_empty() => {
                            Self::render_explicit_object(entries, continuation_indent, options)
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
                        continuation_indent.strip(&block_lines[0]).unwrap_or("");
                    lines.push(format!("{}{}", current_prefix_str, first_block_content));
                    for bl in block_lines.into_iter().skip(1) {
                        lines.push(bl);
                    }

                    current = continuation_prefix.clone();
                    current_prefix = continuation_prefix.clone();
                    current_is_fresh = true;
                }
                PackedToken::Inline(bv) => {
                    // Render the token string on demand. Bare strings need no special
                    // casing here: one can never contain two spaces in a row, so it can
                    // never contain a `,  ` separator either, and it always reads back
                    // whole. Whether the array uses bare strings at all was settled once
                    // in render_packed_array_tokens.
                    // Rendered as if it will share the line, since that is the wider
                    // form and so the safe one to measure against the wrap. If it
                    // turns out to be alone, `finish_inline_line` rebuilds it from
                    // the natural form, which is never wider.
                    let packed = if string_spaces_mode {
                        bv
                    } else {
                        Self::as_packed_comma_token(bv)
                    };
                    let token_str = Self::render_scalar_token(packed, options);

                    if current_is_fresh {
                        // Place the token on the fresh line (first_prefix or continuation).
                        current.push_str(&token_str);
                        current_is_fresh = false;
                        current_count = 1;
                        current_single = Some(bv);

                        // Lone-overflow check: the token alone already exceeds the width.
                        if !fits(&current) {
                            // `first_prefix.len()` stood here: a byte length used
                            // as a width, so a non-ASCII key overstated the room
                            // its own line had already taken.
                            let first_line_extra = if lines.is_empty() {
                                Columns::of(&first_prefix)
                                    .saturating_sub(continuation_indent.width())
                            } else {
                                Columns::ZERO
                            };
                            if let Some(fold_lines) = Self::fold_packed_inline(
                                bv,
                                continuation_indent,
                                first_line_extra,
                                options,
                            ) {
                                // Attach the real line prefix to the first fold line.
                                let actual_prefix = if lines.is_empty() {
                                    first_prefix.clone()
                                } else {
                                    continuation_prefix.clone()
                                };
                                let first_content =
                                    continuation_indent.strip(&fold_lines[0]).unwrap_or("");
                                lines.push(format!("{}{}", actual_prefix, first_content));
                                for fl in fold_lines.into_iter().skip(1) {
                                    lines.push(fl);
                                }
                                current = continuation_prefix.clone();
                                current_prefix = continuation_prefix.clone();
                                current_is_fresh = true;
                                current_count = 0;
                                current_single = None;
                            }
                            // else: overflow accepted — `current` retains the long line.
                        }
                    } else {
                        // Try to pack the token onto the current line.
                        let candidate = format!("{current}{separator}{token_str}");
                        if fits(&candidate) {
                            current = candidate;
                            current_count += 1;
                            current_single = None;
                        } else {
                            // Flush current line, move token to a fresh continuation line.
                            if !string_spaces_mode && current_count > 1 {
                                current.push(',');
                            }
                            lines.push(Self::finish_inline_line(
                                current,
                                &current_prefix,
                                current_count,
                                current_single,
                                options,
                            ));
                            current = format!("{}{}", continuation_prefix, token_str);
                            current_prefix = continuation_prefix.clone();
                            current_is_fresh = false;
                            current_count = 1;
                            current_single = Some(bv);

                            // Lone-overflow check on the new continuation line.
                            if !fits(&current)
                                && let Some(fold_lines) = Self::fold_packed_inline(
                                    bv,
                                    continuation_indent,
                                    Columns::ZERO,
                                    options,
                                ) {
                                    let first_content =
                                        continuation_indent.strip(&fold_lines[0]).unwrap_or("");
                                    lines.push(format!(
                                        "{}{}",
                                        continuation_prefix, first_content
                                    ));
                                    for fl in fold_lines.into_iter().skip(1) {
                                        lines.push(fl);
                                    }
                                    current = continuation_prefix.clone();
                                    current_prefix = continuation_prefix.clone();
                                    current_is_fresh = true;
                                    current_count = 0;
                                    current_single = None;
                                }
                                // else: overflow accepted.
                        }
                    }
                }
            }
        }

        if !current_is_fresh {
            lines.push(Self::finish_inline_line(
                current,
                &current_prefix,
                current_count,
                current_single,
                options,
            ));
        }

        Some(lines)
    }

    fn render_table(
        values: &[T],
        parent_indent: FileIndent,
        forced: bool,
        options: &RenderOptions,
    ) -> Option<Vec<String>> {
        // `forced` (an honored was-a-table fact) bypasses the size and similarity
        // heuristics but never the physical checks below: content a table cannot
        // hold still falls back to block layout.
        if !forced && values.len() < options.table_min_rows {
            return None;
        }

        let mut columns = Vec::<(String, Option<KeyForm>)>::new();
        let mut present_cells = 0usize;

        // Columns are collected in first-seen order across all rows, and every row
        // is then laid out in that order. A row whose own key order disagrees with
        // it would be reordered on round-trip — that is data loss, not a
        // similarity issue, so it refuses the table rather than rendering one.

        for value in values {
            let NodeRef::Object(entries) = value.node() else {
                return None;
            };
            present_cells += entries.len();
            for entry in entries {
                let cell = T::entry_value(entry);
                if matches!(cell.node(), NodeRef::Array(inner) if !inner.is_empty())
                    || matches!(cell.node(), NodeRef::Object(inner) if !inner.is_empty())
                    || matches!(cell.node(), NodeRef::String(text) if text.contains('\n') || text.contains('\r'))
                {
                    return None;
                }
            }
            // Merge this row's order into the column order, rather than appending
            // keys as they are first seen.
            //
            // Every row is laid out in column order, so a key belongs where the row
            // that introduced it put it: `a b c x` arriving after `a b x` inserts
            // `c` between `b` and `x`, and both rows then render in their own order.
            // Appending it instead dropped it after `x` -- which reordered the very
            // row that introduced it, silently, at default settings.
            //
            // Only a genuine contradiction is refused: a row asking for `b` before
            // `a` when `a` already precedes `b` cannot be merged into any single
            // column order, and no table can hold both rows without moving one of
            // them. Those values fall back to block objects, which keep their order.
            let mut cursor = 0usize;
            for entry in entries {
                let key = T::entry_key(entry);
                match columns.iter().position(|(column, _)| column == key) {
                    // Already placed, and placed compatibly with this row so far.
                    Some(at) if at >= cursor => cursor = at + 1,
                    Some(_) => return None,
                    None => {
                        columns.insert(cursor, (key.to_owned(), T::entry_key_form(entry)));
                        cursor += 1;
                    }
                }
            }
        }

        if !forced && columns.len() < options.table_min_columns {
            return None;
        }

        let similarity = present_cells as f32 / (values.len() * columns.len()) as f32;
        if !forced && similarity < options.table_min_similarity {
            return None;
        }

        let mut header_cells = Vec::new();
        let mut rows = Vec::new();
        for (column, key_form) in &columns {
            header_cells.push(render_key_form(column, *key_form, options));
        }

        for value in values {
            let NodeRef::Object(entries) = value.node() else {
                return None;
            };
            let mut row: Vec<String> = Vec::new();
            for (column, _) in &columns {
                let token = if let Some(entry) =
                    entries.iter().find(|e| T::entry_key(e) == column)
                {
                    Self::render_table_cell_token(T::entry_value(entry), options)
                } else {
                    None
                };
                row.push(token.unwrap_or_default());
            }
            rows.push(row);
        }

        // Columns, not bytes. `{:<width$}` pads by character, so a width taken
        // from a byte length reserves room the cell will not use -- a two-character
        // CJK cell measured at six makes its column twice as wide as it needs to
        // be, and can push the table past `table_column_max_width` and refuse to
        // render one that should have rendered.
        let mut widths = vec![Columns::ZERO; columns.len()];
        for (index, header) in header_cells.iter().enumerate() {
            widths[index] = Columns::of(header);
        }
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                widths[index] = widths[index].max(Columns::of(cell));
            }
        }
        // Bail out if any column's content exceeds table_column_max_width.
        // This does not and should not depend on table_fold.
        if let Some(col_max) = options.table_column_max_width
            && widths.iter().any(|w| *w > Columns::new(col_max)) {
                return None;
        }
        // The cell's own padding: one column each side of the content, inside the
        // `|` that delimits it.
        const CELL_PADDING: Columns = Columns::new_const(2);
        for width in &mut widths {
            *width = *width + CELL_PADDING;
        }

        // Bail out if the table is too wide to fit within wrap_width even at indent 0.
        // Each row is: (parent_indent.deeper(1)) spaces + |col1|col2|...|, where each colN width
        // includes 2 chars of padding. The caller handles unindenting via /< />, but if the
        // table still won't fit even at indent 0, block layout is better than overflow.
        if let Some(w) = options.wrap_width {
            // Each column renders as "|" + cell padded to `width` chars, plus trailing "|".
            // Minimum row width assumes indent 0: 2 spaces prefix + sum(widths) + one "|" per column + trailing "|".
            // The unindent logic may reduce indent below parent_indent, so only bail if it can't fit even at indent 0.
            // If table_fold is on, skip this bail-out — the fold logic below will handle overflow rows.
            let cells: Columns = widths.iter().fold(Columns::ZERO, |total, w| total + *w);
            // Two spaces of row prefix, the cells themselves, and one `|` opening each
            // column plus one closing the row.
            let min_row_width =
                Columns::new(2) + cells + Columns::new(widths.len()) + Columns::new(1);
            if min_row_width > Columns::new(w) && !options.table_fold {
                return None;
            }
        }

        let indent = parent_indent.deeper(1).spaces();
        let mut lines = Vec::new();
        lines.push(format!(
            "{}{}",
            indent,
            header_cells
                .iter()
                .zip(widths.iter())
                .map(|(cell, width)| format!("|{}", width.pad(cell)))
                .collect::<String>()
                + "|"
        ));

        // pair_indent for fold marker is two to the left of the `|` on each row
        let pair_indent = parent_indent; // elem rows at parent_indent+2, fold at parent_indent
        let fold_prefix = pair_indent.spaces();

        for (row_value, row) in values.iter().zip(rows) {
            emit_comments(row_value.comments_before(), parent_indent.deeper(1), options, &mut lines);
            let row_line = format!(
                "{}{}",
                indent,
                row.iter()
                    .zip(widths.iter())
                    .map(|(cell, width)| format!("|{}", width.pad(cell)))
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
                    .saturating_sub(pair_indent.deeper(1).width().columns()); // content after `  ` row prefix
                // A budget in columns, so it is compared against a count of them.
                // This used to weigh it against `row_line.len()`, a byte length:
                // a row of CJK is a third as long in characters as in bytes, so
                // it folded at a third of the intended width while the identical
                // table written in Latin text did not fold at all.
                let budget = Columns::new(fold_avail) + pair_indent.deeper(1).width();
                if Columns::of(&row_line) > budget {
                    // Find a fold point: must be within a cell's string data, after the
                    // leading space of a bare string or after the first `"` of a JSON string.
                    // We look for a space inside a cell value (not the cell padding spaces).
                    if let Some((before, after)) = split_table_row_for_fold(&row_line, budget) {
                        lines.push(before);
                        lines.push(format!("{fold_prefix}{}{after}", Marker::Fold.text()));
                        continue;
                    }
                }
            }

            lines.push(row_line);
        }

        Some(lines)
    }

    fn render_table_cell_token(
        value: &T,
        options: &RenderOptions,
    ) -> Option<String> {
        match value.node() {
            NodeRef::Null => Some("null".to_owned()),
            NodeRef::Bool(value) => Some(if value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }),
            NodeRef::Number(value) => Some(value.to_string()),
            NodeRef::String(s) => {
                if s.contains('\n') || s.contains('\r') {
                    return None;
                }
                match resolve_string_form(value.string_form(), options) {
                    Some(StringForm::Bare(form)) if TableBareString::new(s).is_some() => {
                        Some(opened_bare(bare_opener_for(Some(form), options), s))
                    }
                    Some(StringForm::Quoted) => Some(render_json_string(s)),
                    _ => {
                        if options.bare_strings != StringStyle::Quoted
                            && TableBareString::new(s).is_some()
                        {
                            Some(opened_bare(bare_opener_for(None, options), s))
                        } else {
                            Some(render_json_string(s))
                        }
                    }
                }
            }
            NodeRef::Array([]) => Some("[]".to_owned()),
            NodeRef::Object([]) => Some("{}".to_owned()),
            _ => None,
        }
    }
}
