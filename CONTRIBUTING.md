# Contributing to tjson-rs

## Supported Build Targets

All of these must continue to work:

| Target | Command |
|--------|---------|
| Native (default) | `cargo build` |
| Native library only | `cargo build --lib` |
| musl (static binary) | `cargo build --target x86_64-unknown-linux-musl` |
| WASM | `wasm-pack build --target bundler` |

## Building

```sh
cargo build
```

For a static musl binary (requires the musl target: `rustup target add x86_64-unknown-linux-musl`):

```sh
cargo build --target x86_64-unknown-linux-musl
```

## Running Tests

The test suite requires the [tjson-tests](https://github.com/rfanth/tjson-tests) fixture repository, which is included as a git submodule at `tests/fixtures/`.

Clone the repo with submodules:

```sh
git clone --recurse-submodules https://github.com/rfanth/tjson.git
```

Or if you already cloned without submodules:

```sh
git submodule update --init
```

Then run the tests:

```sh
cargo test
```

Alternatively, if you have the test fixtures elsewhere, set `TJSON_TESTS_DIR`:

```sh
TJSON_TESTS_DIR=/path/to/tjson-tests cargo test
```

## Building the WASM / npm Package

Requires [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```sh
cargo install wasm-pack
wasm-pack build --target bundler
```
