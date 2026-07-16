# tjson C API

tjson exposes a small C ABI so it can be called from any language with a C FFI
(C, C++, Delphi, C#, Python via `ctypes`, Go via cgo, Lua, …). This document is
the reference for using it **correctly**, with particular attention to memory
ownership — the one thing a C-level API cannot enforce for you.

The entire surface is six symbols. If you follow the ownership rules in
[Memory ownership](#memory-ownership), you cannot leak or corrupt memory.

## Contents

- [Building the shared library](#building-the-shared-library)
- [The API](#the-api)
- [Memory ownership](#memory-ownership)
- [Usage](#usage)
  - [1. Parse TJSON, success or failure only](#1-parse-tjson-success-or-failure-only)
  - [2. Parse TJSON, report the error on failure](#2-parse-tjson-report-the-error-on-failure)
  - [3. Render JSON as TJSON, with options](#3-render-json-as-tjson-with-options)
  - [4. Processing many inputs (the reuse pitfall)](#4-processing-many-inputs-the-reuse-pitfall)
  - [5. A complete program](#5-a-complete-program)
- [Errors](#errors)
- [Options](#options)
- [Encoding](#encoding)
- [Thread safety](#thread-safety)
- [Testing your integration](#testing-your-integration)
- [A Note on this TJSON Rendering ABI](#a-note-on-this-tjson-rendering-abi)

## Building the shared library

The C ABI is behind the `capi` Cargo feature and is off by default. Build the
dynamic library with:

```sh
cargo build --release --features capi
```

This produces, in `target/release/` (or `target/<triple>/release/`):

| Platform | File |
|----------|------|
| Linux    | `libtjson.so` |
| macOS    | `libtjson.dylib` |
| Windows  | `tjson.dll` (+ `tjson.dll.lib` import library with MSVC) |

The header, [`include/tjson.h`](../include/tjson.h), is maintained by hand in
sync with `src/ffi.rs`. Its `TJSON_ABI_VERSION` macro is checked against the
Rust source by a test, and `tests/capi/run.sh` compiles a real C program
against the header and the built library under AddressSanitizer. The header
deliberately carries no release version — it only changes when the ABI
changes.

### Prebuilt binaries

If you don't want a Rust toolchain, each tagged
[GitHub Release](https://github.com/rfanth/tjson/releases) attaches artifacts
named `tjson-<role>-<os>-<arch>`, where the role is `cli` (the standalone
command-line tool) or `lib` (the C API package):

| Asset | Contents |
|-------|----------|
| `tjson-lib-windows-x64.zip` | `tjson.dll` (64-bit, self-contained: static C runtime) + `tjson.dll.lib` + `include/tjson.h` + LICENSE |
| `tjson-lib-windows-x86.zip` | the same, 32-bit — for Win32 apps (common with legacy Delphi) |
| `tjson-lib-macos-arm64.zip` | `libtjson.dylib` (Apple Silicon) + `include/tjson.h` + LICENSE |
| `tjson-cli-linux-x86_64` | static-musl CLI, runs on any x86-64 Linux |
| `tjson-cli-linux-aarch64` | static-musl CLI for ARM64 Linux (Raspberry Pi, Graviton, Docker on Apple Silicon) |
| `tjson-cli-macos-arm64` | CLI, Apple Silicon |
| `tjson-cli-windows-x64.exe` | CLI, self-contained (static C runtime) |

Every CLI binary is executed as part of the release build before it ships.

There is deliberately **no prebuilt Linux `libtjson.so`**: a prebuilt shared
library would be pinned to the build machine's glibc version and could fail to
load on older systems. On Linux, build it yourself with the command above — it
is a one-liner and links against your own glibc.

> **Note:** a `cdylib` cannot be produced for targets that don't support
> dynamic libraries (e.g. `x86_64-unknown-linux-musl`). If your default target
> is one of those, build for a dynamic-capable target explicitly, e.g.
> `--target x86_64-unknown-linux-gnu`.

## The API

```c
#include "tjson.h"

char       *tjson_to_json  (const char *tjson_utf8, TjsonError *err);
char       *tjson_from_json(const char *json_utf8,
                            const char *options_json_utf8, TjsonError *err);
void        tjson_free_string(char *s);
const char *tjson_version(void);
int32_t     tjson_abi_version(void);

typedef struct TjsonError {
    int32_t code;      /* TJSON_OK or a TJSON_ERR_* value            */
    int32_t line;      /* 1-based position of a parse error, 0 = n/a */
    int32_t column;    /* 1-based position of a parse error, 0 = n/a */
    char   *message;   /* owned; free with tjson_free_string()       */
} TjsonError;
```

- `tjson_to_json` — parse a TJSON string, return the equivalent JSON string.
  The JSON is **compact** (no insignificant whitespace).
- `tjson_from_json` — render a JSON string as TJSON. `options_json_utf8` may be
  `NULL` for defaults (see [Options](#options)).
- `tjson_free_string` — release a string the library returned.
- `tjson_version` — the library release version (a static string; see below).
- `tjson_abi_version` — the ABI version of the loaded library. Compare it
  against the header's `TJSON_ABI_VERSION` macro at startup to catch a
  header/library mismatch before calling anything else. The ABI version only
  changes when the binary interface changes, not on ordinary releases.

Both conversion functions return a newly allocated string on success, or `NULL`
on failure. On failure they fill `*err` (unless `err` is `NULL`).

## Memory ownership

There are exactly two kinds of pointer, and one rule for each.

**Inputs you pass in (`const char *`) — you own them.**
The library only reads them for the duration of the call; it never stores or
frees them. Free them (or not) however you normally would.

**Strings the library returns — the library owns the allocator, you own the
lifetime.** This means the return value of `tjson_to_json` /
`tjson_from_json`, and a non-`NULL` `TjsonError.message`, must be released
with **`tjson_free_string`** — and only that:

- **Never** call the C runtime's `free()` on them. Rust's allocator and your C
  runtime's allocator need not be the same heap; the mismatch is undefined
  behavior (works by luck on Linux/macOS, corrupts the heap on Windows or with
  a custom allocator).
- Free each returned pointer **exactly once**. `tjson_free_string(NULL)` is a
  safe no-op, but freeing the *same real pointer* twice is a double free
  (undefined behavior). The function is null-safe, not idempotent.
- `tjson_version()` returns a **static** string. Do **not** free it.

That is the whole model. Everything below is just applying it.

## Usage

### 1. Parse TJSON, success or failure only

If you only need "did it work," pass `NULL` for the error out-parameter and
check for a `NULL` return. Nothing is allocated for the error, so there is
nothing extra to free.

```c
char *json = tjson_to_json(input, NULL);
if (json == NULL) {
    /* handle failure; no error detail requested */
    return -1;
}
use(json);
tjson_free_string(json);   /* the one allocation, freed once */
```

### 2. Parse TJSON, report the error on failure

Pass a `TjsonError`. On failure, `err.message` is an **owned** string you must
free, and `err.line` / `err.column` locate parse errors (see
[Errors](#errors)). A tidy, branch-free cleanup works because
`tjson_free_string` is null-safe and the message is `NULL` on success:

```c
TjsonError err = { 0, 0, 0, NULL };
char *json = tjson_to_json(input, &err);

if (json == NULL) {
    fprintf(stderr, "tjson: %s (code %d, line %d, column %d)\n",
            err.message, err.code, err.line, err.column);
} else {
    use(json);
}

tjson_free_string(json);         /* NULL on failure -> no-op */
tjson_free_string(err.message);  /* NULL on success -> no-op */
```

Exactly one of `json` / `err.message` is non-`NULL` after the call, and this
frees whichever it is. You never leak and never double free.

### 3. Render JSON as TJSON, with options

Options are a JSON object passed as a string (or `NULL` for defaults) — see
[Options](#options) for the full list of fields.

```c
TjsonError err = { 0, 0, 0, NULL };

/* defaults */
char *a = tjson_from_json("{\"name\":\"Alice\"}", NULL, &err);

/* one key-value pair per line, no packing, no tables */
char *b = tjson_from_json("{\"a\":1,\"b\":2}", "{\"canonical\":true}", &err);

/* narrow output */
char *c = tjson_from_json(doc, "{\"wrapWidth\":40}", &err);

/* ... use ... */
tjson_free_string(a);
tjson_free_string(b);
tjson_free_string(c);
```

If the options string is not a valid options object — not JSON, an unknown
field name, or an invalid value — the call returns `NULL` with
`err.code == TJSON_ERR_OPTIONS` and a message describing the problem. Unknown
fields are **rejected, not ignored**: a typo like `{"wrapWdith":40}` fails
loudly instead of silently rendering with defaults.

### 4. Processing many inputs (the reuse pitfall)

If you reuse a single `TjsonError` across calls, **free `err.message` before
each new call.** The next call resets `err.message` to `NULL` without freeing
it, so a previous error message left in place would leak:

```c
TjsonError err = { 0, 0, 0, NULL };
for (size_t i = 0; i < n; i++) {
    char *json = tjson_to_json(inputs[i], &err);
    if (json == NULL) {
        log_error(err.message);
        tjson_free_string(err.message);   /* <-- required before the next call */
        err.message = NULL;               /* keeps the struct honest */
        continue;
    }
    consume(json);
    tjson_free_string(json);
}
```

The simplest defensive habit is to `tjson_free_string(err.message)` at the end
of every iteration regardless of success — it is null-safe, so it costs nothing
on the success path.

### 5. A complete program

```c
#include <stdio.h>
#include "tjson.h"

int main(void) {
    printf("tjson %s\n", tjson_version());   /* static string: do not free */

    TjsonError err = { 0, 0, 0, NULL };

    char *json = tjson_to_json("  name: Alice  city: London", &err);
    if (json == NULL) {
        fprintf(stderr, "parse failed at line %d, column %d: %s\n",
                err.line, err.column, err.message);
        tjson_free_string(err.message);
        return 1;
    }
    printf("JSON:  %s\n", json);
    tjson_free_string(json);

    char *tjson = tjson_from_json("{\"scores\":[1,2,3]}", NULL, &err);
    if (tjson == NULL) {
        fprintf(stderr, "render failed: %s\n", err.message);
        tjson_free_string(err.message);
        return 1;
    }
    printf("TJSON: %s\n", tjson);
    tjson_free_string(tjson);

    return 0;
}
```

Compile and link against the shared library:

```sh
cc -Iinclude example.c -Ltarget/release -ltjson -o example
LD_LIBRARY_PATH=target/release ./example
```

## Errors

`TjsonError.code` is one of:

| Constant             | Value | Meaning |
|----------------------|-------|---------|
| `TJSON_OK`           | 0 | success (`message` is `NULL`, `line`/`column` are 0) |
| `TJSON_ERR_NULL`     | 1 | a required pointer argument was `NULL` — a bug in the caller, not a data problem |
| `TJSON_ERR_UTF8`     | 2 | an argument's bytes were not valid UTF-8; the message names the argument |
| `TJSON_ERR_PARSE`    | 3 | input was not valid TJSON (`tjson_to_json`) or JSON (`tjson_from_json`) |
| `TJSON_ERR_OPTIONS`  | 4 | the options JSON was not a valid options object (not JSON, unknown field, or invalid value) |
| `TJSON_ERR_INTERNAL` | 5 | an internal failure such as a caught panic — a tjson bug, please report it |

A `NULL` return always corresponds to a nonzero `code`. The `code` lets you
distinguish *why* it failed without parsing `message`.

`line` and `column` are 1-based and refer to the text that failed to parse:
the input document for `TJSON_ERR_PARSE`, the options string for
`TJSON_ERR_OPTIONS`. Both are 0 when no position applies. Use them when you
want to point at the error programmatically (an editor caret, a highlighted
line); `message` already includes the position in human-readable form and,
for TJSON parse errors, the offending source line with a caret marker.

## Options

`tjson_from_json`'s options string is a JSON object; pass `NULL` for all
defaults. The fields are **identical to the JavaScript binding's
`StringifyOptions`** — same camelCase names, same types, same defaults. They
are documented once, in the
[npm README's Options section](../npm-README.md#options); the same
deserializer parses both bindings' options, so the C and JS option sets
cannot drift apart.

What differs in C is only how the object is delivered and policed:

- It is passed as a JSON **string**, e.g. `"{\"wrapWidth\":40}"`, not a live
  object.
- The string must be a JSON **object**. Arrays, bare scalars, and `null` are
  rejected with `TJSON_ERR_OPTIONS` (pass a NULL *pointer*, not the JSON
  text `null`, for defaults).
- Unknown fields are **rejected** with `TJSON_ERR_OPTIONS`, and the error
  message names the offending field. (The JS binding tolerates unknown keys,
  as is idiomatic there — TypeScript catches typos at compile time. A C
  caller has no such net, so typos fail loudly instead of silently rendering
  with defaults.)
- Invalid values are likewise rejected with `TJSON_ERR_OPTIONS`.

## Encoding

All strings crossing the boundary are **NUL-terminated UTF-8**, and the
library validates this: bytes that are not valid UTF-8 are rejected with
`TJSON_ERR_UTF8` rather than misinterpreted. If your language's native string
type is not UTF-8 (for example Delphi and .NET use UTF-16), convert on the way
in and out:

```pascal
// Delphi sketch. TTjsonError mirrors the C struct:
//   record Code, Line, Column: Int32; Message: PAnsiChar; end
function TjsonToJson(const Src: string): string;
var
  raw: PAnsiChar;
  err: TTjsonError;
begin
  raw := tjson_to_json(PAnsiChar(UTF8Encode(Src)), @err);
  if raw = nil then
    raise Exception.Create(string(UTF8String(err.Message)));  // then free it
  try
    Result := UTF8ToString(raw);
  finally
    tjson_free_string(raw);
  end;
end;
```

Interior NUL bytes are not representable in a C string; input is read up to the
first NUL, exactly like every C string API.

## Thread safety

The functions hold no shared mutable state — errors are reported through the
`TjsonError` you provide, not a global or thread-local. You may call them
concurrently from multiple threads, provided each call uses its own inputs and
its own `TjsonError`, and a string returned to one thread is not freed by
another while still in use.

## Testing your integration

A C-level API cannot enforce the ownership rules above, so validate your
integration with a memory tool — this catches leaks, double frees, and
allocator mismatches that otherwise hide until production:

```sh
# AddressSanitizer + LeakSanitizer
cc -fsanitize=address -g -Iinclude your_test.c -Ltarget/release -ltjson -o t
ASAN_OPTIONS=detect_leaks=1 LD_LIBRARY_PATH=target/release ./t

# or Valgrind, no rebuild needed
LD_LIBRARY_PATH=target/release valgrind --leak-check=full ./t
```

tjson's own C ABI smoke test ([`tests/capi/`](../tests/capi/)) runs this way;
`tests/capi/run.sh` builds the library and exercises the ABI under
AddressSanitizer, and `cargo test --features capi` runs it automatically as
part of the normal test suite.

## A Note on this TJSON Rendering ABI

The point of typing JSON->TJSON rendering options in tjson_from_json as a JSON
string over an options struct in this C ABI is to simplify rolling your own
interface even if your language isn't supported directly, without imposing a
maintainance burden on either tjson-rs or you when the TJSON rendering options
change.

The ABI version from tjson_abi_version does not need to be and will not be
bumped when TJSON rendering options change, are added, or are removed, though
tjson_version will change.  Your glue code linking to this C FFI will not need
to change either.

This means that the calling language may not have the fine-grained compile time
option typing that Rust or Typescript users enjoy. I have tried to compensate
by having unusually good error messages. If you want typed structs, which would
necessitate more frequent ABI changes as the underlying tjson rendering options
change, feel free to build and maintain your own interface that calls directly
into the native Rust code.

