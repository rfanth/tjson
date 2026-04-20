#[cfg(target_arch = "wasm32")]
mod wasm;

mod error;
mod options;
mod parse;
mod render;
mod util;
mod value;

pub use error::{Error, ParseError, Result};
pub use options::{
    BareStyle, FoldStyle, IndentGlyphMarkerStyle, IndentGlyphStyle, MultilineStyle,
    StringArrayStyle, TableUnindentStyle, TjsonOptions,
};
pub use value::{Entry, TjsonValue};
#[doc(hidden)]
pub use options::TjsonConfig;

pub const MIN_WRAP_WIDTH: usize = options::MIN_WRAP_WIDTH;
pub const DEFAULT_WRAP_WIDTH: usize = options::DEFAULT_WRAP_WIDTH;

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

use parse::ParseOptions;
use render::Renderer;


fn parse_str_with_options(input: &str, options: ParseOptions) -> Result<TjsonValue> {
    parse::Parser::parse_document(input, options.start_indent).map_err(Error::Parse)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value as JsonValue;

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
                    .map(|e| e.key.as_str())
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
