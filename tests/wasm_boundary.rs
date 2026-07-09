//! JS-boundary behavior tests for the wasm binding, run in a real JS engine:
//!
//!     wasm-pack test --node -- --test wasm_boundary
//!
//! (stage 4 of test-all.sh). Host `cargo test` compiles this file to nothing.
//! These pin the decisions that only execute on the JS side: null/undefined
//! options mean defaults, unknown fields are tolerated, non-objects and
//! retired option names throw.
#![cfg(target_arch = "wasm32")]

use tjson::wasm::{from_json, parse, stringify, to_json};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

/// Build a live JS value from JSON text, exactly as a JS caller would.
fn js(text: &str) -> JsValue {
    js_sys::JSON::parse(text).expect("test JSON must parse")
}

/// Extract the message from a thrown JS Error.
fn error_message(err: JsValue) -> String {
    String::from(js_sys::Error::from(err).message())
}

#[wasm_bindgen_test]
fn null_and_undefined_options_mean_defaults() {
    let out = from_json(r#"{"name":"Alice"}"#, JsValue::NULL).expect("null options must work");
    assert!(out.contains("Alice"));
    let out =
        from_json(r#"{"name":"Alice"}"#, JsValue::UNDEFINED).expect("undefined options must work");
    assert!(out.contains("Alice"));
}

#[wasm_bindgen_test]
fn object_options_are_applied() {
    // canonical => one pair per line.
    let out = from_json(r#"{"a":1,"b":2}"#, js(r#"{"canonical":true}"#)).unwrap();
    assert_eq!(out.lines().count(), 2, "canonical must give one pair per line: {out:?}");
}

#[wasm_bindgen_test]
fn unknown_option_fields_are_tolerated() {
    // The documented JS options-bag contract: extra keys are legal and the
    // known keys still apply.
    let out =
        from_json(r#"{"a":1,"b":2}"#, js(r#"{"notAnOption":1,"canonical":true}"#)).unwrap();
    assert_eq!(out.lines().count(), 2, "known fields must still apply: {out:?}");
}

#[wasm_bindgen_test]
fn array_options_are_rejected() {
    // Without the shape guard, [true] would be applied positionally as
    // {canonical: true}.
    let err = from_json(r#"{"a":1}"#, js("[true]")).unwrap_err();
    let message = error_message(err);
    assert!(message.contains("must be an object"), "got: {message}");
}

#[wasm_bindgen_test]
fn scalar_options_are_rejected() {
    let err = from_json(r#"{"a":1}"#, js("40")).unwrap_err();
    let message = error_message(err);
    assert!(message.contains("must be an object"), "got: {message}");
}

#[wasm_bindgen_test]
fn retired_option_name_gets_migration_hint() {
    let err = from_json(r#"{"a":1}"#, js(r#"{"tableMinCols":2}"#)).unwrap_err();
    let message = error_message(err);
    assert!(message.contains("tableMinColumns"), "got: {message}");
}

#[wasm_bindgen_test]
fn invalid_option_value_is_rejected() {
    let err = from_json(r#"{"a":1}"#, js(r#"{"wrapWidth":"wide"}"#)).unwrap_err();
    let message = error_message(err);
    assert!(message.contains("invalid option value"), "got: {message}");
}

#[wasm_bindgen_test]
fn stringify_applies_the_same_options_path() {
    // stringify shares parse_options with from_json; pin one case through it.
    let out = stringify(js(r#"{"a":1,"b":2}"#), js(r#"{"canonical":true}"#)).unwrap();
    assert_eq!(out.lines().count(), 2);
    let err = stringify(js(r#"{"a":1}"#), js("[true]")).unwrap_err();
    assert!(error_message(err).contains("must be an object"));
}

#[wasm_bindgen_test]
fn parse_and_to_json_round_trip() {
    let value = parse("  name: Alice").expect("valid TJSON must parse");
    assert!(value.is_object());
    let json = to_json("  name: Alice").unwrap();
    assert!(json.contains("Alice"));
}
