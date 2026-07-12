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

```sh
npm install @rfanth/tjson
```

Or with no build step at all — the `/web` entry is self-contained (wasm
inlined, initialized during import, nothing to call or configure) and works
from any CDN that serves npm packages as files:

```js
import { parse, stringify, fromJson, toJson } from 'https://unpkg.com/@rfanth/tjson/web/index.js';
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

### Two entries, same API

- **`@rfanth/tjson`** (the package root) — the lean build for bundlers
  (webpack, Vite with a wasm plugin) and Node. The wasm ships as a separate
  file and is instantiated by your toolchain.
- **`@rfanth/tjson/web`** — the zero-setup build: wasm inlined, initialized
  by top-level await during import. Works in browsers straight off a CDN,
  in bundlers without wasm plugins, and in server runtimes — at the cost of
  a larger payload and no streaming instantiation. If the root entry fights
  your toolchain, this one won't.

Both expose the identical four functions with identical behavior.

## Options

`stringify` and `fromJson` accept an optional [`StringifyOptions`](https://github.com/rfanth/tjson/blob/master/src/wasm.rs) object. TypeScript users get full autocomplete and inline documentation for all options — hover over any field in your editor.

Options use **camelCase** in JavaScript. The underlying library's Rust API uses snake_case and idiomatic Rust, but exposes the same options.

Options must be a plain object; `null`/`undefined` mean defaults, and anything else (an array, a number, a class instance) throws. Note that the options bag rides the same value pipeline as data, so an options object with a `toJSON()` method is converted by it first — pass a plain literal. Unknown fields are ignored — TypeScript catches misspelled names at compile time — except option names that were renamed or removed in a past release, which throw with a hint pointing at the replacement.

## Value handling

`stringify` follows `JSON.stringify` semantics: `toJSON()` methods are honored
(a `Date` serializes as its ISO string), object keys with `undefined` values
are omitted, and key order is preserved. Values with **no** JSON form fail
loudly instead of silently serializing as junk: `Map`, `Set`, class instances
without `toJSON`, `NaN`/`Infinity`, and strings
containing unpaired surrogates (usually a sign of a string truncated
mid-character).

**Exact big integers.** `BigInt` values serialize as exact JSON numbers
(requires `JSON.rawJSON` — Node 21+ / modern browsers; older runtimes throw
rather than corrupt). On the way back, integers beyond
`Number.MAX_SAFE_INTEGER` throw by default — a plain JS number cannot hold
them exactly — or are revived as `BigInt` from the exact source digits with
`parse(text, { bigints: true })`. The string-to-string functions (`toJson`,
`fromJson`) carry numbers of any size exactly with no options needed. Note
that JS has no exact decimal type, so non-integer numbers are always `number`
(f64) on the JS side.

**Key options:**

| Option | Default | Description |
|---|---|---|
| `canonical` | `false` | One key per line, no packing, no tables |
| `wrapWidth` | `80` | Column wrap limit; `0` for unlimited |
| `tables` | `true` | Render arrays-of-objects as pipe tables |
| `multilineStrings` | `true` | Use `\`\`` blocks for strings containing newlines |
| `inlineObjects` | `true` | Pack multiple key-value pairs onto one line |
| `inlineArrays` | `true` | Pack multiple array items onto one line |
| `stringArrayStyle` | `"preferComma"` | How to pack all-string arrays |

**Advanced options:**

| Option | Default | Description |
|---|---|---|
| `bareStrings` | `"prefer"` | Use bare (unquoted) string values when spec permits |
| `bareKeys` | `"prefer"` | Use bare (unquoted) object keys when spec permits |
| `forceMarkers` | `false` | Force explicit `[` / `{` indent markers on single-step indents |
| `multilineStyle` | `"bold"` | Multiline block style (`"bold"`, `"floating"`, `"light"`, etc.) |
| `multilineMinLines` | `1` | Min newlines in a string before using a multiline block |
| `indentGlyphStyle` | `"auto"` | When to wrap deeply nested content in `/<` `/>` glyphs |
| `indentGlyphMarkerStyle` | `"compact"` | Where to place the opening `/<` glyph |
| `tableUnindentStyle` | `"auto"` | How to reposition wide tables toward the left margin |
| `tableMinRows` | `3` | Min rows required to render a table |
| `tableMinColumns` | `3` | Min columns required to render a table |
| `tableMinSimilarity` | `0.8` | Min fraction of rows sharing a column |
| `tableColumnMaxWidth` | `40` | Bail on table if any column exceeds this width |
| `fold` | — | Set all four fold styles at once; more specific options override |
| `numberFoldStyle` | `"auto"` | How to fold long numbers across lines |
| `stringBareFoldStyle` | `"auto"` | How to fold long bare strings |
| `stringQuotedFoldStyle` | `"auto"` | How to fold long quoted strings |
| `stringMultilineFoldStyle` | `"none"` | How to fold multiline block continuation lines |
| `eol` | `"lf"` | Line ending between output lines (`"lf"` or `"crlf"`) |

**Experimental options** (may change or be removed in a future version):

| Option | Default | Description |
|---|---|---|
| `kvPackMultiple` | `2` | Spacing multiplier between packed key-value pairs (1–4; spaces = value × 2) |
| `multilineMaxLines` | `10` | Max lines in a `"floating"` block before falling back to `"bold"` |
| `tableFold` | `false` | Fold long table rows across continuation lines |

Full option reference with inline documentation is in the TypeScript types bundled with the package.

## Resources

- **Website and live demo**: [textjson.com](https://textjson.com)
- **Test suite**: [tjson-tests](https://github.com/rfanth/tjson-tests)
- **Rust/WASM crate**: [tjson-rs](https://crates.io/crates/tjson-rs) — same options, snake_case API
- **MariaDB/MySQL UDF**: [tjson-udf](https://github.com/rfanth/tjson-udf) — same options in SQL
- **Specification**: [tjson-specification.md](https://github.com/rfanth/tjson-spec/blob/master/tjson-specification.md) —
  The spec is versioned independently from this implementation: each release
  is written against the spec as published at release time (the two are
  typically released together when the spec behavior changes).

## License

BSD-3-Clause
