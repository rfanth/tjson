// created by R.F. Anthracite <rfa@rfanth.com>

use std::fs;
use std::io::{self, Read};

use argh::FromArgs;

/// Convert JSON to TJSON, reading from stdin and writing to stdout.
/// Created by R.F. Anthracite (rfa@rfanth.com)
#[derive(FromArgs)]
struct Args {
    /// show program version and exit
    #[argh(switch, short = 'V')]
    version: bool,

    /// output JSON from TJSON input
    #[argh(switch, short = 'j')]
    json: bool,

    /// output TJSON from JSON input (default)
    #[argh(switch, short = 't')]
    tjson: bool,

    /// one key-value pair per line, no inline packing, no tables
    #[argh(switch, short = 'C')]
    canonical: bool,

    /// force single-level [ and { markers for nonempty arrays/objects
    #[argh(switch)]
    force_markers: bool,

    /// disable all inline packing (objects + arrays), tables still on
    #[argh(switch)]
    no_inline: bool,

    /// disable inline object key packing only
    #[argh(switch)]
    no_inline_object: bool,

    /// disable inline array packing only
    #[argh(switch)]
    no_inline_array: bool,

    /// bare string emission policy: prefer, none
    #[argh(option, default = "\"prefer\".to_owned()")]
    bare_strings: String,

    /// bare key emission policy: prefer, none
    #[argh(option, default = "\"prefer\".to_owned()")]
    bare_keys: String,

    /// string-only array packing style: spaces, prefer-spaces, comma, prefer-comma, none
    #[argh(option, default = "\"prefer-comma\".to_owned()")]
    string_array_style: String,

    /// override wrap column (default 80, 0=unlimited)
    #[argh(option, short = 'w', default = "80")]
    wrap: usize,

    /// disable pipe table rendering
    #[argh(switch)]
    no_tables: bool,

    /// minimum rows for a table (default 3)
    #[argh(option, default = "3")]
    table_min_rows: usize,

    /// minimum columns for a table (default 3)
    #[argh(option, default = "3")]
    table_min_cols: usize,

    /// minimum key-similarity fraction (default 0.8)
    #[argh(option, default = "0.8")]
    table_similarity: f32,

    /// suppress the final newline normally added after all output
    #[argh(switch)]
    no_final_newline: bool,

    /// read from file instead of stdin
    #[argh(option, short = 'i')]
    input: Option<String>,

    /// write to file instead of stdout
    #[argh(option, short = 'o')]
    output: Option<String>,
}

fn main() {
    let args: Args = argh::from_env();

    if args.version {
        println!("{}", version_text());
        return;
    }

    if args.json && args.tjson {
        eprintln!("tjson: --json and --tjson are mutually exclusive");
        std::process::exit(1);
    }

    let string_array_style = args
        .string_array_style
        .parse::<tjson::StringArrayStyle>()
        .unwrap_or_else(|e| {
            eprintln!("tjson: {}", e);
            std::process::exit(1);
        });
    let bare_strings = args
        .bare_strings
        .parse::<tjson::BareStyle>()
        .unwrap_or_else(|e| {
            eprintln!("tjson: {}", e);
            std::process::exit(1);
        });
    let bare_keys = args
        .bare_keys
        .parse::<tjson::BareStyle>()
        .unwrap_or_else(|e| {
            eprintln!("tjson: {}", e);
            std::process::exit(1);
        });

    let input = match &args.input {
        Some(path) => fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("tjson: {}: {}", path, e);
            std::process::exit(1);
        }),
        None => {
            let mut s = String::new();
            io::stdin().read_to_string(&mut s).unwrap_or_else(|e| {
                eprintln!("tjson: {}", e);
                std::process::exit(1);
            });
            s
        }
    };

    let result = if args.json {
        // TJSON -> JSON
        tjson::parse_str(&input)
            .and_then(|v| v.to_json_value_lossy())
            .map(|v| serde_json::to_string_pretty(&v).expect("json serialization failed"))
    } else {
        let render_options = tjson::RenderOptions {
            canonical: args.canonical,
            force_markers: args.force_markers,
            bare_strings,
            bare_keys,
            inline_objects: !(args.no_inline || args.no_inline_object),
            inline_arrays: !(args.no_inline || args.no_inline_array),
            string_array_style,
            tables: !args.no_tables,
            wrap_width: if args.wrap == 0 {
                None
            } else {
                Some(args.wrap)
            },
            table_min_rows: args.table_min_rows,
            table_min_cols: args.table_min_cols,
            table_similarity: args.table_similarity,
            ..tjson::RenderOptions::default()
        };

        // JSON -> TJSON (default)
        serde_json::from_str(&input)
            .map_err(tjson::Error::from)
            .map(tjson::TjsonValue::from_json_value)
            .and_then(|v| tjson::render_string_with_options(&v, render_options))
    };

    let output_str = result.unwrap_or_else(|e| {
        eprintln!("tjson: {}", e);
        std::process::exit(1);
    });

    let output_str = finalize_output(output_str, !args.no_final_newline);

    match &args.output {
        Some(path) => fs::write(path, output_str).unwrap_or_else(|e| {
            eprintln!("tjson: {}: {}", path, e);
            std::process::exit(1);
        }),
        None => print!("{}", output_str),
    }
}

fn finalize_output(mut output: String, add_final_newline: bool) -> String {
    if add_final_newline && !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn version_text() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::{finalize_output, version_text};

    #[test]
    fn adds_final_newline_by_default() {
        assert_eq!(finalize_output("abc".to_string(), true), "abc\n");
    }

    #[test]
    fn does_not_double_existing_final_newline() {
        assert_eq!(finalize_output("abc\n".to_string(), true), "abc\n");
    }

    #[test]
    fn can_suppress_final_newline() {
        assert_eq!(finalize_output("abc".to_string(), false), "abc");
    }

    #[test]
    fn reports_program_version() {
        assert_eq!(version_text(), "tjson 0.1.0");
    }
}
