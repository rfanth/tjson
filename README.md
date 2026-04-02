# tjson-rs

A Rust library and CLI tool for [TJSON](https://textjson.com)

TJSON is a hyper-readable, round trip safe and data preserving substitute for JSON that feels like text and represents the same data while looking quite different and allowing different generator rules to optimize readibility.  It is not a superset or a subset of JSON.  It's commonality is that it represents the same underlying data, not the format itself.  It's position based to emphasize locality of meaning, and adds bare strings, pipe tables, comments, multiline string literals, and line folding to make the contained data easier to read while remaining fully convertible to and from standard JSON without data loss.  TJSON is optimized for reading and deterministic data, not human editing.

Usage as a binary, library (including WASM too), and through serde Serialize are all fully supported.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
tjson = "0.3"
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

// Canonical (one key per line, no packing)
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
import { parse, stringify } from '@rfanth/tjson';

// Render JSON as TJSON
const tjson = stringify('{"name":"Alice","scores":[1,2,3]}');

// With options (camelCase for WASM/JS only)
const canonical = stringify('{"name":"Alice"}', { canonical: true });
const narrow = stringify('{"name":"Alice"}', { wrapWidth: 40, stringArrayStyle: 'preferSpaces' });

// Parse TJSON back to a JSON string
const json = parse('  name: Alice');
```


## Resources

- Website and online demo: [textjson.com](https://textjson.com)
- Specification: [tjson-specification.md](https://github.com/rfanth/tjson-spec/blob/master/tjson-specification.md)
- Test suite: [tjson-tests](https://github.com/rfanth/tjson-tests)

## License

BSD-3-Clause. See [LICENSE](LICENSE).
