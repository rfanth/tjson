/*
 * End-to-end smoke test for the tjson C ABI.
 *
 * Built and run by tests/capi/run.sh, which links against the cdylib and runs
 * this under AddressSanitizer. Exits 0 on success, 1 on any failure.
 */
#include <stdio.h>
#include <string.h>

#include "tjson.h"

static int failures = 0;

static void check(int condition, const char *what) {
    if (!condition) {
        fprintf(stderr, "FAIL: %s\n", what);
        failures++;
    }
}

int main(void) {
    /* the loaded library must implement the ABI this header describes */
    check(tjson_abi_version() == TJSON_ABI_VERSION,
          "loaded library ABI matches the header's TJSON_ABI_VERSION");

    /* version is a static string; must not be freed */
    const char *version = tjson_version();
    check(version != NULL && version[0] != '\0', "tjson_version returns a string");

    /* TJSON -> JSON */
    TjsonError err = { 0, 0, 0, NULL };
    char *json = tjson_to_json("  name: Alice  age: 30", &err);
    check(json != NULL, "tjson_to_json succeeds");
    check(err.code == TJSON_OK && err.message == NULL, "success leaves err clear");
    check(err.line == 0 && err.column == 0, "success leaves position clear");
    check(json != NULL && strstr(json, "\"Alice\"") != NULL, "JSON contains the value");
    tjson_free_string(json);

    /* JSON -> TJSON with options */
    char *tjson = tjson_from_json("{\"a\":1,\"b\":2}", "{\"canonical\":true}", &err);
    check(tjson != NULL, "tjson_from_json succeeds");
    tjson_free_string(tjson);

    /* JSON -> TJSON with default options (null options pointer) */
    tjson = tjson_from_json("{\"a\":1}", NULL, &err);
    check(tjson != NULL, "tjson_from_json with null options succeeds");
    tjson_free_string(tjson);

    /* error path: invalid TJSON returns NULL and fills err (message owned),
     * with a 1-based position; the bad byte here is on line 2 */
    char *bad = tjson_to_json("  ok: yes\n  key: \a", &err);
    check(bad == NULL, "invalid TJSON returns NULL");
    check(err.code == TJSON_ERR_PARSE, "invalid TJSON sets TJSON_ERR_PARSE");
    check(err.line == 2, "parse error reports the line");
    check(err.column >= 1, "parse error reports a 1-based column");
    check(err.message != NULL, "invalid TJSON provides a message");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: a typo'd option field must be rejected, not ignored */
    bad = tjson_from_json("{\"a\":1}", "{\"wrapWdith\":40}", &err);
    check(bad == NULL, "unknown option field returns NULL");
    check(err.code == TJSON_ERR_OPTIONS, "unknown option field sets TJSON_ERR_OPTIONS");
    check(err.message != NULL && strstr(err.message, "wrapWdith") != NULL,
          "unknown option field is named in the message");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: a removed/renamed option gets a migration hint */
    bad = tjson_from_json("{\"a\":1}", "{\"tableMinCols\":2}", &err);
    check(bad == NULL, "renamed option field returns NULL");
    check(err.code == TJSON_ERR_OPTIONS, "renamed option field sets TJSON_ERR_OPTIONS");
    check(err.message != NULL && strstr(err.message, "tableMinColumns") != NULL,
          "renamed option message points at the new name");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: options must be a JSON object — without the shape guard,
     * [true] would be accepted positionally as {"canonical":true} */
    bad = tjson_from_json("{\"a\":1}", "[true]", &err);
    check(bad == NULL, "array options returns NULL");
    check(err.code == TJSON_ERR_OPTIONS, "array options sets TJSON_ERR_OPTIONS");
    check(err.message != NULL && strstr(err.message, "JSON object") != NULL,
          "array options message names the required shape");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: a bad option value is also TJSON_ERR_OPTIONS */
    bad = tjson_from_json("{\"a\":1}", "{\"wrapWidth\":\"wide\"}", &err);
    check(bad == NULL, "invalid option value returns NULL");
    check(err.code == TJSON_ERR_OPTIONS, "invalid option value sets TJSON_ERR_OPTIONS");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: NULL input pointer is distinguished from bad encoding.
     * The previous error carried a position; this one has none — its 0/0
     * must come from this error, not leak from the last (no franken-errors) */
    bad = tjson_to_json(NULL, &err);
    check(bad == NULL, "NULL input returns NULL");
    check(err.code == TJSON_ERR_NULL, "NULL input sets TJSON_ERR_NULL");
    check(err.line == 0 && err.column == 0,
          "positionless error does not inherit the previous error's position");
    tjson_free_string(err.message);
    err.message = NULL;

    /* error path: bytes that are not UTF-8 (0xFF never appears in UTF-8) */
    const char not_utf8[] = { (char)0xFF, 0 };
    bad = tjson_to_json(not_utf8, &err);
    check(bad == NULL, "non-UTF-8 input returns NULL");
    check(err.code == TJSON_ERR_UTF8, "non-UTF-8 input sets TJSON_ERR_UTF8");
    tjson_free_string(err.message);
    err.message = NULL;

    /* success with a reused err must reset every field the last error set
     * (err.code is stale TJSON_ERR_UTF8 here from the block above) */
    char *reused = tjson_to_json("  ok: yes", &err);
    check(reused != NULL, "success after a prior error works");
    check(err.code == TJSON_OK && err.line == 0 && err.column == 0 && err.message == NULL,
          "success resets a reused error struct");
    tjson_free_string(reused);

    /* null-safety and opt-out of the error out-param */
    tjson_free_string(NULL);
    char *no_err = tjson_to_json("  ok: yes", NULL);
    check(no_err != NULL, "null err out-param is allowed");
    tjson_free_string(no_err);

    if (failures == 0) {
        printf("all C ABI checks passed\n");
        return 0;
    }
    fprintf(stderr, "%d C ABI check(s) failed\n", failures);
    return 1;
}
