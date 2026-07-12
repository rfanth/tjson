use js_sys::Error;
use wasm_bindgen::prelude::*;

fn err(msg: impl AsRef<str>) -> JsValue {
    Error::new(msg.as_ref()).into()
}

// JS <-> Rust value transport.
//
// Values cross the boundary as JSON *text*, produced and consumed by the two
// helpers below, rather than by per-node conversion (serde-wasm-bindgen was
// retired). One traversal both polices and carries the data, so nothing can
// be accepted that wasn't inspected:
//
//   * JSON.stringify semantics are the contract JS users already know:
//     toJSON() is honored (a Date serializes as its ISO string — that is the
//     value's own declared JSON form), and `undefined` object values mean
//     "absent" and drop the key.
//   * Values with NO declared JSON form fail loudly with the offending key
//     named: Map, Set, class instances without toJSON, and NaN/Infinity.
//   * Ill-formed strings (unpaired surrogates — which the raw wasm string
//     ABI would silently corrupt to U+FFFD) are rejected on the Rust side:
//     JSON.stringify emits the lone surrogate as an escape (well-formed
//     mode) and serde_json refuses that escape. Loud enforcement with no
//     per-string scan on the hot path.
//   * BigInt serializes as an exact JSON number via JSON.rawJSON where the
//     runtime has it (V8 11+/Node 21+), and errors loudly where it doesn't.
//     Combined with serde_json's arbitrary_precision and tjson's Number
//     (which stores the original digit string), integers of any size
//     round-trip exactly.
//   * On the way back to JS, integers beyond Number.MAX_SAFE_INTEGER error
//     loudly by default (a plain JS number cannot hold them exactly);
//     parse(text, { bigints: true }) revives them as BigInt from the exact
//     source digits instead.
// The transport helpers live in a real JS file (wasm-bindgen ships snippet
// modules alongside the wasm), where JS can be formatted, highlighted, and
// linted like JS instead of hiding inside a Rust string.
#[wasm_bindgen(module = "/src/js/value_transport.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = valueToJsonText)]
    fn value_to_json_text(value: &JsValue) -> Result<String, JsValue>;
    #[wasm_bindgen(catch, js_name = jsonTextToValue)]
    fn json_text_to_value(text: &str, bigints: bool) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = throwNamingIllFormedString)]
    fn throw_naming_ill_formed_string(value: &JsValue) -> Result<(), JsValue>;
}

#[wasm_bindgen(typescript_custom_section)]
const TS_TYPES: &'static str = r#"
export type BareStyle = "prefer" | "none";
export type FoldStyle = "auto" | "fixed" | "none";
export type MultilineStyle = "floating" | "bold" | "boldFloating" | "transparent" | "light" | "foldingQuotes";
export type TableUnindentStyle = "left" | "auto" | "floating" | "none";
export type StringArrayStyle = "spaces" | "preferSpaces" | "comma" | "preferComma" | "none";
export type IndentGlyphStyle = "auto" | "fixed" | "none";
export type IndentGlyphMarkerStyle = "compact" | "separate";
export type Eol = "lf" | "crlf";

export interface StringifyOptions {
    /** Start from a preset canonical configuration (one pair per line, no packing, no tables). */
    canonical?: boolean;
    /** Wrap width in columns. 0 means unlimited. Values between 1 and 19 are clamped to 20. */
    wrapWidth?: number;
    /** Force explicit `[` / `{` indent markers on arrays and objects, even for single-step indents that would normally be implicit. */
    forceMarkers?: boolean;
    /** Whether to use bare (unquoted) strings. Default: `"prefer"`. */
    bareStrings?: BareStyle;
    /** Whether to use bare (unquoted) object keys. Default: `"prefer"`. */
    bareKeys?: BareStyle;
    /** Allow packing multiple key-value pairs onto one line. Default: `true`. */
    inlineObjects?: boolean;
    /** Allow packing multiple array items onto one line. Default: `true`. */
    inlineArrays?: boolean;
    /** Allow multiline string blocks for strings containing newlines. Default: `true`. */
    multilineStrings?: boolean;
    /** Multiline block style. Default: `"bold"`. */
    multilineStyle?: MultilineStyle;
    /** Minimum number of lines before a multiline block is used. Default: `1`. */
    multilineMinLines?: number;
    /** @experimental Maximum number of lines in a minimal (`) multiline block before falling back to a bold style multiline block (``), applies with multilineStyle: "floating" only.  The idea is that we want to reserve a minimal style multiline for short multilines only for "floating".  "light" has a similar look with no max line fallback.  Default: `10`. */
    multilineMaxLines?: number;
    /** Enable table rendering for uniform arrays-of-objects. Default: `true`. */
    tables?: boolean;
    /** @experimental Allow folding long table rows across continuation lines.  (Not currently implemented.  It is probably best to avoid this option for now as it may change.)  Default: `false`. */
    tableFold?: boolean;
    /** Whether to push wide tables toward the left margin. Independent of `indentGlyphStyle`. Default: `"auto"`. */
    tableUnindentStyle?: TableUnindentStyle;
    /** Minimum rows required to render a table. Default: `3`. */
    tableMinRows?: number;
    /** Minimum columns required to render a table. Default: `3`. */
    tableMinColumns?: number;
    /** Minimum fraction [0–1] of rows sharing a column before it's included. Default: `0.8`. */
    tableMinSimilarity?: number;
    /** If any column's content width (including the leading space on bare string values) exceeds this value, the table is abandoned and falls back to block layout. `0` means no limit. Default: `40`. */
    tableColumnMaxWidth?: number;
    /** How to pack short-string arrays onto one line. Default: `"preferComma"`. */
    stringArrayStyle?: StringArrayStyle;
    /** Set all fold styles at once. More specific fold options override this if also set. */
    fold?: FoldStyle;
    /** How to fold long numbers across lines. Default: `"auto"`. */
    numberFoldStyle?: FoldStyle;
    /** How to fold bare strings. Default: `"auto"`. */
    stringBareFoldStyle?: FoldStyle;
    /** How to fold quoted strings. Default: `"auto"`. */
    stringQuotedFoldStyle?: FoldStyle;
    /** How to fold multiline string continuation lines. Default: `"none"`. */
    stringMultilineFoldStyle?: FoldStyle;
    /** Whether to wrap deeply-nested objects and arrays in `/<` `/>` glyphs to reduce visual depth. Independent of `tableUnindentStyle`. Default: `"auto"`. */
    indentGlyphStyle?: IndentGlyphStyle;
    /** Where to place the opening `/<` glyph. Default: `"compact"`. */
    indentGlyphMarkerStyle?: IndentGlyphMarkerStyle;
    /** @experimental Spacing multiplier between packed key-value pairs. Valid values: 1–4 (clamped); actual spaces = value × 2. Default: `2` (4 spaces). May be changed or removed in a future version. */
    kvPackMultiple?: number;
    /** Line ending used between output lines. `"lf"` (default) keeps output canonical and byte-identical across platforms; `"crlf"` is for a consumer that genuinely requires CRLF. Being on Windows is not itself a reason, as most Windows tooling handles LF, and TJSON survives whole-file LF↔CRLF conversion, so a consumer can usually convert on its own. Default: `"lf"`. */
    eol?: Eol;
}

export interface ParseOptions {
    /** Revive integers beyond Number.MAX_SAFE_INTEGER as BigInt (exact). When
     * false (the default), such integers throw rather than silently losing
     * precision as a JS number. Default: `false`. */
    bigints?: boolean;
}

/** Parse a TJSON string and return a JavaScript value.
 *
 * Inherently precision-bounded: tjson carries numbers at arbitrary
 * precision, but a JS number is an f64. Plain float precision loss is
 * accepted (you chose JS values); integers a JS number cannot hold exactly
 * throw by default or become BigInt with `{ bigints: true }`, and numbers
 * JSON.parse would turn into ±Infinity throw. For a lossless pipeline use
 * `toJson` (exact text out) with your own `JSON.parse` reviver. */
export function parse(input: string, options?: ParseOptions): any;

/** Parse a TJSON string and return a JSON string. Never lossy: numbers of
 * any size and precision pass through as exact text. */
export function toJson(input: string): string;

/** Render a JSON string as TJSON, with optional options. Never lossy:
 * numbers of any size and precision pass through as exact text. */
export function fromJson(input: string, options?: StringifyOptions): string;

/** Render a JavaScript value as TJSON, with optional options. */
export function stringify(input: any, options?: StringifyOptions): string;
"#;

/// Parse a TJSON string and return a JavaScript value.
///
/// Accepts the full TJSON format: bare strings and keys, multiline strings,
/// pipe tables, line folding, and comments. The output is a live JavaScript
/// value — object, array, string, number, boolean, or null.
///
/// ```js
/// const value = parse("  name: Alice\n  age: 30");
/// // → { name: "Alice", age: 30 }
/// ```
///
/// Inherently precision-bounded: tjson carries numbers at arbitrary
/// precision, but a JS number is an f64, so plain float precision loss is
/// accepted (the caller chose JS values). Integers a JS number cannot hold
/// exactly throw by default — pass `{ bigints: true }` to receive them as
/// BigInt, revived from the exact source digits — and numbers JSON.parse
/// would silently turn into ±Infinity throw. For a fully lossless pipeline,
/// use `toJson` (exact text) with your own `JSON.parse` reviver.
///
/// Throws an `Error` if the input is not valid TJSON.
#[wasm_bindgen(skip_typescript)]
pub fn parse(input: &str, options: JsValue) -> Result<JsValue, JsValue> {
    let json: serde_json::Value = crate::from_str(input)
        .map_err(|e| err(format!("invalid TJSON (input must be valid TJSON): {e}")))?;
    let text = serde_json::to_string(&json).map_err(|e| {
        err(format!("internal error serializing parsed value (this is likely a TJSON bug, please report it): {e}"))
    })?;
    // Same rigor as StringifyOptions: the bag must be a plain object, and
    // bigints must be an actual boolean — parse(text, [1]) or
    // { bigints: "false" } silently doing something was the alternative.
    let bigints = if options.is_null() || options.is_undefined() {
        false
    } else if !options.is_object() || js_sys::Array::is_array(&options) {
        return Err(err("options must be an object (or null/undefined for defaults)"));
    } else {
        let value = js_sys::Reflect::get(&options, &JsValue::from_str("bigints"))
            .unwrap_or(JsValue::UNDEFINED);
        if value.is_undefined() {
            false
        } else {
            match value.as_bool() {
                Some(flag) => flag,
                None => return Err(err("option bigints must be a boolean")),
            }
        }
    };
    json_text_to_value(&text, bigints)
}

/// Render a JavaScript value as TJSON, with optional options object.
///
/// Value handling follows `JSON.stringify` semantics: `toJSON()` methods are
/// honored (a `Date` serializes as its ISO string) and object keys with
/// `undefined` values are omitted. Values with no declared JSON form fail
/// loudly with the offending key named: `Map`, `Set`, class instances without
/// `toJSON`, `NaN`/`Infinity`, and strings containing unpaired surrogates.
/// `BigInt` serializes as an exact JSON number (requires `JSON.rawJSON`,
/// Node 21+ / modern browsers).
///
/// ```js
/// const tjson = stringify({ name: "Alice", scores: [1, 2, 3] });
///
/// // Canonical: one key per line, no packing, no tables
/// const canonical = stringify({ name: "Alice" }, { canonical: true });
/// ```
///
/// Throws an `Error` if the value is not JSON-serializable, or if an option
/// value is unrecognised.
#[wasm_bindgen(skip_typescript)]
pub fn stringify(input: JsValue, options: JsValue) -> Result<String, JsValue> {
    let text = value_to_json_text(&input)?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
        // The designed enforcement point for ill-formed strings: the JS
        // replacer does no per-string scan; lone surrogates arrive here as
        // \uXXXX escapes and serde_json refuses them. Rather than matching
        // serde's (rewordable) message text to classify the failure, let the
        // diagnostic re-walk decide: an ill-formed string is the only
        // invalid thing JSON.stringify can emit, so if the walk throws, that
        // was the cause — key-named. If it comes back clean, this is a
        // genuine bug. The document is already dead here, so the walk's
        // cost is irrelevant.
        match throw_naming_ill_formed_string(&input) {
            Err(named) => named,
            Ok(()) => err(format!("internal error re-reading serialized value (this is likely a TJSON bug, please report it): {e}")),
        }
    })?;
    let opts = parse_options(options)?;
    crate::to_string_with(&json, opts)
        .map_err(|e| err(format!("TJSON render error (this is likely a TJSON bug, please report it): {e}")))
}

/// Parse a TJSON string and return a JSON string.
///
/// Like `parse`, but returns a JSON string instead of a JavaScript value.
/// Useful when you need to pass the result to another JSON consumer, and it
/// carries numbers of any size exactly (there is no JS number on the path).
///
/// Throws an `Error` if the input is not valid TJSON.
#[wasm_bindgen(js_name = "toJson", skip_typescript)]
pub fn to_json(input: &str) -> Result<String, JsValue> {
    let json: serde_json::Value = crate::from_str(input)
        .map_err(|e| err(format!("invalid TJSON (input must be valid TJSON): {e}")))?;
    serde_json::to_string(&json).map_err(|e| {
        err(format!("internal error converting to JSON string (this is likely a TJSON bug, please report it): {e}"))
    })
}

/// Render a JSON string as TJSON, with optional options object.
///
/// Like `stringify`, but accepts a JSON string instead of a JavaScript value.
/// Useful when you already have a JSON string and want to avoid parsing it
/// first — and it carries numbers of any size exactly.
///
/// Throws an `Error` if the input is not valid JSON, or if an option value is
/// unrecognised.
#[wasm_bindgen(js_name = "fromJson", skip_typescript)]
pub fn from_json(input: &str, options: JsValue) -> Result<String, JsValue> {
    let json: serde_json::Value = serde_json::from_str(input)
        .map_err(|e| err(format!("invalid JSON string (input must be valid JSON): {e}")))?;
    let opts = parse_options(options)?;
    crate::to_string_with(&json, opts)
        .map_err(|e| err(format!("TJSON render error (this is likely a TJSON bug, please report it): {e}")))
}

fn parse_options(options: JsValue) -> Result<crate::RenderOptions, JsValue> {
    if options.is_null() || options.is_undefined() {
        return Ok(crate::RenderOptions::default());
    }
    // The options bag rides the same strict text pipeline as data values, so
    // a Map or an ill-formed string in an option fails loudly here too.
    let text = value_to_json_text(&options)?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| err(format!("invalid option value (see StringifyOptions for valid values): {e}")))?;
    // Options must be a plain object. Without this check, serde accepts a
    // JS array positionally as struct fields ([true] would silently mean
    // {canonical: true}) and bare scalars produce an error that leaks the
    // Rust struct name. null/undefined were already handled above and
    // keep meaning "defaults".
    let Some(object) = json.as_object() else {
        return Err(err("options must be an object (or null/undefined for defaults)"));
    };
    // Unknown fields are tolerated (idiomatic JS options bag; TypeScript
    // checks names at compile time), except retired names from the shared
    // table, which get a migration hint instead of silently no-opping.
    for retired in crate::options::RETIRED_OPTIONS {
        if object.contains_key(retired.name) {
            return Err(err(format!("{} — please update your code", retired.hint)));
        }
    }
    let config: crate::TjsonConfig = serde_json::from_value(json)
        .map_err(|e| err(format!("invalid option value (see StringifyOptions for valid values): {e}")))?;
    Ok(config.into())
}
