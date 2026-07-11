//! JS-boundary behavior tests for the wasm binding, run in a real JS engine:
//!
//!     wasm-pack test --node -- --test wasm_boundary
//!
//! (stage 4 of test-all.sh). Host `cargo test` compiles this file to nothing.
//!
//! These are written against the *contract*, not the implementation:
//! JSON.stringify semantics (toJSON honored, undefined means absent), loud
//! key-named errors for values with no declared JSON form (Map, Set, class
//! instances, NaN/Infinity, ill-formed strings), exact BigInt round-tripping,
//! and loud-by-default handling of integers a JS number cannot hold.
#![cfg(target_arch = "wasm32")]

use tjson::wasm::{from_json, parse, stringify, to_json};
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

/// Build a live JS value from a JS expression, exactly as a JS caller would.
fn js(expr: &str) -> JsValue {
    js_sys::eval(expr).expect("test JS expression must evaluate")
}

/// Extract the message from a thrown JS Error.
fn error_message(err: JsValue) -> String {
    String::from(js_sys::Error::from(err).message())
}

/// Stringify of a value expected to fail: return the error message.
fn stringify_err(expr: &str) -> String {
    let out = stringify(js(expr), JsValue::UNDEFINED);
    error_message(out.expect_err("contract says this value must be rejected"))
}

/// typeof + string form of a JS value, via JS itself.
fn js_typeof_and_string(v: &JsValue) -> (String, String) {
    let f = js_sys::Function::new_with_args("v", "return typeof v + '|' + String(v);");
    let out = f.call1(&JsValue::NULL, v).unwrap().as_string().unwrap();
    let (t, s) = out.split_once('|').unwrap();
    (t.to_string(), s.to_string())
}

// ---- options handling (behavior carried over from the previous pipeline) ----

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
    let out = from_json(r#"{"a":1,"b":2}"#, js(r#"({canonical:true})"#)).unwrap();
    assert_eq!(out.lines().count(), 2, "canonical must give one pair per line: {out:?}");
}

#[wasm_bindgen_test]
fn unknown_option_fields_are_tolerated() {
    let out = from_json(r#"{"a":1,"b":2}"#, js(r#"({notAnOption:1, canonical:true})"#)).unwrap();
    assert_eq!(out.lines().count(), 2, "known fields must still apply: {out:?}");
}

#[wasm_bindgen_test]
fn array_options_are_rejected() {
    let err = from_json(r#"{"a":1}"#, js("[true]")).unwrap_err();
    assert!(error_message(err).contains("must be an object"));
}

#[wasm_bindgen_test]
fn scalar_options_are_rejected() {
    let err = from_json(r#"{"a":1}"#, js("40")).unwrap_err();
    assert!(error_message(err).contains("must be an object"));
}

#[wasm_bindgen_test]
fn retired_option_name_gets_migration_hint() {
    let err = from_json(r#"{"a":1}"#, js(r#"({tableMinCols:2})"#)).unwrap_err();
    assert!(error_message(err).contains("tableMinColumns"));
}

#[wasm_bindgen_test]
fn invalid_option_value_is_rejected() {
    let err = from_json(r#"{"a":1}"#, js(r#"({wrapWidth:"wide"})"#)).unwrap_err();
    let msg = error_message(err);
    assert!(msg.contains("invalid option value"), "{msg}");
}

#[wasm_bindgen_test]
fn stringify_shares_the_strict_options_path() {
    // stringify and fromJson use one options pipeline: canonical applies,
    // and non-object options are rejected the same way.
    let out = stringify(js(r#"({a:1,b:2})"#), js(r#"({canonical:true})"#)).unwrap();
    assert_eq!(out.lines().count(), 2, "{out:?}");
    let err = stringify(js(r#"({a:1})"#), js("[true]")).unwrap_err();
    assert!(error_message(err).contains("must be an object"));
}

#[wasm_bindgen_test]
fn parse_options_must_be_an_object() {
    for bad in ["[1]", "40", r#""x""#] {
        let err = parse("  a:1", js(bad)).unwrap_err();
        assert!(error_message(err).contains("must be an object"), "options {bad}");
    }
}

#[wasm_bindgen_test]
fn parse_bigints_option_must_be_a_boolean() {
    let err = parse("  a:1", js(r#"({bigints:1})"#)).unwrap_err();
    assert!(error_message(err).contains("must be a boolean"));
    // Real booleans and absence are fine.
    parse("  a:1", js(r#"({bigints:false})"#)).unwrap();
    parse("  a:1", js(r#"({})"#)).unwrap();
}

// ---- JSON.stringify semantics: declared intent is honored ----

#[wasm_bindgen_test]
fn to_json_methods_are_honored() {
    // Date declares its own JSON form (toJSON -> ISO string); honoring it is
    // honoring the value author's explicit intent.
    let out = stringify(js("({d: new Date(0)})"), JsValue::UNDEFINED).unwrap();
    assert!(out.contains("1970-01-01T00:00:00.000Z"), "Date must serialize via toJSON: {out:?}");
}

#[wasm_bindgen_test]
fn undefined_object_values_mean_absent() {
    // undefined is JS's spelling of "not present": the key is dropped, not
    // rewritten to null.
    let out = stringify(js("({a: undefined, b: 1})"), JsValue::UNDEFINED).unwrap();
    assert!(out.contains("b:1"), "{out:?}");
    assert!(!out.contains('a'), "undefined-valued key must be dropped, got: {out:?}");
}

#[wasm_bindgen_test]
fn key_order_is_preserved() {
    let out = stringify(js(r#"({z:1, a:2, m:3})"#), js(r#"({canonical:true})"#)).unwrap();
    let keys: Vec<char> = out.lines().filter_map(|l| l.trim_start().chars().next()).collect();
    assert_eq!(keys, ['z', 'a', 'm'], "insertion order must survive: {out:?}");
}

// ---- values with no declared JSON form fail loudly, naming the key ----

#[wasm_bindgen_test]
fn maps_are_rejected_loudly() {
    let msg = stringify_err("({m: new Map([['k','v']])})");
    assert!(msg.contains("Map"), "must name the type: {msg}");
    assert!(msg.contains("'m'"), "must name the key: {msg}");
}

#[wasm_bindgen_test]
fn sets_are_rejected_loudly() {
    let msg = stringify_err("({s: new Set([1,2])})");
    assert!(msg.contains("Set"), "{msg}");
    assert!(msg.contains("'s'"), "must name the key: {msg}");
}

#[wasm_bindgen_test]
fn class_instances_without_to_json_are_rejected_loudly() {
    let msg = stringify_err("({t: new Uint8Array([1,2,3])})");
    assert!(msg.contains("Uint8Array"), "must name the type: {msg}");
    assert!(msg.contains("'t'"), "must name the key: {msg}");
}

#[wasm_bindgen_test]
fn nan_and_infinity_are_rejected_loudly() {
    let msg = stringify_err("({ratio: NaN})");
    assert!(msg.contains("NaN"), "{msg}");
    assert!(msg.contains("'ratio'"), "must name the key: {msg}");
    let msg = stringify_err("({x: Infinity})");
    assert!(msg.contains("Infinity"), "{msg}");
}

#[wasm_bindgen_test]
fn lone_surrogates_are_rejected_loudly_not_mangled() {
    // The raw wasm string ABI would silently corrupt this to U+FFFD; the
    // contract is a loud error that names the offending key (located by a
    // failure-path-only re-walk — the hot path does no per-string scans).
    let msg = stringify_err(r#"({a: "x\ud800y"})"#);
    assert!(msg.contains("surrogate"), "{msg}");
    assert!(msg.contains("'a'"), "must name the key: {msg}");
}

#[wasm_bindgen_test]
fn ill_formed_object_keys_are_rejected_loudly() {
    // Keys are WTF-16 strings too; JSON.stringify escapes an ill-formed key
    // the same way, serde rejects it, and the diagnostic walk names it as a
    // key rather than a value.
    let msg = stringify_err(r#"({"x\ud800y": 1})"#);
    assert!(msg.contains("surrogate"), "{msg}");
    assert!(msg.contains("key"), "must identify it as a key: {msg}");
}

#[wasm_bindgen_test]
fn cyclic_values_are_rejected() {
    let msg = stringify_err("(() => { const o = {}; o.self = o; return o; })()");
    assert!(msg.to_lowercase().contains("circular"), "{msg}");
}

#[wasm_bindgen_test]
fn top_level_unserializable_is_rejected() {
    let msg = stringify_err("(function(){})");
    assert!(msg.contains("serializable"), "{msg}");
}

// ---- exact integers: BigInt out, BigInt back ----

#[wasm_bindgen_test]
fn bigint_serializes_as_exact_digits() {
    // 2^53 + 1: the first integer a JS number cannot hold.
    let out = stringify(js("({n: 9007199254740993n})"), JsValue::UNDEFINED).unwrap();
    assert!(out.contains("9007199254740993"), "exact digits must survive: {out:?}");
    // And a genuinely huge one, far beyond f64.
    let out = stringify(js("({n: 123456789012345678901234567890n})"), JsValue::UNDEFINED).unwrap();
    assert!(out.contains("123456789012345678901234567890"), "{out:?}");
}

#[wasm_bindgen_test]
fn safe_range_bigints_serialize_without_raw_json_machinery() {
    // 42n is exactly representable as a plain number: it takes the
    // dependency-free fast path (which also works on runtimes without
    // JSON.rawJSON) and round-trips as an ordinary JS number.
    let out = stringify(js("({n: 42n})"), JsValue::UNDEFINED).unwrap();
    assert!(out.contains("n:42"), "{out:?}");
    let back = parse(&out, JsValue::UNDEFINED).unwrap();
    let n = js_sys::Reflect::get(&back, &"n".into()).unwrap();
    let (ty, s) = js_typeof_and_string(&n);
    assert_eq!(ty, "number");
    assert_eq!(s, "42");
}

#[wasm_bindgen_test]
fn bigint_round_trips_exactly_with_bigints_option() {
    let tjson_text = stringify(js("({n: 9007199254740993n})"), JsValue::UNDEFINED).unwrap();
    let back = parse(&tjson_text, js("({bigints:true})")).unwrap();
    let n = js_sys::Reflect::get(&back, &"n".into()).unwrap();
    let (ty, s) = js_typeof_and_string(&n);
    assert_eq!(ty, "bigint", "must revive as BigInt");
    assert_eq!(s, "9007199254740993", "must revive the exact value");
}

#[wasm_bindgen_test]
fn unsafe_integers_error_loudly_by_default() {
    // Silent f64 corruption of our own exact output is the one forbidden
    // outcome: without the opt-in, this must throw, and the message must
    // point at the fix.
    let err = parse("  n:9007199254740993", JsValue::UNDEFINED).unwrap_err();
    let msg = error_message(err);
    assert!(msg.contains("exactly"), "{msg}");
    assert!(msg.contains("'n'"), "must name the key: {msg}");
    assert!(msg.contains("bigints"), "message must name the opt-in: {msg}");
}

#[wasm_bindgen_test]
fn safe_integers_stay_plain_numbers_even_with_bigints_option() {
    let back = parse("  n:42", js("({bigints:true})")).unwrap();
    let n = js_sys::Reflect::get(&back, &"n".into()).unwrap();
    let (ty, s) = js_typeof_and_string(&n);
    assert_eq!(ty, "number");
    assert_eq!(s, "42");
}

#[wasm_bindgen_test]
fn integers_that_overflow_f64_still_error_or_revive() {
    // 400 nines: JSON.parse alone would produce Infinity (isInteger(Infinity)
    // is false — the hole Grok's review caught). Default: loud. With
    // bigints: revived exactly from source digits.
    let digits = "9".repeat(400);
    let doc = format!("  n:{digits}");
    let err = parse(&doc, JsValue::UNDEFINED).unwrap_err();
    let msg = error_message(err);
    assert!(msg.contains("bigints"), "must point at the opt-in: {msg}");

    let back = parse(&doc, js("({bigints:true})")).unwrap();
    let n = js_sys::Reflect::get(&back, &"n".into()).unwrap();
    let (ty, s) = js_typeof_and_string(&n);
    assert_eq!(ty, "bigint");
    assert_eq!(s, digits, "must revive the exact digits");
}

#[wasm_bindgen_test]
fn float_notation_that_overflows_f64_errors_instead_of_becoming_infinity() {
    // 1e400 is float notation, but JSON.parse would silently turn it into
    // Infinity — which does not round-trip even approximately, so it throws
    // regardless of the bigints option.
    for opts in ["undefined", "({bigints:true})"] {
        let err = parse("  x:1e400", js(opts)).unwrap_err();
        let msg = error_message(err);
        assert!(msg.contains("Infinity"), "options {opts}: {msg}");
    }
}

#[wasm_bindgen_test]
fn exponent_form_floats_stay_numbers() {
    // 1e30 is float notation: the author wrote a float, it stays a number —
    // only digit-form integers are policed.
    let back = parse("  x:1e30", JsValue::UNDEFINED).unwrap();
    let n = js_sys::Reflect::get(&back, &"x".into()).unwrap();
    let (ty, _) = js_typeof_and_string(&n);
    assert_eq!(ty, "number");
}

// ---- round trip smoke ----

#[wasm_bindgen_test]
fn parse_and_to_json_round_trip() {
    let value = parse("  name: Alice", JsValue::UNDEFINED).expect("valid TJSON must parse");
    assert!(value.is_object());
    let json = to_json("  name: Alice").unwrap();
    assert!(json.contains("Alice"));
}

#[wasm_bindgen_test]
fn to_json_carries_huge_integers_exactly() {
    // The string-to-string path has no JS number in it and must be exact
    // with no options needed.
    let json = to_json("  n:123456789012345678901234567890").unwrap();
    assert!(json.contains("123456789012345678901234567890"), "{json:?}");
}
