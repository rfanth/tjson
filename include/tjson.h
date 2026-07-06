/*
 * tjson.h — C ABI for the tjson library (Text JSON).
 *
 * This header is maintained by hand, in sync with src/ffi.rs (a Rust-side
 * test checks the version macros; tests/capi/run.sh compiles a C program
 * against this header and the built library under AddressSanitizer).
 *
 * Build the shared library with the `capi` feature:
 *     cargo build --release --features capi
 *
 * MEMORY OWNERSHIP
 *   - Input pointers (const char *) are borrowed. The library never frees
 *     them; the caller keeps ownership.
 *   - Any char * returned here, and any non-null TjsonError.message, is
 *     allocated by Rust. Release it with tjson_free_string() ONLY.
 *       * Do NOT use the C runtime's free() on it. Rust's allocator and the
 *         C runtime's may be different heaps; the mismatch is undefined
 *         behavior (a crash on Windows / with a custom allocator).
 *       * Free each returned pointer exactly ONCE. Passing NULL is safe;
 *         a double free of a real pointer is undefined behavior.
 *       * If you reuse one TjsonError across calls, free its .message (or
 *         call tjson_free_string on it, which is NULL-safe) before the next
 *         call, or it leaks.
 *   - tjson_version() returns a static string. Do NOT free it.
 *
 * All strings crossing the boundary are NUL-terminated UTF-8.
 *
 * These are the standard C manual-memory rules; there is no way to enforce
 * them at the ABI. Test integrations under AddressSanitizer or Valgrind.
 * See docs/c-api.md for the full reference and worked examples.
 */

#ifndef TJSON_H
#define TJSON_H

#include <stdint.h>

/*
 * The ABI version this header describes. It changes only when the binary
 * interface changes (a function added or changed, the TjsonError layout
 * altered) — not on ordinary library releases. Compare against
 * tjson_abi_version() at startup to detect a mismatch between the header you
 * compiled with and the library you loaded; tjson_version() reports the
 * library's release version as a string.
 */
#define TJSON_ABI_VERSION 1

/* Success. TjsonError.code is set to this when a call succeeds. */
#define TJSON_OK 0

/* A required pointer argument was NULL. A bug in the caller, not a data
 * problem. */
#define TJSON_ERR_NULL 1

/* An argument's bytes were not valid UTF-8. The message names the argument. */
#define TJSON_ERR_UTF8 2

/* The input was not valid TJSON (tjson_to_json) or not valid JSON
 * (tjson_from_json). TjsonError.line/.column locate the problem. */
#define TJSON_ERR_PARSE 3

/* The options JSON was not a valid options object: not JSON, an unknown
 * field, or an invalid value. */
#define TJSON_ERR_OPTIONS 4

/* An internal failure, such as a caught panic. This indicates a bug in
 * tjson — please report it. */
#define TJSON_ERR_INTERNAL 5

/*
 * Explicit error out-parameter, filled in on failure.
 *
 * Stack-allocate one and pass its address, or pass NULL if you only care
 * whether the call returned NULL. On success, code is TJSON_OK, line and
 * column are 0, and message is NULL. On failure, code is nonzero and message
 * is an owned, NUL-terminated UTF-8 string that must be freed with
 * tjson_free_string(). For parse errors the message includes the offending
 * source line and a caret marker, ready to display.
 *
 * line and column are 1-based and refer to the text that failed to parse:
 * the input document for TJSON_ERR_PARSE, the options string for
 * TJSON_ERR_OPTIONS. Both are 0 when no position applies.
 */
typedef struct TjsonError {
    int32_t code;
    int32_t line;
    int32_t column;
    char *message;
} TjsonError;

#ifdef __cplusplus
extern "C" {
#endif

/*
 * Parse a TJSON string (UTF-8) and return the equivalent JSON string
 * (UTF-8, compact — no insignificant whitespace).
 *
 * Returns a newly allocated string that must be freed with
 * tjson_free_string(), or NULL on error (in which case *err, if err is
 * non-NULL, is filled in).
 */
char *tjson_to_json(const char *tjson_utf8, TjsonError *err);

/*
 * Render a JSON string (UTF-8) as TJSON (UTF-8).
 *
 * options_json_utf8 may be NULL for default rendering; otherwise it is a
 * JSON object of camelCase option fields, for example
 * "{\"wrapWidth\":80,\"tables\":true}" — see docs/c-api.md for the full
 * list. Unknown fields and invalid values are rejected with
 * TJSON_ERR_OPTIONS. Returns a newly allocated string that must be freed
 * with tjson_free_string(), or NULL on error (in which case *err, if err is
 * non-NULL, is filled in).
 */
char *tjson_from_json(const char *json_utf8,
                      const char *options_json_utf8,
                      TjsonError *err);

/*
 * Free a string returned by tjson_to_json(), tjson_from_json(), or the
 * message field of a TjsonError. Passing NULL is a no-op.
 */
void tjson_free_string(char *s);

/* Return the tjson version as a static, NUL-terminated string. Do not free
 * it. */
const char *tjson_version(void);

/*
 * Return the ABI version of the loaded library. Compare against the
 * TJSON_ABI_VERSION macro of the header you compiled with to detect a
 * header/library mismatch before calling anything else.
 */
int32_t tjson_abi_version(void);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* TJSON_H */
