#!/usr/bin/env bash
#
# Run the full tjson test battery. This is THE pre-release test command.
#
#   ./test-all.sh
#
# Stage 1 runs every Rust test with the C API enabled — features are
# additive, so this is the core suite PLUS the FFI unit tests PLUS the C
# smoke test (tests/capi_smoke.rs shells out to tests/capi/run.sh, which
# compiles a real C program against include/tjson.h with clang and runs it
# under AddressSanitizer).
#
# Stage 2 proves the WASM binding still compiles (it does not run wasm
# tests; the npm package has its own release checks).
set -euo pipefail
cd "$(dirname "$0")"

echo "=== 1/3 cargo test --features capi (core + FFI + C smoke test) ==="
cargo test --features capi

echo "=== 2/3 WASM compile check ==="
# --lib: only the library compiles to wasm (the CLI's terminal_size
# dependency is desktop-only, and wasm-pack builds the lib alone anyway).
cargo check --lib --target wasm32-unknown-unknown

echo "=== 3/3 no-default-features compile check ==="
# The lib docs advertise `default-features = false` (drops the serde_json
# From impls); make sure that config keeps compiling. The CLI is skipped
# automatically (required-features in Cargo.toml).
cargo check --no-default-features

echo
echo "all test stages passed"
