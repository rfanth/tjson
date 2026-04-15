# tjson-rs

A Rust library and CLI tool for [TJSON](https://textjson.com)

TJSON is a hyper-readable, round trip safe and data preserving substitute for JSON that feels like text and represents the same data while looking quite different and allowing different generator rules to optimize readability.  It is not a superset or a subset of JSON, but it does represent the same underlying data in a different format.  It's position based to emphasize locality of meaning, and adds bare strings, pipe tables, comments, multiline string literals, and line folding to make the contained data easier to read while remaining fully convertible to and from standard JSON while retaining exactly the same data.  TJSON is optimized for reading and deterministic data, not human editing.

Usage as a binary, library (including WASM too), and through serde Serialize are all fully supported.

**Input JSON**
```json
{
  "name": "Alice",
  "age": 30,
  "active": true,
  "bio": "She is a developer.\nShe loves Rust.",
  "scores": [90, 85, 92],
  "tags": ["rust", "wasm", "json", "serialization"],
  "team": [
    {"name": "Alice", "age": 30, "role": "admin"},
    {"name": "Bob",   "age": 25, "role": "user"},
    {"name": "Carol", "age": 35, "role": "user"}
  ]
}
```

**TJSON output**
```
  name: Alice    age:30    active:true
  bio: ``
| She is a developer.
| She loves Rust.
   ``
  scores:  90, 85, 92
  tags:   rust,  wasm,  json,  serialization
  team:
    |name    |age  |role    |
    | Alice  |30   | admin  |
    | Bob    |25   | user   |
    | Carol  |35   | user   |
```

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
tjson = { package = "tjson-rs", version = "0.5" }
```

Install the CLI:

```sh
cargo install tjson-rs
```

## Library Usage

### Parse TJSON

```rust
use tjson::TjsonValue;

// Parse a TJSON object (keys indented 2 spaces at the top level)
let value: TjsonValue = "  name: Alice\n  age:30".parse()?;

// Parse a bare string
let value: TjsonValue = " hello world".parse()?;
```

### Render to TJSON

```rust
use tjson::{TjsonValue, TjsonOptions};

let value = TjsonValue::from(serde_json::json!({"name": "Alice", "age": 30}));

// Default options
let tjson = value.to_tjson_with(TjsonOptions::default())?;

// Canonical (one key per line, no packing, see docs for details)
let canonical = value.to_tjson_with(TjsonOptions::canonical())?;
```

### Serde integration

```rust
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Person {
    name: String,
    age: u32,
}

// Deserialize from TJSON
let person: Person = tjson::from_str("  name: Alice\n  age:30")?;

// Serialize to TJSON
let tjson = tjson::to_string(&person)?;
```

## Options

`TjsonOptions` is a builder. All methods take `self` and return `Self`:

```rust
let opts = TjsonOptions::default()
    .wrap_width(Some(60))
    .tables(false)
    .multiline_strings(false);

let tjson = tjson::to_string_with(&value, opts)?;
```

**Key options:**

| Option | Default | Description |
|---|---|---|
| `canonical()` | `false` | One key per line, no packing, no tables |
| `wrap_width(Option<usize>)` | `Some(80)` | Column wrap limit, clamped to >= 20; `None` for unlimited |
| `tables(bool)` | `true` | Render arrays-of-objects as pipe tables |
| `multiline_strings(bool)` | `true` | Use ` `` ` blocks for strings containing newlines |
| `inline_objects(bool)` | `true` | Pack multiple key-value pairs onto one line |
| `inline_arrays(bool)` | `true` | Pack multiple array items onto one line |
| `string_array_style(StringArrayStyle)` | `PreferComma` | How to pack all-string arrays |

**Advanced options:**

| Option | Default | Description |
|---|---|---|
| `bare_strings(BareStyle)` | `Prefer` | Use bare (unquoted) string values when spec permits |
| `bare_keys(BareStyle)` | `Prefer` | Use bare (unquoted) object keys when spec permits |
| `force_markers(bool)` | `false` | Force explicit `[` / `{` indent markers on single-step indents |
| `multiline_style(MultilineStyle)` | `Bold` | Multiline block style (`Bold`, `Floating`, `Light`, etc.) |
| `multiline_min_lines(usize)` | `1` | Min newlines in a string before using a multiline block |
| `indent_glyph_style(IndentGlyphStyle)` | `Auto` | When to wrap deeply nested content in `/<` `/>` glyphs |
| `indent_glyph_marker_style(IndentGlyphMarkerStyle)` | `Compact` | Where to place the opening `/<` glyph |
| `table_unindent_style(TableUnindentStyle)` | `Auto` | How to reposition wide tables toward the left margin |
| `table_min_rows(usize)` | `3` | Min rows required to render a table |
| `table_min_columns(usize)` | `3` | Min columns required to render a table |
| `table_min_similarity(f32)` | `0.8` | Min fraction of rows sharing a column |
| `table_column_max_width(Option<usize>)` | `Some(40)` | Bail on table if any column exceeds this width |
| `fold(FoldStyle)` | — | Set all four fold styles at once; individual options override |
| `number_fold_style(FoldStyle)` | `Auto` | How to fold long numbers across lines |
| `string_bare_fold_style(FoldStyle)` | `Auto` | How to fold long bare strings |
| `string_quoted_fold_style(FoldStyle)` | `Auto` | How to fold long quoted strings |
| `string_multiline_fold_style(FoldStyle)` | `None` | How to fold multiline block continuation lines |

**Experimental options** (may change or be removed in a future version):

| Option | Default | Description |
|---|---|---|
| `kv_pack_multiple(usize)` | `2` | Spacing multiplier between packed key-value pairs (1–4; spaces = value × 2) |
| `multiline_max_lines(usize)` | `10` | Max lines in a `Floating` block before falling back to `Bold` |
| `table_fold(bool)` | `false` | Fold long table rows across continuation lines |

## CLI Usage

```sh
# JSON to TJSON
echo '{"name":"Alice","scores":[1,2,3]}' | tjson

# TJSON to JSON
echo '  name: Alice' | tjson --json

# From/to files
tjson -i data.json -o data.tjson
tjson --json -i data.tjson -o data.json

# Canonical output
tjson --canonical -i data.json
```

## WASM / JavaScript

This crate also compiles to WebAssembly. The npm package `@rfanth/tjson` wraps it with a JavaScript/TypeScript API (camelCase options, full TypeScript types). See the [npm README](npm-README.md) for usage and options.

## Resources

- **Website and live demo**: [textjson.com](https://textjson.com)
- **Specification**: [tjson-specification.md](https://github.com/rfanth/tjson-spec/blob/master/tjson-specification.md)
- **Test suite**: [tjson-tests](https://github.com/rfanth/tjson-tests)
- **npm package**: [@rfanth/tjson](https://www.npmjs.com/package/@rfanth/tjson) — JavaScript/TypeScript bindings
- **MariaDB/MySQL UDF**: [tjson-udf](https://github.com/rfanth/tjson-udf) — same options in SQL

## License

BSD-3-Clause. See [LICENSE](LICENSE).
