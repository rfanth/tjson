// created by R.F. Anthracite <rfa@rfanth.com>
#![forbid(unsafe_code)]

use std::fs;
use std::io::{self, Read};
use std::str::FromStr;

use pico_args::Arguments;

/// Width of the attached terminal, if any.
#[cfg(not(target_arch = "wasm32"))]
fn terminal_width() -> Option<usize> {
    terminal_size::terminal_size().map(|(terminal_size::Width(w), _)| w as usize)
}

/// wasm32 has no terminal (and no terminal_size dependency). The CLI is
/// meaningless there, but cargo builds bin targets whenever integration
/// tests build — including tests/wasm_boundary.rs — so it must compile.
#[cfg(target_arch = "wasm32")]
fn terminal_width() -> Option<usize> {
    None
}

fn help_text() -> String {
    format!("\
Usage: tjson [OPTIONS] [-i FILE] [-o FILE]

Convert JSON to TJSON or TJSON to JSON.

Options:
  -t, --tjson                 Output TJSON from JSON input (default)
  -j, --json                  Output pretty JSON from TJSON input
  -i, --input FILE            Read from file instead of stdin
  -o, --output FILE           Write to file instead of stdout
      --[no-]final-newline    Enable/disable final newline (default: on)
  -V, --version               Show program version and exit

TJSON Output Formatting Options (for output TJSON only, not help/errors/JSON):
  General:
  -C, --canonical             One key-value pair per line, no inline packing,
                                no multiline strings, no tables, inf width,
                                otherwise default, other options can override
  -T                          Set wrap and table widths to terminal width
  -w, --width N               Wrap column, 0=unlimited, term=terminal width
                                (default: {})
      --eol VALUE             Output line ending: lf (default), crlf. Prefer lf,
                                use crlf only when a consumer truly requires it.
                                This option may become library-only not cli in
                                favor of platform tools like unix2dos.

  Value formatting:
      --force-markers         Force single-level [ and {{ markers for
                                nonempty arrays/objects (default: off)
      --[no-]inline           Enable/disable all inline packing (default: on)
      --[no-]inline-object    Enable/disable inline object packing (default: on)
      --[no-]inline-array     Enable/disable inline array packing (default: on)
      --bare-strings VALUE    Bare string policy: prefer, none (default: prefer)
      --bare-keys VALUE       Bare key policy: prefer, none (default: prefer)
      --string-array-style STYLE  String array packing: none, comma, spaces,
                                prefer-spaces, prefer-comma(default)
  -k, --kv-pack-multiple N    Spacing multiplier between packed KV pairs,
                                1-4, spaces = N*2 (default: 2) [experimental]

  Tables:
      --[no-]tables           Enable/disable pipe table rendering (default: on)
      --table-min-rows N      Minimum rows for a table (default: 3)
      --table-min-columns N   Minimum columns for a table (default: 3)
      --table-similarity N    Minimum key-similarity fraction (default: 0.8)
      --table-column-max-width N  Maximum column width in tables (default: 40)
      --table-fold            Enable / fold continuations for wide table rows
                                [experimental]
      --table-unindent-style STYLE
                                Table repositioning: left, auto, floating, none
                                (default: auto)
      --indent-glyph-style STYLE
                                When to use /< /> indent-offset glyphs:
                                auto, fixed, none (default: auto)

  Multiline strings:
      --[no-]multiline        Enable/disable multiline string rendering
                                (default: on)
      --multiline-style STYLE  Style: bold, floating, bold-floating, bold-light,
                                light, transparent, folding-quotes (default: bold)
      --multiline-min-lines N  Minimum EOL count for multiline (default: 1)
      --multiline-max-lines N  Maximum lines before floating falls back to bold,
                                 0=unlimited (default: 10)

  Folding:
      --fold STYLE            Set all fold styles: auto, fixed, none
                                (does not affect --table-fold)
      --fold-bare STYLE       Fold style for bare strings (default: auto)
      --fold-quoted STYLE     Fold style for quoted strings (default: auto)
      --fold-multiline STYLE  Fold style within multiline bodies (default: none)
      --fold-number STYLE     Fold style for numbers (default: auto)
", tjson::DEFAULT_WRAP_WIDTH)
}

fn parse_val<T>(args: &mut Arguments, flag: &'static str) -> Option<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    args.opt_value_from_str(flag).unwrap_or_else(|e| {
        eprintln!("tjson: {flag}: {e}");
        std::process::exit(1);
    })
}

fn main() {
    let mut args = Arguments::from_env();

    if args.contains(["-V", "--version"]) {
        println!("{}", version_text());
        return;
    }

    if args.contains(["-h", "--help"]) {
        print!("{}", help_text());
        return;
    }

    let flag_term        = args.contains("-T");
    let flag_json     = args.contains(["-j", "--json"]);
    let flag_termjson    = args.contains(["-t", "--tjson"]);
    let flag_canonical = args.contains(["-C", "--canonical"]);
    let flag_force_markers   = args.contains("--force-markers");
    let flag_inline          = args.contains("--inline");
    let flag_no_inline       = args.contains("--no-inline");
    let flag_inline_obj      = args.contains("--inline-object");
    let flag_no_inline_obj   = args.contains("--no-inline-object");
    let flag_inline_arr      = args.contains("--inline-array");
    let flag_no_inline_arr   = args.contains("--no-inline-array");
    let flag_termables          = args.contains("--tables");
    let flag_no_tables       = args.contains("--no-tables");
    let flag_termable_fold      = args.contains("--table-fold");
    let flag_multiline       = args.contains("--multiline");
    let flag_no_multiline    = args.contains("--no-multiline");
    let opt_table_unindent_style: Option<String> = parse_val(&mut args, "--table-unindent-style");
    let flag_final_newline    = args.contains("--final-newline");
    let flag_no_final_newline = args.contains("--no-final-newline");

    let opt_wrap_str:   Option<String> = parse_val(&mut args, "--width")
        .or_else(|| parse_val(&mut args, "-w"));
    let opt_wrap: Option<usize> = match opt_wrap_str.as_deref() {
        None => None,
        Some("term") => Some(terminal_width().unwrap_or_else(|| {
            eprintln!("tjson: --width term: no terminal detected, using 80 columns");
            80
        })),
        Some(s) => Some(s.parse::<usize>().unwrap_or_else(|_| {
            eprintln!("tjson: --width: invalid value '{s}' (expected a number or 'term')");
            std::process::exit(1);
        })),
    };
    let opt_input:      Option<String> = args.opt_value_from_str(["-i", "--input"]).unwrap_or_else(|e| {
        eprintln!("tjson: --input: {e}"); std::process::exit(1);
    });
    let opt_output:     Option<String> = args.opt_value_from_str(["-o", "--output"]).unwrap_or_else(|e| {
        eprintln!("tjson: --output: {e}"); std::process::exit(1);
    });
    let opt_bare_strings:    Option<String> = parse_val(&mut args, "--bare-strings");
    let opt_bare_keys:       Option<String> = parse_val(&mut args, "--bare-keys");
    let opt_string_array_style: Option<String> = parse_val(&mut args, "--string-array-style");
    let opt_table_min_rows:  Option<usize>  = parse_val(&mut args, "--table-min-rows");
    let opt_table_min_cols:  Option<usize>  = parse_val(&mut args, "--table-min-columns")
        .or_else(|| parse_val(&mut args, "--table-min-cols")); // compat alias + obvious typo — not in help
    let opt_table_min_similarity: Option<f32>   = parse_val(&mut args, "--table-similarity");
    let opt_table_col_max:   Option<usize>  = parse_val(&mut args, "--table-column-max-width");
    let opt_kv_pack_multiple: Option<usize> = parse_val(&mut args, "--kv-pack-multiple")
        .or_else(|| parse_val(&mut args, "-k"));
    let opt_indent_glyph_style: Option<String> = parse_val(&mut args, "--indent-glyph-style");
    let opt_multiline_style: Option<String> = parse_val(&mut args, "--multiline-style");
    let opt_multiline_min:   Option<usize>  = parse_val(&mut args, "--multiline-min-lines");
    let opt_multiline_max:   Option<usize>  = parse_val(&mut args, "--multiline-max-lines");
    let opt_fold:            Option<String> = parse_val(&mut args, "--fold");
    let opt_fold_bare:       Option<String> = parse_val(&mut args, "--fold-bare");
    let opt_fold_quoted:     Option<String> = parse_val(&mut args, "--fold-quoted");
    let opt_fold_multiline:  Option<String> = parse_val(&mut args, "--fold-multiline");
    let opt_fold_number:     Option<String> = parse_val(&mut args, "--fold-number");
    let opt_eol:             Option<String> = parse_val(&mut args, "--eol");

    // Check for unrecognised arguments
    let remaining = args.finish();
    if !remaining.is_empty() {
        for arg in &remaining {
            eprintln!("tjson: unrecognized argument: {}", arg.to_string_lossy());
        }
        std::process::exit(1);
    }

    if flag_json && flag_termjson {
        eprintln!("tjson: --json and --tjson are mutually exclusive");
        std::process::exit(1);
    }

    // --eol governs TJSON output line endings. On the CLI we deliberately keep JSON output at
    // LF (matching the library and canonical JSON) instead of applying --eol to it — the
    // pretty-printed JSON *has* line endings we could touch, we just choose not to. So pairing
    // --eol with JSON output is rejected loudly rather than silently ignored or emitting mixed
    // endings (see the note on finalize_output). --help/--version short-circuit above and never
    // reach here, so they ignore --eol like every other flag.
    //
    // TODO(0.7.0, breaking): the same logic applies to EVERY TJSON-output option (--width,
    // --bare-strings, --tables, --multiline-*, --fold-*, -C, -T, …): they shape TJSON rendering
    // and do nothing for JSON output, so they should all be rejected with -j and grouped under a
    // "TJSON OUTPUT OPTIONS" help heading. That flip is a breaking change — today those flags
    // accept-and-ignore *correctly* with -j — so batch it into a breaking release. --eol is
    // different: its only prior behavior with -j (in 0.6.6, hours old) was the buggy mixed-ending
    // output, so rejecting it is a bugfix that breaks nobody, not the loss of relied-upon behavior.
    if opt_eol.is_some() && flag_json {
        eprintln!(
            "tjson: --eol sets TJSON output line endings only; JSON output is always LF. \
             To change JSON line endings, use a line-ending conversion tool for your platform"
        );
        std::process::exit(1);
    }

    // Output line ending between TJSON output lines and the trailing newline. Defaults to LF.
    let eol: tjson::Eol = match opt_eol.as_deref() {
        None => tjson::Eol::Lf,
        Some(s) => s.parse().unwrap_or_else(|e| {
            eprintln!("tjson: --eol: {e}");
            std::process::exit(1);
        }),
    };

    let input = match &opt_input {
        Some(path) => fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("tjson: {path}: {e}");
            std::process::exit(1);
        }),
        None => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s).unwrap_or_else(|e| {
                eprintln!("tjson: {e}");
                std::process::exit(1);
            });
            s
        }
    };

    let result = if flag_json {
        // TJSON -> JSON
        input.parse::<tjson::Value>()
            .and_then(|v| serde_json::to_string_pretty(&serde_json::Value::from(v)).map_err(tjson::Error::from))
    } else {
        let mut opts = if flag_canonical {
            tjson::RenderOptions::canonical()
        } else {
            tjson::RenderOptions::default()
        };

        // -T: terminal width baseline — applied first so explicit flags override
        if flag_term {
            let tw = terminal_width().unwrap_or_else(|| {
                eprintln!("tjson: -T: no terminal detected, using 80 columns");
                80
            });
            opts = opts.wrap_width(Some(tw));
            if tw / 2 > 40 {
                opts = opts.table_column_max_width(Some(tw / 2));
            }
        }

        // Switches — general first, specific overrides after
        if flag_force_markers                      { opts = opts.force_markers(true); }
        if flag_no_inline    || flag_no_inline_obj { opts = opts.inline_objects(false); }
        if flag_inline       || flag_inline_obj    { opts = opts.inline_objects(true); }
        if flag_no_inline    || flag_no_inline_arr { opts = opts.inline_arrays(false); }
        if flag_inline       || flag_inline_arr    { opts = opts.inline_arrays(true); }
        if flag_no_tables                          { opts = opts.tables(false); }
        if flag_termables                             { opts = opts.tables(true); }
        if flag_termable_fold                         { opts = opts.table_fold(true); }
        if let Some(v) = opt_table_unindent_style.as_deref().map(|s| s.parse::<tjson::TableUnindentStyle>().unwrap_or_else(|e| { eprintln!("tjson: --table-unindent-style: {e}"); std::process::exit(1); })) {
            opts = opts.table_unindent_style(v);
        }
        if flag_no_multiline                       { opts = opts.multiline_strings(false); }
        if flag_multiline                          { opts = opts.multiline_strings(true); }

        // Options
        if let Some(w) = opt_wrap {
            if w == 0 {
                opts = opts.wrap_width(None);
            } else if w < tjson::MIN_WRAP_WIDTH {
                eprintln!("tjson: --width {w} is too narrow (minimum {}); using {}", tjson::MIN_WRAP_WIDTH, tjson::MIN_WRAP_WIDTH);
                opts = opts.wrap_width(Some(tjson::MIN_WRAP_WIDTH));
            } else {
                opts = opts.wrap_width(Some(w));
            }
        }
        if let Some(v) = opt_bare_strings.as_deref().map(|s| s.parse::<tjson::BareStyle>().unwrap_or_else(|e| { eprintln!("tjson: --bare-strings: {e}"); std::process::exit(1); })) {
            opts = opts.bare_strings(v);
        }
        if let Some(v) = opt_bare_keys.as_deref().map(|s| s.parse::<tjson::BareStyle>().unwrap_or_else(|e| { eprintln!("tjson: --bare-keys: {e}"); std::process::exit(1); })) {
            opts = opts.bare_keys(v);
        }
        if let Some(v) = opt_string_array_style.as_deref().map(|s| s.parse::<tjson::StringArrayStyle>().unwrap_or_else(|e| { eprintln!("tjson: --string-array-style: {e}"); std::process::exit(1); })) {
            opts = opts.string_array_style(v);
        }
        if let Some(v) = opt_table_min_rows   { opts = opts.table_min_rows(v); }
        if let Some(v) = opt_table_min_cols   { opts = opts.table_min_columns(v); }
        if let Some(v) = opt_table_min_similarity { opts = opts.table_min_similarity(v); }
        if let Some(v) = opt_table_col_max    { opts = opts.table_column_max_width(if v == 0 { None } else { Some(v) }); }
        if let Some(v) = opt_kv_pack_multiple {
            opts = opts.kv_pack_multiple(v).unwrap_or_else(|e| { eprintln!("tjson: --kv-pack-multiple: {e}"); std::process::exit(1); });
        }
        if let Some(v) = opt_indent_glyph_style.as_deref().map(|s| s.parse::<tjson::IndentGlyphStyle>().unwrap_or_else(|e| { eprintln!("tjson: --indent-glyph-style: {e}"); std::process::exit(1); })) {
            opts = opts.indent_glyph_style(v);
        }
        if let Some(v) = opt_multiline_style.as_deref().map(|s| s.parse::<tjson::MultilineStyle>().unwrap_or_else(|e| { eprintln!("tjson: --multiline-style: {e}"); std::process::exit(1); })) {
            opts = opts.multiline_style(v);
        }
        if let Some(v) = opt_multiline_min    { opts = opts.multiline_min_lines(v); }
        if let Some(v) = opt_multiline_max    { opts = opts.multiline_max_lines(v); }
        // --fold sets all four; per-type flags override (more specific wins)
        if let Some(v) = opt_fold.as_deref().map(|s| s.parse::<tjson::FoldStyle>().unwrap_or_else(|e| { eprintln!("tjson: --fold: {e}"); std::process::exit(1); })) {
            opts = opts.fold(v);
        }
        if let Some(v) = opt_fold_bare.as_deref().map(|s| s.parse::<tjson::FoldStyle>().unwrap_or_else(|e| { eprintln!("tjson: --fold-bare: {e}"); std::process::exit(1); })) {
            opts = opts.string_bare_fold_style(v);
        }
        if let Some(v) = opt_fold_quoted.as_deref().map(|s| s.parse::<tjson::FoldStyle>().unwrap_or_else(|e| { eprintln!("tjson: --fold-quoted: {e}"); std::process::exit(1); })) {
            opts = opts.string_quoted_fold_style(v);
        }
        if let Some(v) = opt_fold_multiline.as_deref().map(|s| s.parse::<tjson::FoldStyle>().unwrap_or_else(|e| { eprintln!("tjson: --fold-multiline: {e}"); std::process::exit(1); })) {
            opts = opts.string_multiline_fold_style(v);
        }
        if let Some(v) = opt_fold_number.as_deref().map(|s| s.parse::<tjson::FoldStyle>().unwrap_or_else(|e| { eprintln!("tjson: --fold-number: {e}"); std::process::exit(1); })) {
            opts = opts.number_fold_style(v);
        }
        opts = opts.eol(eol);

        // JSON -> TJSON (default)
        serde_json::from_str::<serde_json::Value>(&input)
            .map_err(tjson::Error::from)
            .map(tjson::Value::from)
            .map(|v| v.to_tjson_with(opts))
    };

    let output_str = result.unwrap_or_else(|e| {
        eprintln!("tjson: {e}");
        std::process::exit(1);
    });

    // --final-newline overrides --no-final-newline (more specific wins)
    let add_final_newline = if flag_final_newline { true } else { !flag_no_final_newline };
    let output_str = finalize_output(output_str, add_final_newline, eol);

    match &opt_output {
        Some(path) => fs::write(path, output_str).unwrap_or_else(|e| {
            eprintln!("tjson: {path}: {e}");
            std::process::exit(1);
        }),
        None => {
            use std::io::Write;
            if let Err(e) = std::io::stdout().write_all(output_str.as_bytes()) {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
                eprintln!("tjson: {e}");
                std::process::exit(1);
            }
        }
    }
}

// Appends the trailing newline — the "additional" line ending after the last content line.
// This terminator MUST match the line endings already in `output`'s body, or the document
// ends with mixed endings. TJSON output is rendered with this same `eol`, so they agree.
// JSON output is always LF-bodied (serde_json), which is exactly why `--eol` is rejected for
// JSON upstream: if that guard is ever loosened, appending a CRLF terminator onto an LF JSON
// body here is the mixed-ending bug. Keep body-eol and this trailing-eol in agreement.
fn finalize_output(mut output: String, add_final_newline: bool, eol: tjson::Eol) -> String {
    if add_final_newline && !output.ends_with('\n') {
        output.push_str(eol.as_str());
    }
    output
}

fn version_text() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{finalize_output, version_text};
    use tjson::Eol;

    #[test]
    fn adds_final_newline_by_default() {
        assert_eq!(finalize_output("abc".to_string(), true, Eol::Lf), "abc\n");
    }

    #[test]
    fn does_not_double_existing_final_newline() {
        assert_eq!(finalize_output("abc\n".to_string(), true, Eol::Lf), "abc\n");
    }

    #[test]
    fn can_suppress_final_newline() {
        assert_eq!(finalize_output("abc".to_string(), false, Eol::Lf), "abc");
    }

    #[test]
    fn adds_crlf_final_newline_when_eol_is_crlf() {
        assert_eq!(finalize_output("abc".to_string(), true, Eol::CrLf), "abc\r\n");
    }

    #[test]
    fn reports_program_version() {
        assert_eq!(version_text(), format!("tjson-rs {}", env!("CARGO_PKG_VERSION")));
    }
}
