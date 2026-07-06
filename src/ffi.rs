//! C ABI (FFI) bindings for tjson, gated behind the `capi` feature.
//!
//! This is the **only** module in the crate that uses `unsafe`. Every build
//! that does not enable `capi` keeps `#![forbid(unsafe_code)]` (see `lib.rs`);
//! here the crate lint is relaxed to `deny` and re-allowed for this module
//! alone.
//!
//! The public C header, [`include/tjson.h`](../include/tjson.h), is
//! **hand-maintained** — when you change an exported signature, constant, or
//! the [`TjsonError`] layout, update the header to match and bump
//! [`TJSON_ABI_VERSION`] in both places. Ordinary releases do not touch the
//! header: it deliberately carries no crate version, only the ABI version.
//! The test module below pins the struct layout and checks the header's ABI
//! version against this module, and `tests/capi/run.sh` compiles a real C
//! program against the header and the built cdylib under AddressSanitizer.
//!
//! The conventions follow the common Rust-exposing-C idiom (compare Mozilla
//! `ffi-support`, `rure`, and `rustls-ffi`):
//!
//! * Inputs are borrowed `*const c_char`; the caller keeps ownership of them.
//! * Returned strings are allocated by Rust and **must** be released with
//!   [`tjson_free_string`] — never the C runtime's `free`, because the two
//!   allocators need not match.
//! * Every entry point runs inside [`std::panic::catch_unwind`], so a Rust
//!   panic can never unwind across the FFI boundary (which would be UB). A
//!   caught panic becomes a [`TJSON_ERR_INTERNAL`] error rather than aborting
//!   the host process.
//! * Errors are reported explicitly through a caller-provided [`TjsonError`]
//!   out-parameter, not through a thread-local.
//!
//! All strings crossing the boundary are NUL-terminated UTF-8.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;

/// The ABI version of this C API. Incremented only when the binary interface
/// changes (a function added or changed, the [`TjsonError`] layout altered) —
/// **not** on ordinary releases. Must match `TJSON_ABI_VERSION` in
/// `include/tjson.h`; a test below enforces that, and callers compare the
/// header macro against [`tjson_abi_version`] at runtime to detect a
/// header/library mismatch.
pub const TJSON_ABI_VERSION: i32 = 1;

/// Success. [`TjsonError::code`] is set to this when a call succeeds.
pub const TJSON_OK: i32 = 0;
/// A required pointer argument was null. This is a bug in the caller, not a
/// data problem.
pub const TJSON_ERR_NULL: i32 = 1;
/// An argument's bytes were not valid UTF-8. The message names the argument.
pub const TJSON_ERR_UTF8: i32 = 2;
/// The input was not valid TJSON (for [`tjson_to_json`]) or not valid JSON
/// (for [`tjson_from_json`]). `line`/`column` locate the problem.
pub const TJSON_ERR_PARSE: i32 = 3;
/// The options JSON was not a valid options object: not JSON, an unknown
/// field, or an invalid value.
pub const TJSON_ERR_OPTIONS: i32 = 4;
/// An internal failure, such as a caught panic. This indicates a bug in
/// tjson — please report it.
pub const TJSON_ERR_INTERNAL: i32 = 5;

/// Explicit error out-parameter, filled in on failure.
///
/// Stack-allocate one and pass its address, or pass null if you only care
/// whether the call returned null. On success `code` is [`TJSON_OK`],
/// `line`/`column` are 0, and `message` is null. On failure `code` is nonzero
/// and `message` is an owned, NUL-terminated UTF-8 string that must be freed
/// with [`tjson_free_string`].
///
/// `line` and `column` are 1-based and refer to the text that failed to
/// parse: the input document for [`TJSON_ERR_PARSE`], the options string for
/// [`TJSON_ERR_OPTIONS`]. Both are 0 when no position applies.
///
/// The field order (`code`, `line`, `column`, `message`) is ABI: it must match
/// the struct in `include/tjson.h` and is pinned by a layout test below.
#[repr(C)]
pub struct TjsonError {
    pub code: i32,
    pub line: i32,
    pub column: i32,
    pub message: *mut c_char,
}

/// Internal, owned form of a boundary error, converted into the caller's
/// [`TjsonError`] by [`set_error`] at the very end of each entry point.
struct FfiError {
    code: i32,
    line: i32,
    column: i32,
    message: String,
}

impl FfiError {
    /// An error with no source position (`line`/`column` = 0).
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, line: 0, column: 0, message: message.into() }
    }

    /// An error located at a 1-based `line`/`column` in some source text.
    /// Positions of 0 mean "not applicable" and are passed through as-is.
    fn at(code: i32, line: usize, column: usize, message: impl Into<String>) -> Self {
        Self {
            code,
            line: clamp_to_i32(line),
            column: clamp_to_i32(column),
            message: message.into(),
        }
    }
}

/// Narrow a usize position to the i32 the C struct carries, saturating rather
/// than wrapping for absurdly large inputs.
fn clamp_to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Reset `err` to the success state. A null `err` is ignored.
fn clear_error(err: *mut TjsonError) {
    if err.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a non-null `err` points to a live TjsonError.
    unsafe {
        (*err).code = TJSON_OK;
        (*err).line = 0;
        (*err).column = 0;
        (*err).message = ptr::null_mut();
    }
}

/// Record `error` in `err`. A null `err` is ignored.
fn set_error(err: *mut TjsonError, error: &FfiError) {
    if err.is_null() {
        return;
    }
    let owned = CString::new(error.message.as_str()).unwrap_or_else(|_| {
        CString::new("error message contained an interior NUL byte").unwrap()
    });
    // SAFETY: the caller guarantees a non-null `err` points to a live TjsonError.
    unsafe {
        (*err).code = error.code;
        (*err).line = error.line;
        (*err).column = error.column;
        (*err).message = owned.into_raw();
    }
}

/// Borrow a C string argument as `&str`, reporting null and invalid UTF-8 as
/// distinct errors. `what` names the argument for the error message.
///
/// # Safety
///
/// `ptr` must be null or point to a valid NUL-terminated C string that stays
/// alive for the duration of the returned borrow.
unsafe fn borrow_utf8<'a>(ptr: *const c_char, what: &str) -> Result<&'a str, FfiError> {
    if ptr.is_null() {
        return Err(FfiError::new(TJSON_ERR_NULL, format!("{what} pointer was null")));
    }
    // SAFETY: delegated to this function's own safety contract.
    unsafe { CStr::from_ptr(ptr) }.to_str().map_err(|e| {
        FfiError::new(TJSON_ERR_UTF8, format!("{what} was not valid UTF-8: {e}"))
    })
}

/// Turn the result of a boundary operation into the C return value, populating
/// `err` on any failure path.
fn finish(
    result: std::thread::Result<Result<String, FfiError>>,
    err: *mut TjsonError,
) -> *mut c_char {
    match result {
        Ok(Ok(output)) => match CString::new(output) {
            Ok(owned) => owned.into_raw(),
            Err(_) => {
                set_error(
                    err,
                    &FfiError::new(TJSON_ERR_INTERNAL, "output contained an interior NUL byte"),
                );
                ptr::null_mut()
            }
        },
        Ok(Err(error)) => {
            set_error(err, &error);
            ptr::null_mut()
        }
        Err(_) => {
            set_error(
                err,
                &FfiError::new(
                    TJSON_ERR_INTERNAL,
                    "internal panic while processing input (this is a bug, please report it)",
                ),
            );
            ptr::null_mut()
        }
    }
}

/// Map a TJSON parsing failure onto the C error model, preserving the source
/// position for real parse errors.
fn tjson_parse_failure(error: crate::Error) -> FfiError {
    match error {
        crate::Error::Parse(parse_error) => FfiError::at(
            TJSON_ERR_PARSE,
            parse_error.line(),
            parse_error.column(),
            parse_error.to_string(),
        ),
        // from_str only produces Parse errors for bad input; anything else
        // escaping here is a tjson bug, not a data problem.
        other => FfiError::new(TJSON_ERR_INTERNAL, other.to_string()),
    }
}

/// Deserialize an options object into [`RenderOptions`](crate::RenderOptions).
///
/// The field names are camelCase, e.g. `{"wrapWidth":80,"tables":true}` — the
/// full list is documented in `docs/c-api.md`. Unlike the JS binding (where a
/// tolerant options bag is idiomatic and TypeScript catches typos at compile
/// time), a C caller has no static checking at all, so unknown fields are
/// rejected here rather than silently ignored.
fn parse_options(options_json: &str) -> Result<crate::RenderOptions, FfiError> {
    let options_error = |e: &serde_json::Error| {
        FfiError::at(TJSON_ERR_OPTIONS, e.line(), e.column(), format!("invalid options JSON: {e}"))
    };

    // Strictness has to be imposed *here*, not on TjsonConfig: that struct is
    // shared with the JS/WASM binding, whose users rely on the tolerant
    // options-bag behavior, so adding #[serde(deny_unknown_fields)] to it
    // would change published behavior. serde_ignored solves this at the call
    // site: it wraps the deserializer and invokes the callback with the path
    // of every field that TjsonConfig ignored, letting this one entry point
    // reject typos while every other consumer of TjsonConfig is untouched.
    // (Same technique cargo uses to warn about unknown manifest keys.)
    let mut unknown_fields: Vec<String> = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_str(options_json);
    let config: crate::TjsonConfig =
        serde_ignored::deserialize(&mut deserializer, |path| {
            unknown_fields.push(path.to_string());
        })
        .map_err(|e| options_error(&e))?;
    // serde_ignored::deserialize reads one JSON value; end() rejects trailing
    // garbage after it, matching serde_json::from_str's strictness.
    deserializer.end().map_err(|e| options_error(&e))?;

    if !unknown_fields.is_empty() {
        let mut message = format!("unknown option field(s): {}", unknown_fields.join(", "));
        if unknown_fields.iter().any(|field| field == "tableMinCols") {
            message.push_str("; tableMinCols has been renamed to tableMinColumns");
        }
        return Err(FfiError::new(TJSON_ERR_OPTIONS, message));
    }
    Ok(config.into())
}

/// Parse a TJSON string (UTF-8) and return the equivalent JSON string (UTF-8,
/// compact — no insignificant whitespace).
///
/// Returns a newly allocated string that must be freed with
/// [`tjson_free_string`], or null on error (in which case `err`, if non-null,
/// is filled in).
#[unsafe(no_mangle)]
pub extern "C" fn tjson_to_json(tjson_utf8: *const c_char, err: *mut TjsonError) -> *mut c_char {
    clear_error(err);
    let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<String, FfiError> {
        // SAFETY: the caller contract requires a valid C string or null.
        let input = unsafe { borrow_utf8(tjson_utf8, "input") }?;
        let value: serde_json::Value = crate::from_str(input).map_err(tjson_parse_failure)?;
        serde_json::to_string(&value)
            .map_err(|e| FfiError::new(TJSON_ERR_INTERNAL, e.to_string()))
    }));
    finish(result, err)
}

/// Render a JSON string (UTF-8) as TJSON (UTF-8).
///
/// `options_json_utf8` may be null for default rendering; otherwise it is a
/// JSON object of camelCase option fields, for example
/// `{"wrapWidth":80,"tables":true}` (full list in `docs/c-api.md`). Unknown
/// fields and invalid values are rejected with [`TJSON_ERR_OPTIONS`]. Returns
/// a newly allocated string that must be freed with [`tjson_free_string`], or
/// null on error (in which case `err`, if non-null, is filled in).
#[unsafe(no_mangle)]
pub extern "C" fn tjson_from_json(
    json_utf8: *const c_char,
    options_json_utf8: *const c_char,
    err: *mut TjsonError,
) -> *mut c_char {
    clear_error(err);
    let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<String, FfiError> {
        // SAFETY: the caller contract requires a valid C string or null.
        let input = unsafe { borrow_utf8(json_utf8, "input") }?;
        let value: serde_json::Value = serde_json::from_str(input).map_err(|e| {
            FfiError::at(TJSON_ERR_PARSE, e.line(), e.column(), format!("input is not valid JSON: {e}"))
        })?;

        let options = if options_json_utf8.is_null() {
            crate::RenderOptions::default()
        } else {
            // SAFETY: same contract as above for the options pointer.
            let options_json = unsafe { borrow_utf8(options_json_utf8, "options") }?;
            parse_options(options_json)?
        };

        crate::to_string_with(&value, options)
            .map_err(|e| FfiError::new(TJSON_ERR_INTERNAL, e.to_string()))
    }));
    finish(result, err)
}

/// Free a string returned by [`tjson_to_json`], [`tjson_from_json`], or the
/// `message` field of a [`TjsonError`]. Passing null is a no-op.
#[unsafe(no_mangle)]
pub extern "C" fn tjson_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    // SAFETY: `s` must have been produced by `CString::into_raw` in this module,
    // as documented for every function that hands out a string.
    unsafe {
        drop(CString::from_raw(s));
    }
}

/// Return the tjson version as a static, NUL-terminated string. Do not free it.
#[unsafe(no_mangle)]
pub extern "C" fn tjson_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Return the ABI version of the loaded library. Compare against the
/// `TJSON_ABI_VERSION` macro of the header you compiled with to detect a
/// header/library mismatch before calling anything else.
#[unsafe(no_mangle)]
pub extern "C" fn tjson_abi_version() -> i32 {
    TJSON_ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, cleared error out-parameter for a call.
    fn new_err() -> TjsonError {
        TjsonError { code: TJSON_OK, line: 0, column: 0, message: ptr::null_mut() }
    }

    /// Consume an owned `*mut c_char` from the API: copy it to a `String` and
    /// free it through the library, exactly as a C caller must.
    fn take(ptr: *mut c_char) -> String {
        assert!(!ptr.is_null(), "expected a non-null string");
        // SAFETY: `ptr` came from the API and is a valid C string.
        let owned = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_owned();
        tjson_free_string(ptr);
        owned
    }

    /// Read a caller-owned error message and free it, as a C caller must.
    fn take_error(err: &mut TjsonError) -> String {
        assert!(!err.message.is_null(), "expected a non-null error message");
        // SAFETY: `message` came from the API and is a valid C string.
        let owned = unsafe { CStr::from_ptr(err.message) }.to_str().unwrap().to_owned();
        tjson_free_string(err.message);
        err.message = ptr::null_mut();
        owned
    }

    /// The TjsonError layout is ABI, shared with include/tjson.h and every
    /// compiled caller. This test freezes it.
    #[test]
    fn error_struct_layout_is_frozen() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(offset_of!(TjsonError, code), 0);
        assert_eq!(offset_of!(TjsonError, line), 4);
        assert_eq!(offset_of!(TjsonError, column), 8);
        let pointer_size = size_of::<*mut c_char>();
        let expected_message_offset = if pointer_size == 8 { 16 } else { 12 };
        assert_eq!(offset_of!(TjsonError, message), expected_message_offset);
        assert_eq!(size_of::<TjsonError>(), expected_message_offset + pointer_size);
        assert_eq!(align_of::<TjsonError>(), align_of::<*mut c_char>());
    }

    /// include/tjson.h is hand-maintained; its TJSON_ABI_VERSION macro must
    /// match this module's constant. Deliberately nothing here tracks the
    /// crate version — the header only changes when the ABI changes, so
    /// ordinary releases touch Cargo.toml alone. (The rest of the header is
    /// exercised by tests/capi/run.sh, which compiles a C program against it.)
    #[test]
    fn header_abi_version_matches_module() {
        let header = include_str!("../include/tjson.h");
        let expected = format!("#define TJSON_ABI_VERSION {TJSON_ABI_VERSION}");
        assert!(
            header.contains(&expected),
            "include/tjson.h is out of date: expected `{expected}` — the header \
             and src/ffi.rs must be updated together when the ABI changes"
        );
    }

    #[test]
    fn abi_version_function_reports_the_constant() {
        assert_eq!(tjson_abi_version(), TJSON_ABI_VERSION);
    }

    #[test]
    fn to_json_round_trips() {
        let input = CString::new("  name: Alice  city: London").unwrap();
        let mut err = new_err();
        let out = tjson_to_json(input.as_ptr(), &mut err);
        assert_eq!(err.code, TJSON_OK);
        assert_eq!(err.line, 0);
        assert_eq!(err.column, 0);
        assert!(err.message.is_null());
        let json: serde_json::Value = serde_json::from_str(&take(out)).unwrap();
        assert_eq!(json, serde_json::json!({ "name": "Alice", "city": "London" }));
    }

    #[test]
    fn from_json_uses_defaults_when_options_null() {
        let input = CString::new(r#"{"name":"Alice"}"#).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), ptr::null(), &mut err);
        assert_eq!(err.code, TJSON_OK);
        assert_eq!(take(out).trim_end(), "  name: Alice");
    }

    #[test]
    fn from_json_honors_options() {
        let input = CString::new(r#"{"a":1,"b":2}"#).unwrap();
        let options = CString::new(r#"{"canonical":true}"#).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), options.as_ptr(), &mut err);
        assert_eq!(err.code, TJSON_OK);
        // Canonical layout puts one pair per line.
        assert_eq!(take(out).lines().count(), 2);
    }

    #[test]
    fn invalid_tjson_reports_parse_error_with_position() {
        // The control character is on line 2 so the position is unambiguous.
        let input = CString::new("  ok: yes\n  key: \u{0007}").unwrap();
        let mut err = new_err();
        let out = tjson_to_json(input.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_PARSE);
        assert_eq!(err.line, 2, "line should locate the bad character");
        assert!(err.column >= 1, "column should be 1-based, got {}", err.column);
        assert!(!take_error(&mut err).is_empty());
    }

    #[test]
    fn invalid_json_reports_parse_error_with_position() {
        let input = CString::new("{\n  not json\n}").unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), ptr::null(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_PARSE);
        assert_eq!(err.line, 2, "line should come from the JSON parser");
        assert!(err.column >= 1);
        assert!(take_error(&mut err).contains("JSON"));
    }

    #[test]
    fn invalid_option_value_reports_options_error() {
        let input = CString::new(r#"{"a":1}"#).unwrap();
        let options = CString::new(r#"{"wrapWidth":"wide"}"#).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), options.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_OPTIONS);
        assert!(!take_error(&mut err).is_empty());
    }

    #[test]
    fn unknown_option_field_is_rejected_and_named() {
        let input = CString::new(r#"{"a":1}"#).unwrap();
        // "wrapWdith" is a typo of "wrapWidth" — it must not be silently ignored.
        let options = CString::new(r#"{"wrapWdith":40}"#).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), options.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_OPTIONS);
        let message = take_error(&mut err);
        assert!(message.contains("wrapWdith"), "message must name the field: {message}");
    }

    #[test]
    fn renamed_table_min_cols_gets_migration_hint() {
        let input = CString::new(r#"{"a":1}"#).unwrap();
        let options = CString::new(r#"{"tableMinCols":2}"#).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), options.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_OPTIONS);
        let message = take_error(&mut err);
        assert!(
            message.contains("tableMinColumns"),
            "message must point at the new name: {message}"
        );
    }

    #[test]
    fn null_input_reports_null_error() {
        let mut err = new_err();
        let out = tjson_to_json(ptr::null(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_NULL);
        assert!(take_error(&mut err).contains("null"));
    }

    #[test]
    fn non_utf8_input_reports_utf8_error() {
        // 0xFF can never appear in well-formed UTF-8.
        let input = CString::new(vec![0xFFu8]).unwrap();
        let mut err = new_err();
        let out = tjson_to_json(input.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_UTF8);
        assert!(take_error(&mut err).contains("input"));
    }

    #[test]
    fn non_utf8_options_reports_utf8_error() {
        let input = CString::new(r#"{"a":1}"#).unwrap();
        let options = CString::new(vec![0xFFu8]).unwrap();
        let mut err = new_err();
        let out = tjson_from_json(input.as_ptr(), options.as_ptr(), &mut err);
        assert!(out.is_null());
        assert_eq!(err.code, TJSON_ERR_UTF8);
        assert!(take_error(&mut err).contains("options"));
    }

    #[test]
    fn null_error_out_param_is_allowed() {
        let input = CString::new("  ok: yes").unwrap();
        // A caller that does not care about the message passes null for `err`.
        let out = tjson_to_json(input.as_ptr(), ptr::null_mut());
        assert!(!out.is_null());
        tjson_free_string(out);
    }

    #[test]
    fn free_string_is_null_safe() {
        tjson_free_string(ptr::null_mut());
    }

    #[test]
    fn version_is_present() {
        // SAFETY: tjson_version returns a valid static C string.
        let version = unsafe { CStr::from_ptr(tjson_version()) }.to_str().unwrap();
        assert_eq!(version, env!("CARGO_PKG_VERSION"));
    }
}
