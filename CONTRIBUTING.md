# Contributing to tjson-rs

## Supported Build Targets

All of these must continue to work:

| Target | Command |
|--------|---------|
| Native (default) | `cargo build` |
| Native library only | `cargo build --lib` |
| Library without serde_json feature | `cargo build --lib --no-default-features` |
| musl (static binary) | `cargo build --target x86_64-unknown-linux-musl` |
| WASM | `wasm-pack build --target bundler` |
| C API (shared library) | `cargo build --release --features capi --target x86_64-unknown-linux-gnu` |

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

Plain `cargo test` does not compile the C API. To run everything including
the FFI tests and the C smoke test (requires clang and the
`x86_64-unknown-linux-gnu` target):

```sh
cargo test --features capi
```

The full pre-release battery — the above plus WASM and no-default-features
compile checks — is:

```sh
./test-all.sh
```

## Building the WASM / npm Package

Requires [wasm-pack](https://rustwasm.github.io/wasm-pack/):

```sh
cargo install wasm-pack
wasm-pack build --target bundler
```

## Building the C API

The C ABI lives in `src/ffi.rs` behind the `capi` feature (see
[docs/c-api.md](docs/c-api.md)). A `cdylib` cannot be produced for musl, so
on a musl-default host build for a gnu target explicitly:

```sh
cargo build --release --features capi --target x86_64-unknown-linux-gnu
```

The header `include/tjson.h` is maintained by hand. If you change the binary
interface (signatures, constants, the `TjsonError` layout), update the header
and bump `TJSON_ABI_VERSION` in both `src/ffi.rs` and the header — tests fail
if they drift. Ordinary releases must not touch the header.
