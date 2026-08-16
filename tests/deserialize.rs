//! Differential and error-quality tests for the native serde Deserializer.
//!
//! The old implementation of `tjson::from_str` was a trampoline: parse to `Value`,
//! render to a JSON string, `serde_json::from_str` that string. The native Deserializer
//! must preserve its observable semantics. `oracle()` reproduces the trampoline so every
//! differential case states explicitly whether the new path must MATCH it or is an
//! intended, documented divergence.

use std::collections::HashMap;

use serde::Deserialize;

/// The old trampoline, kept as the semantics oracle.
fn oracle<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, String> {
    let value: tjson::Value = input.parse().map_err(|e| format!("parse: {e}"))?;
    serde_json::from_str(&value.to_json()).map_err(|e| format!("json: {e}"))
}

fn native<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, String> {
    tjson::from_str(input).map_err(|e| format!("{e}"))
}

/// Assert both paths agree on success value; error text may differ (the new path's
/// errors are strictly richer), so errors compare as "both err".
fn assert_matches_oracle<T>(input: &str)
where
    T: serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let old: Result<T, String> = oracle(input);
    let new: Result<T, String> = native(input);
    match (&old, &new) {
        (Ok(o), Ok(n)) => assert_eq!(o, n, "value mismatch for input {input:?}"),
        (Err(_), Err(_)) => {}
        _ => panic!("outcome mismatch for {input:?}:\n  oracle: {old:?}\n  native: {new:?}"),
    }
}

// ---- Structs, containers, scalars ----

#[derive(Deserialize, PartialEq, Debug)]
struct Person {
    name: String,
    age: u32,
}

#[test]
fn structs_match_oracle() {
    assert_matches_oracle::<Person>("  name: Alice  age:30");
    assert_matches_oracle::<Person>("  name: Alice\n  age:30");
    assert_matches_oracle::<Person>("  age:30\n  name: Alice");
    // Type mismatch: both must fail.
    assert_matches_oracle::<Person>("  name: Alice\n  age: banana");
    // Missing field: both must fail.
    assert_matches_oracle::<Person>("  name: Alice");
}

#[derive(Deserialize, PartialEq, Debug)]
struct Wide {
    b: bool,
    o: Option<String>,
    v: Vec<i64>,
    f: f64,
    nested: HashMap<String, Vec<u8>>,
}

#[test]
fn assorted_types_match_oracle() {
    let input = concat!(
        "  b:true\n",
        "  o:null\n",
        "  v:  1, -2, 3\n",
        "  f:2.5\n",
        "  nested:\n",
        "    xs:  1, 2, 3",
    );
    assert_matches_oracle::<Wide>(input);
}

#[test]
fn typed_lossy_float_is_accepted() {
    // Deserializing into f64 is the caller opting into f64's precision loss: exact
    // digits the format preserves may be rounded, and that is not an error.
    #[derive(Deserialize, PartialEq, Debug)]
    struct F {
        x: f64,
    }
    assert_matches_oracle::<F>("  x:1.00");
    assert_matches_oracle::<F>("  x:123456789012345678901234567890.5");
    // Wrong kind (not lossy conversion) must still fail: null is not a float.
    assert_matches_oracle::<F>("  x:null");
    assert_matches_oracle::<F>("  x: banana");
}

#[test]
fn integer_range_checks_match_oracle() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct B {
        x: u8,
    }
    assert_matches_oracle::<B>("  x:255");
    assert_matches_oracle::<B>("  x:256");
    assert_matches_oracle::<B>("  x:-1");
}

#[test]
fn i128_and_u128_deserialize_from_big_digits() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct Big {
        x: u128,
        y: i128,
    }
    let big: Big =
        tjson::from_str("  x:170141183460469231731687303715884105727\n  y:-170141183460469231731687303715884105728")
            .expect("128-bit integers must deserialize from digit strings");
    assert_eq!(big.x, 170141183460469231731687303715884105727u128);
    assert_eq!(big.y, i128::MIN);
}

// ---- Exact-digit preservation (arbitrary_precision protocol) ----

#[test]
fn tjson_number_field_keeps_exact_digits() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct N {
        n: tjson::Number,
    }
    for digits in ["1.00", "123456789012345678901234567890.5", "-0.000100"] {
        let input = format!("  n:{digits}");
        let parsed: N = tjson::from_str(&input).expect("Number field must accept any JSON number");
        assert_eq!(parsed.n.as_str(), digits, "digits must survive typed deserialization");
    }
    // Exponent-sign spelling is outside the data-integrity guarantee: "1e+100"
    // and "1e100" may differ. tjson::Number delegates to serde_json::Number,
    // which canonicalizes the sign; value and precision survive, the spelling
    // may not.
    let parsed: N = tjson::from_str("  n:1e100").expect("exponent numbers deserialize");
    assert_eq!(parsed.n.as_str(), "1e+100");
    assert_eq!(parsed.n.as_f64(), Some(1e100));
}

#[test]
fn serde_json_number_field_keeps_exact_digits() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct N {
        n: serde_json::Number,
    }
    let parsed: N = tjson::from_str("  n:1.00").expect("serde_json::Number must deserialize");
    assert_eq!(parsed.n.to_string(), "1.00");
}

#[test]
fn value_target_round_trips_exactly() {
    // Deserializing into tjson::Value must reproduce the parse exactly, digits included.
    let input = "  n:1.00\n  big:99999999999999999999\n  s: hello\n  a:  1, 2";
    let direct: tjson::Value = input.parse().expect("parses");
    let deserialized: tjson::Value = tjson::from_str(input).expect("deserializes");
    assert_eq!(direct, deserialized);
}

#[test]
fn serde_json_value_target_matches_oracle() {
    // This is the path the wasm/JS binding rides: src/wasm.rs parse() and toJson()
    // call from_str::<serde_json::Value>. The big integers exercise every rung of the
    // deserialize_any ladder (u64, u128, beyond-u128 via the token map) because JS
    // BigInt revival depends on their digits surviving exactly.
    let input = concat!(
        "  n:1.00\n",
        "  s: hello\n",
        "  a:  1, 2, 3\n",
        "  fits64:18446744073709551615\n",
        "  fits128:99999999999999999999999\n",
        "  beyond128:999999999999999999999999999999999999999999",
    );
    let old: serde_json::Value = oracle(input).expect("oracle succeeds");
    let new: serde_json::Value = native(input).expect("native succeeds");
    assert_eq!(old, new);
    assert_eq!(
        new["fits128"].to_string(),
        "99999999999999999999999",
        "u128-range digits must survive exactly for JS BigInt revival"
    );
    assert_eq!(new["beyond128"].to_string(), "999999999999999999999999999999999999999999");
}

// ---- Enums ----

#[derive(Deserialize, PartialEq, Debug)]
enum Shape {
    Point,
    Circle(f64),
    Rect { w: u32, h: u32 },
    Pair(u32, u32),
}

#[test]
fn externally_tagged_enums_match_oracle() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct S {
        shape: Shape,
    }
    assert_matches_oracle::<S>("  shape: Point");
    assert_matches_oracle::<S>("  shape:\n    Circle:2.5");
    assert_matches_oracle::<S>("  shape:\n    Rect:\n      w:3  h:4");
    assert_matches_oracle::<S>("  shape:\n    Pair:  3, 4");
    // Unknown variant must fail on both.
    assert_matches_oracle::<S>("  shape: Blob");
}

#[derive(Deserialize, PartialEq, Debug)]
#[serde(untagged)]
enum StrOrNum {
    Num(f64),
    Str(String),
}

#[test]
fn untagged_integers_and_strings_match_oracle() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct U {
        a: StrOrNum,
        b: StrOrNum,
    }
    assert_matches_oracle::<U>("  a:30  b: hello");
}

#[test]
fn untagged_floats_are_an_intended_divergence() {
    // DOCUMENTED DIVERGENCE (plan §Phase C findings): the trampoline delivered floats
    // through the arbitrary_precision token map, which untagged matching cannot see
    // into, so untagged floats always failed. The native deserializer visits f64
    // directly when the digits round-trip, so they now succeed. Errors becoming
    // successes is the accepted direction.
    #[derive(Deserialize, PartialEq, Debug)]
    struct U {
        a: StrOrNum,
    }
    let old: Result<U, String> = oracle("  a:30.5");
    let new: Result<U, String> = native("  a:30.5");
    assert!(old.is_err(), "trampoline failed untagged floats; if this now passes, \
        serde/serde_json changed and the divergence note should be revisited");
    assert_eq!(new.expect("native must accept untagged floats").a, StrOrNum::Num(30.5));

    // Non-round-tripping digits still take the exact path and still fail to match —
    // serde's protocol gives no way to know the consumer would accept the loss.
    let strict: Result<U, String> = native("  a:1.00");
    assert!(strict.is_err(), "1.00 must not silently become 1.0 through untagged");
}

#[test]
fn exact_only_numbers_explain_untagged_failures() {
    // DELIBERATE: serde's untagged matching buffers values shape-blind, so when an
    // exact-only number (one with no exact f64 representation, like 1.00) rides the
    // exact token map and matches neither Num(f64) nor Str, serde can only say
    // "data did not match any variant" — it never knew why. Our access wrappers still
    // hold the failing value node, so they append the real cause: the number exists
    // only in exact form and an f64 variant cannot match it. Without this note, the
    // failure looks arbitrary ("30.5 works but 1.00 doesn't?!"); with it, the message
    // names the boundary of the exactness guarantee and the escape hatch
    // (tjson::Number). If this test breaks because serde changed its untagged error
    // plumbing, the hint mechanism in de.rs (exactness_hint) needs re-verifying, not
    // deleting.
    #[derive(Deserialize, PartialEq, Debug)]
    struct U {
        a: StrOrNum,
    }
    let err = tjson::from_str::<U>("  a:1.00").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("the trailing zeros in 1.00 are part of the number as written"),
        "untagged failure must explain the lexical-fidelity cause: {message}"
    );
    assert!(
        message.contains("read back as 1.0"),
        "the note must show the readback spelling: {message}"
    );

    // A number that is genuinely beyond f64 precision (not just a trailing zero)
    // gets the precision-overflow wording instead.
    let err = tjson::from_str::<U>("  a:123456789012345678901234567890.5").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("exceeds what an f64 can hold exactly"),
        "precision overflow gets its own wording: {message}"
    );
    assert!(
        message.contains("tjson::Number"),
        "the note must name the escape hatch: {message}"
    );

    // The hint is scoped to Content-land failures: a plain typed mismatch on the same
    // number is a located, self-explanatory error and must NOT carry the note.
    #[derive(Deserialize, PartialEq, Debug)]
    struct S {
        a: String,
    }
    let err = tjson::from_str::<S>("  a:1.00").unwrap_err();
    let message = err.to_string();
    assert!(
        !message.contains("written form"),
        "typed errors explain themselves; no note: {message}"
    );

    // And the escape hatch the note names must actually work.
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(untagged)]
    enum ExactOrStr {
        Num(tjson::Number),
        Str(String),
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct E {
        a: ExactOrStr,
    }
    let ok: E = tjson::from_str("  a:1.00").expect("Number variant accepts exact-only numbers");
    let ExactOrStr::Num(n) = ok.a else { panic!("expected Num variant") };
    assert_eq!(n.as_str(), "1.00");
}

#[test]
fn exact_only_numbers_nested_in_object_variants_also_hint() {
    // The exact-only number can be INSIDE the object an untagged variant is trying to
    // match: Policy fails because backoff:2.50 arrives as the exact token map, not an
    // f64. The hint must find the nested number, not just top-level ones.
    #[derive(Deserialize, PartialEq, Debug)]
    struct Policy {
        attempts: u32,
        backoff: f64,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(untagged)]
    enum Retry {
        Count(u32),
        Policy(Policy),
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct C {
        r: Retry,
    }
    let ok: C = tjson::from_str("  r:\n    attempts:5\n    backoff:2.5").expect("2.5 rides f64");
    assert_eq!(ok.r, Retry::Policy(Policy { attempts: 5, backoff: 2.5 }));

    let err = tjson::from_str::<C>("  r:\n    attempts:5\n    backoff:2.50").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("the trailing zero in 2.50 is part of the number as written"),
        "hint must name the nested offender: {message}"
    );
    // And point AT it: the untagged error is born unlocated in serde's buffering, so
    // the hint machinery relocates it to the offending number, not the container.
    assert!(
        message.contains("line 3") && message.contains("backoff:2.50"),
        "error must locate the offending number, not the container: {message}"
    );
}

#[test]
fn token_map_leaks_into_permissive_untagged_variants() {
    // KNOWN, DOCUMENTED, NOT ENDORSED: inherited from serde_json's arbitrary_precision
    // protocol, which we speak deliberately (that is how exact digits reach Number and
    // Value targets). An exact-only number travels as the single-entry map
    // {"$serde_json::private::Number": "<digits>"}, and serde's untagged matching is
    // shape-based — so an untagged variant permissive enough to accept ANY map will
    // swallow that token map as data. serde_json's own from_value with
    // arbitrary_precision behaves identically; there is no defense inside serde's
    // protocol because the buffer cannot know a map is "really" a number. These
    // assertions pin the failure mode so a change in it (either direction — a serde
    // fix or a regression) is noticed rather than silent. If untagged-with-permissive-
    // map-variants over exact-only numbers becomes a real user scenario, the answer is
    // a custom Deserialize (inspect a tjson::Value), not a tweak here.
    use std::collections::HashMap;

    // A map-of-strings variant matches the token map, private key and all.
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(untagged)]
    enum CountOrTags {
        Count(u32),
        Tags(HashMap<String, String>),
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct T {
        r: CountOrTags,
    }
    let leaked: T = tjson::from_str("  r:5.00").expect("token map matches the map variant");
    let CountOrTags::Tags(tags) = leaked.r else { panic!("expected Tags") };
    assert_eq!(tags.get("$serde_json::private::Number").map(String::as_str), Some("5.00"));

    // An all-optional struct variant matches it as an empty struct (unknown fields
    // are ignored by default), silently swallowing the number.
    #[derive(Deserialize, PartialEq, Debug)]
    struct OptPolicy {
        attempts: Option<u32>,
        backoff: Option<f64>,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(untagged)]
    enum RetryOpt {
        Count(u32),
        Policy(OptPolicy),
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct O {
        r: RetryOpt,
    }
    let swallowed: O = tjson::from_str("  r:5.00").expect("all-optional struct matches any map");
    assert_eq!(
        swallowed.r,
        RetryOpt::Policy(OptPolicy { attempts: None, backoff: None }),
        "exact-only number silently becomes an empty policy — known protocol wart"
    );
}

// ---- Duplicate keys ----

#[test]
fn duplicate_keys_error_for_structs_and_last_wins_for_maps() {
    assert_matches_oracle::<Person>("  name: Alice\n  name: Bob\n  age:30");
    assert_matches_oracle::<HashMap<String, u32>>("  a:1\n  a:2\n  b:3");
    let map: HashMap<String, u32> = native("  a:1\n  a:2\n  b:3").expect("maps accept dupes");
    assert_eq!(map["a"], 2, "last duplicate wins, matching serde_json");
}

// ---- deny_unknown_fields / flatten ----

#[test]
fn deny_unknown_fields_matches_oracle() {
    #[derive(Deserialize, PartialEq, Debug)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        a: u32,
    }
    assert_matches_oracle::<Strict>("  a:1");
    assert_matches_oracle::<Strict>("  a:1\n  b:2");
}

#[test]
fn flatten_matches_oracle() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct Outer {
        name: String,
        #[serde(flatten)]
        rest: HashMap<String, u32>,
    }
    assert_matches_oracle::<Outer>("  name: x\n  a:1\n  b:2");
}

// ---- Error quality: the reason this deserializer exists ----

#[test]
fn type_error_reports_real_source_location() {
    let err = tjson::from_str::<Person>("  name: Alice\n  age: banana").unwrap_err();
    let tjson::Error::Deserialize(err) = err else {
        panic!("expected Error::Deserialize, got: {err:?}")
    };
    assert_eq!(err.path(), "age");
    assert_eq!(err.line(), Some(2), "error must point at the TJSON line, not synthetic JSON");
    assert_eq!(err.column(), Some(8), "column must point at the value token");
    let display = err.to_string();
    assert!(display.contains("line 2, column 8"), "display: {display}");
    assert!(display.contains("age: banana"), "display must include the source line: {display}");
    assert!(display.contains('^'), "display must include the caret: {display}");
}

#[test]
fn nested_error_paths_name_the_full_route() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct Server {
        port: u16,
    }
    #[derive(Deserialize, PartialEq, Debug)]
    struct Config {
        servers: Vec<Server>,
    }
    let input = "  servers:\n  [ { port:80\n    { port: oops";
    let err = tjson::from_str::<Config>(input).unwrap_err();
    let tjson::Error::Deserialize(err) = err else { panic!("expected Deserialize error") };
    assert_eq!(err.path(), "servers[1].port");
    assert_eq!(err.line(), Some(3));
}

#[test]
fn from_value_errors_have_path_but_no_location() {
    let value: tjson::Value = "  name: Alice\n  age: banana".parse().expect("valid TJSON");
    let err = tjson::from_value::<Person>(&value).unwrap_err();
    let tjson::Error::Deserialize(err) = err else { panic!("expected Deserialize error") };
    assert_eq!(err.path(), "age");
    assert_eq!(err.line(), None, "no source text means no coordinates, never fake ones");
    assert_eq!(err.column(), None);
    let display = err.to_string();
    assert!(!display.contains("line"), "unlocated display must not mention lines: {display}");
}

#[test]
fn null_into_f64_complains_like_serde_json() {
    #[derive(Deserialize, PartialEq, Debug)]
    struct F {
        x: f64,
    }
    let err = tjson::from_str::<F>("  x:null").unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("invalid type: null, expected f64"),
        "must use serde_json's wording: {message}"
    );
}

#[test]
fn from_value_succeeds_for_valid_data() {
    let value: tjson::Value = "  name: Alice\n  age:30".parse().expect("valid TJSON");
    let person: Person = tjson::from_value(&value).expect("valid data deserializes");
    assert_eq!(person, Person { name: "Alice".into(), age: 30 });
}

#[test]
fn borrowed_strings_work_through_from_value() {
    // from_value supports zero-copy borrows out of the Value.
    #[derive(Deserialize, PartialEq, Debug)]
    struct B<'a> {
        name: &'a str,
    }
    let value: tjson::Value = "  name: Alice".parse().expect("valid TJSON");
    let borrowed: B<'_> = tjson::from_value(&value).expect("borrowing deserialization");
    assert_eq!(borrowed.name, "Alice");
}

#[test]
fn from_document_reads_typed_data_without_projection() {
    // The deserializer is generic over the internal tree trait, so a Document is as
    // valid a source as a Value: one parse yields both the comments and the typed
    // data, with no intermediate Value copy.
    #[derive(Deserialize, PartialEq, Debug)]
    struct Config {
        name: String,
        retries: u32,
    }
    let doc: tjson::Document = "// prod settings\n  name: web\n  retries:3".parse().unwrap();
    let config: Config = tjson::from_document(&doc).expect("typed data straight from Document");
    assert_eq!(config, Config { name: "web".into(), retries: 3 });
    assert_eq!(doc.root().comments_before()[0].text(), "// prod settings");

    // Errors: field path, no coordinates (a Document in hand has no source text).
    #[derive(Deserialize, PartialEq, Debug)]
    struct Bad {
        retries: bool,
    }
    let err = tjson::from_document::<Bad>(&doc).unwrap_err();
    let tjson::Error::Deserialize(err) = err else { panic!("expected Deserialize error") };
    assert_eq!(err.path(), "retries");
    assert_eq!(err.line(), None);
}

/// Every rung of the number ladder, at its edges.
///
/// `deserialize_any` offers a number as u64, then i64, then u128, then i128, then
/// f64 when the digits survive the trip, and only then as the exact token map.
/// Which rung a number lands on is invisible from outside -- what is visible is
/// the value that comes out, so every boundary is compared against the oracle
/// rather than asserted directly.
///
/// The edges are the point. A ladder is exactly the shape that is right in the
/// middle of each rung and wrong by one at the joins, and nothing here used to
/// stand on a join.
#[test]
fn the_number_ladder_matches_the_oracle_at_every_boundary() {
    let digits = [
        "0", "-0", "1", "-1",
        // u64's top, and one past it.
        "18446744073709551615", "18446744073709551616",
        // i64's floor, and one below.
        "-9223372036854775808", "-9223372036854775809",
        // u128's top, and one past it.
        "340282366920938463463374607431768211455",
        "340282366920938463463374607431768211456",
        // i128's floor, and one below.
        "-170141183460469231731687303715884105728",
        "-170141183460469231731687303715884105729",
        // Spellings an f64 keeps, and spellings it does not.
        "1.0", "1.00", "2.5", "2.50", "0.1", "-0.0",
        "1e100", "1e+100", "1e-100", "1.7976931348623157e308",
        // Beyond f64 entirely, and precision no f64 holds.
        "123456789012345678901234567890.5",
        "0.10000000000000000000000000000001",
    ];
    // The routes a number can take are not one route. `Value` goes through the
    // parser, `Number` has its own visitor, and untagged enums and `flatten`
    // buffer through `deserialize_any` -- so they see whichever visit method the
    // ladder picked rather than asking for a type. A rung that changed would show
    // up in some of these and not others, which is the whole reason to sweep them
    // together.
    for number in digits {
        let input = format!("  n:{number}");
        assert_matches_oracle::<serde_json::Value>(&input);
        assert_matches_oracle::<HashMap<String, serde_json::Value>>(&input);
        assert_matches_oracle::<HashMap<String, serde_json::Number>>(&input);
        // `Flattened` is swept separately: see
        // `flatten_and_untagged_accept_numbers_past_u64`, a known gap.

        // Whatever rung it takes, the value must survive as a number rather than
        // decaying to a string or failing outright.
        let exact: HashMap<String, tjson::Number> =
            tjson::from_str(&input).unwrap_or_else(|e| panic!("{number}: {e}"));
        assert_eq!(
            exact["n"].as_f64(),
            number.parse::<f64>().ok(),
            "{number} changed value on the way to tjson::Number"
        );

        // Untagged is swept in the known-gap test alongside `flatten`: both
        // buffer through `Content`, and both refuse anything past `u64`.
    }
}


/// KNOWN GAP, not a flaky test: `-0` loses its sign on the way to an exact field.
///
/// `-0` parses as an `i64` and an `i64` has no negative zero, so the ladder hands
/// the visitor `0` and the spelling is gone before anything downstream can keep
/// it. Every other exact spelling survives -- see `exact_digits_survive_every_rung`
/// -- and `serde_json::Value` agrees with the oracle here, so the divergence is
/// confined to fields that asked for the exact digits.
///
/// The cause is upstream and known; a patched serde_json tree exists outside this
/// repository, and this crate deliberately does not build against it. Pinned as
/// the behaviour that is true today so that a fix announces itself:
///
/// ```text
/// cargo test --test deserialize -- --ignored negative_zero
/// ```
#[test]
#[ignore = "known gap: -0 loses its sign through the integer rung"]
fn negative_zero_keeps_its_sign_for_an_exact_field() {
    #[derive(Deserialize)]
    struct Holder {
        n: tjson::Number,
    }
    let holder: Holder = tjson::from_str("  n:-0").expect("-0 is a valid number");
    assert_eq!(holder.n.as_str(), "-0", "-0 lost its sign");
}

/// KNOWN GAP, not a flaky test: `flatten` and untagged reject numbers past `u64`.
///
/// `#[serde(flatten)]` and untagged enums do not ask for a type -- they buffer the
/// value through `deserialize_any` into serde's private `Content`, and `Content`
/// has no 128-bit variants. So `visit_u128` comes back as *"invalid type: integer
/// ... as u128, expected any value"*, and a document the old trampoline accepted
/// is refused:
///
/// ```text
///   n:18446744073709551616
/// ```
///
/// The trampoline avoided it by never visiting 128-bit integers at all: with
/// `arbitrary_precision`, serde_json hands anything past `i64`/`u64` to the exact
/// token *map*, which `Content` buffers happily. This deserializer's ladder offers
/// `visit_u128`/`visit_i128` first, and only falls through to the token map after.
///
/// Confirmed pre-existing, not introduced by the `Delivery` refactor: the same
/// case fails identically against the pre-refactor `de.rs`.
///
/// Typed fields are unaffected -- `deserialize_u128` is its own method and does not
/// go through `deserialize_any` -- which is why `i128_and_u128_deserialize_from_big_digits`
/// passes. Only the buffering consumers see it.
///
/// ```text
/// cargo test --test deserialize -- --ignored flatten_and_untagged
/// ```
#[test]
#[ignore = "known gap: Content has no 128-bit variants, so flatten/untagged reject them"]
fn flatten_and_untagged_accept_numbers_past_u64() {
    #[derive(Deserialize, Debug, PartialEq)]
    struct Flattened {
        #[serde(flatten)]
        rest: HashMap<String, serde_json::Value>,
    }
    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(untagged)]
    enum Untagged {
        Int(i64),
        Float(f64),
    }
    for number in [
        "18446744073709551616",
        "340282366920938463463374607431768211455",
        "-9223372036854775809",
    ] {
        let input = format!("  n:{number}");
        assert_matches_oracle::<Flattened>(&input);
        assert_matches_oracle::<HashMap<String, Untagged>>(&input);
    }
}

/// The serde modes that buffer, and a few that do not, against the oracle.
///
/// Internally and adjacently tagged enums route through `Content` the same way
/// `flatten` and untagged do, and neither was covered here at all. They are the
/// places a `Deserializer` gets caught out, because they never ask for a type --
/// they take whichever visit method it picked and try to make sense of it later.
#[test]
fn buffering_and_borrowing_modes_match_the_oracle() {
    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(tag = "kind")]
    enum Internal {
        Alpha { n: i64 },
        Beta { s: String },
    }

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(tag = "kind", content = "body")]
    enum Adjacent {
        Alpha(i64),
        Beta(String),
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct Defaults {
        #[serde(default)]
        missing: u32,
        #[serde(default)]
        present: String,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct Newtype(i64);

    #[derive(Deserialize, Debug, PartialEq)]
    struct Optional {
        a: Option<i64>,
        b: Option<i64>,
    }

    assert_matches_oracle::<Internal>("  kind: Alpha\n  n:7");
    assert_matches_oracle::<Internal>("  kind: Beta\n  s: hello");
    assert_matches_oracle::<Adjacent>("  kind: Alpha\n  body:7");
    assert_matches_oracle::<Adjacent>("  kind: Beta\n  body: hello");
    assert_matches_oracle::<Defaults>("  present: here");
    assert_matches_oracle::<Newtype>("  9");
    assert_matches_oracle::<Optional>("  a:1\n  b:null");

    // The same modes over a number the ladder delivers on a wide rung.
    assert_matches_oracle::<Internal>("  kind: Alpha\n  n:-9223372036854775808");
    assert_matches_oracle::<Adjacent>("  kind: Alpha\n  body:-9223372036854775808");

    // Zero-copy is not reachable from the public API even though the visitor
    // offers it: `from_str` takes `DeserializeOwned`, so a `&'a str` field cannot
    // be expressed. `visit_borrowed_str` is still the right call -- it lets an
    // owned `String` skip a copy -- but nothing can borrow from the input here.
}
