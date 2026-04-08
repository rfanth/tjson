# tjson-rs

A Rust library and CLI tool for [TJSON](https://textjson.com)

TJSON is a hyper-readable, round trip safe and data preserving substitute for JSON that feels like text and represents the same data while looking quite different and allowing different generator rules to optimize readability.  It is not a superset or a subset of JSON, but it does represent the same underlying data in a different format.  It's position based to emphasize locality of meaning, and adds bare strings, pipe tables, comments, multiline string literals, and line folding to make the contained data easier to read while remaining fully convertible to and from standard JSON while retaining exactly the same data.  TJSON is optimized for reading and deterministic data, not human editing.

Usage as a binary, library (including WASM too), and through serde Serialize are all fully supported.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
tjson = { package = "tjson-rs", version = "0.4" }
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

Install:

```sh
npm install @rfanth/tjson
```

Usage:

```js
import { parse, stringify, toJson, fromJson } from '@rfanth/tjson';

// Render JSON as TJSON with or without options
const tjsonString = fromJson('{"name":"Alice","scores":[1,2,3]}');
const tjsonStringNarrow = fromJson('{"name":"Alice","scores":[1,2,3]}', { wrapWidth: 40 });

// Parse TJSON to a JSON string
const jsonString = toJson(tjsonString);

// Render from a native javascript object with or without options (camelCase for WASM/JS only)
const defaultTjson = stringify({name: "Alice"});
const canonicalTjson = stringify({name: "Alice"}, { canonical: true });
const noFoldTjson = stringify({name: "Alice"}, { fold: "none", stringArrayStyle: 'preferSpaces' });

// Parse TJSON to a native javascript value
const jsObject = parse('  name: Alice');

```


## Resources

- Website and online demo: [textjson.com](https://textjson.com)
- Specification: [tjson-specification.md](https://github.com/rfanth/tjson-spec/blob/master/tjson-specification.md)
- Test suite: [tjson-tests](https://github.com/rfanth/tjson-tests)

## License

BSD-3-Clause. See [LICENSE](LICENSE).
