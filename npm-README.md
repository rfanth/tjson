# @rfanth/tjson

JavaScript/TypeScript bindings for [TJSON](https://textjson.com) — a readable, round-trip-safe alternative to JSON.

TJSON represents the same data model as JSON but renders it in a way that feels like text: bare strings and keys, pipe tables for arrays of objects, multiline string literals, line folding, and comments. It is not a superset or subset of JSON — it is a different surface syntax for the same underlying data, fully convertible in both directions without data loss.

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
  name: Alice  age:30  active:true
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

```sh
npm install @rfanth/tjson
```

Or via CDN (no bundler needed):

```js
import { parse, stringify, fromJson, toJson } from 'https://esm.sh/@rfanth/tjson';
```

## Usage

```ts
import { parse, stringify, fromJson, toJson } from '@rfanth/tjson';

// JS value → TJSON
const tjson = stringify({ name: 'Alice', scores: [95, 87, 92] });

// TJSON → JS value
const value = parse(tjson);

// With options
const canonical = stringify({ name: 'Alice' }, { canonical: true });
const narrow    = stringify({ name: 'Alice' }, { wrapWidth: 40, stringArrayStyle: 'preferSpaces' });

// String pipeline variants (if you already have a JSON string)
const tjson2 = fromJson('{"name":"Alice"}');
const json   = toJson(tjson2);
```

`stringify` accepts any JSON-serializable JS value and returns a TJSON string.
`parse` accepts a TJSON string and returns a JS value — just like `JSON.parse`.
`fromJson`/`toJson` are the string-in/string-out variants for JSON string pipelines.

All four functions throw an `Error` on invalid input.

## Options

`stringify` and `fromJson` accept an optional [`StringifyOptions`](https://github.com/rfanth/tjson/blob/master/src/wasm.rs) object. TypeScript users get full autocomplete and inline documentation for all options — hover over any field in your editor.

Options use **camelCase** in JavaScript. The underlying library's Rust API uses snake_case and idiomatic Rust, but exposes the same options.

Key options at a glance:

| Option | Default | Description |
|---|---|---|
| `canonical` | `false` | One key per line, no packing, no tables |
| `wrapWidth` | `80` | Column wrap limit; `0` for unlimited |
| `tables` | `true` | Render arrays-of-objects as pipe tables |
| `multilineStrings` | `true` | Use `\`\`` blocks for strings containing newlines |
| `bareStrings` | `"prefer"` | Use bare (unquoted) string values when spec permits |
| `bareKeys` | `"prefer"` | Use bare (unquoted) object keys when spec permits |
| `inlineObjects` | `true` | Pack multiple key-value pairs onto one line |
| `inlineArrays` | `true` | Pack multiple array items onto one line |
| `stringArrayStyle` | `"preferComma"` | How to pack all-string arrays |
| `indentGlyphStyle` | `"auto"` | When to emit `/<` `/>` glyphs for deeply nested content |
| `forceMarkers` | `false` | Always emit `=` / `:` markers, even when inferrable |
| `multilineStyle` | `"bold"` | Multiline block style (`"bold"`, `"floating"`, etc.) |
| `multilineMinLines` | `1` | Min newlines in a string before using a block |
| `multilineMaxLines` | `10` | Max lines in a block before falling back |
| `tableFold` | `false` | Fold long table rows across continuation lines |
| `tableUnindentStyle` | `"auto"` | How to reposition wide tables using indent glyphs |
| `tableMinRows` | `3` | Min rows required to render a table |
| `tableMinCols` | `3` | Min columns required to render a table |
| `tableMinSimilarity` | `0.8` | Min fraction of rows sharing a column |
| `tableColumnMaxWidth` | `40` | Bail on table if any column exceeds this width |
| `fold` | — | Set all four fold styles at once; more specific options override |
| `numberFoldStyle` | `"auto"` | How to fold long numbers across lines |
| `stringBareFoldStyle` | `"auto"` | How to fold long bare strings |
| `stringQuotedFoldStyle` | `"auto"` | How to fold long quoted strings |
| `stringMultilineFoldStyle` | `"none"` | How to fold multiline block continuation lines |
| `indentGlyphMarkerStyle` | `"compact"` | Where to place the opening `/<` glyph |

Full option reference with documentation is in the TypeScript types bundled with the package.

## Resources

- **Website and live demo**: [textjson.com](https://textjson.com)
- **Specification**: [tjson-specification.md](https://github.com/rfanth/tjson-spec/blob/master/tjson-specification.md)
- **Test suite**: [tjson-tests](https://github.com/rfanth/tjson-tests)
- **Rust/WASM crate**: [tjson-rs](https://crates.io/crates/tjson-rs) — same options, snake_case API
- **MariaDB/MySQL UDF**: [tjson-udf](https://github.com/rfanth/tjson-udf) — same options in SQL

## License

BSD-3-Clause
