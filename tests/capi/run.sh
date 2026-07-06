#!/usr/bin/env bash
#
# Build the tjson cdylib (capi feature) and run the C ABI smoke test under
# AddressSanitizer + LeakSanitizer. Any leak or memory error fails the run.
#
# The cdylib is only produced for targets that support it (not musl), so this
# pins a gnu target explicitly.
set -euo pipefail
cd "$(dirname "$0")/../.."

TARGET="${TJSON_CAPI_TARGET:-x86_64-unknown-linux-gnu}"
CC="${CC:-clang}"

cargo build --release --features capi --target "$TARGET"
LIBDIR="target/$TARGET/release"

"$CC" -fsanitize=address -g -Iinclude tests/capi/roundtrip.c \
    -L"$LIBDIR" -ltjson -o "$LIBDIR/capi_roundtrip"

ASAN_OPTIONS=detect_leaks=1 \
LD_LIBRARY_PATH="$LIBDIR" \
    "$LIBDIR/capi_roundtrip"
