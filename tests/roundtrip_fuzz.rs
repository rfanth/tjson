//! Round-trip fuzz: random value -> render -> parse -> compare.
//!
//! Everything runs against the library directly (no subprocess), so a full sweep
//! is milliseconds rather than minutes and the library is what gets exercised
//! rather than the CLI.
//!
//! The generator is deliberately biased toward the shapes that have historically
//! broken TJSON rather than toward uniform random data: strings holding commas,
//! embedded newlines, quotes, backticks, pipes, leading and doubled spaces,
//! non-ASCII and emoji, and strings long enough to force folding. Keys get the
//! same treatment. Uniform random text finds almost nothing here.
//!
//! Failures are shrunk before reporting: keys are dropped, elements removed and
//! strings halved for as long as the failure survives, so what lands in the
//! assertion message is a minimal reproducer rather than the raw case.
//!
//! The seed is fixed, so a red run reproduces exactly. To explore further, raise
//! `CASES` or change `SEED` -- six seeds at 1500 cases (28,500 round trips each)
//! were clean when this landed, so new failures here are worth taking seriously
//! rather than assuming the sweep is flaky.

use tjson::{RenderOptions, TjsonConfig, Value};

const SEED: u64 = 0x7273_6f6e_5f74_6a73;
const CASES: usize = 400;

// ---------------------------------------------------------------- prng

/// xorshift64*, so the sweep is deterministic without pulling in `rand`.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// True with probability `numerator / 100`.
    fn chance(&mut self, numerator: u64) -> bool {
        self.next_u64() % 100 < numerator
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

// ---------------------------------------------------------------- generation

const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    "india", "juliett", "kilo", "lima", "mike", "november", "oscar", "papa",
];

fn words(rng: &mut Rng, count: usize) -> String {
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(rng.pick(WORDS));
    }
    out
}

/// Strings weighted toward the hazards, not toward uniform noise.
fn gen_string(rng: &mut Rng) -> String {
    match rng.below(16) {
        // comma-separated: the array-separator / fold-point collisions
        0..=2 => {
            let n = 2 + rng.below(11);
            (0..n).map(|_| *rng.pick(WORDS)).collect::<Vec<_>>().join(", ")
        }
        // embedded newlines: multiline strings, folding-quotes
        3 => format!("{}\n{}", words(rng, 3), words(rng, 3)),
        4 => format!("{}\n", words(rng, 2)),
        5 => "\n".to_owned(),
        // characters that are structural somewhere in TJSON
        6 => format!("{} | {}", words(rng, 2), words(rng, 2)),
        7 => format!("{} / {}", words(rng, 2), words(rng, 2)),
        8 => format!("{}`{}", words(rng, 2), words(rng, 2)),
        9 => format!("\"{}\"", words(rng, 2)),
        // whitespace shapes the bare-string rules care about
        10 => format!("  {}", words(rng, 2)),
        11 => format!("{}  {}", words(rng, 1), words(rng, 1)),
        // non-ASCII and wide/multi-byte characters (fold boundary hazards)
        12 => format!("何でも {} 🎉", words(rng, 2)),
        13 => format!("{} 👨‍👩‍👧 café", words(rng, 2)),
        // long enough to force folding at any reasonable width
        14 => {
            let n = 20 + rng.below(12);
            words(rng, n)
        }
        _ => {
            let n = 1 + rng.below(4);
            words(rng, n)
        }
    }
}

fn gen_key(rng: &mut Rng) -> String {
    let base = *rng.pick(WORDS);
    match rng.below(10) {
        0 => format!("{base}?"),
        1 => format!("{base} {}", rng.pick(WORDS)),
        2 => format!("{base}:{base}"),
        3 => format!("{base}, {base}"),
        4 => format!("何でも{base}"),
        5 => words(rng, 14), // long enough to fold a key
        _ => format!("{base}{}", rng.below(100)),
    }
}

fn gen_value(rng: &mut Rng, depth: usize) -> serde_json::Value {
    use serde_json::Value as J;
    if depth >= 3 || rng.chance(45) {
        return match rng.below(10) {
            0..=4 => J::String(gen_string(rng)),
            5 | 6 => J::from(rng.next_u64() as i64 % 100_000),
            7 => J::from((rng.next_u64() % 10_000) as f64 / 16.0),
            8 => J::Bool(rng.chance(50)),
            _ => J::Null,
        };
    }
    if rng.chance(50) {
        let n = rng.below(6);
        J::Array((0..n).map(|_| gen_value(rng, depth + 1)).collect())
    } else {
        let n = rng.below(6);
        let mut map = serde_json::Map::new();
        for _ in 0..n {
            map.insert(gen_key(rng), gen_value(rng, depth + 1));
        }
        J::Object(map)
    }
}

// ---------------------------------------------------------------- option sweep

/// Option sets as JSON, deserialized through the same `TjsonConfig` path the
/// fixtures use, so the sweep exercises the public configuration surface and
/// cannot drift from the option names users actually write.
const OPTION_SETS: &[(&str, &str)] = &[
    ("default", "{}"),
    ("wrap40", r#"{"wrapWidth":40}"#),
    ("wrap20", r#"{"wrapWidth":20}"#),
    ("wrap120", r#"{"wrapWidth":120}"#),
    ("canonical", r#"{"canonical":true}"#),
    ("kv1", r#"{"kvPackMultiple":1}"#),
    ("kv3", r#"{"kvPackMultiple":3}"#),
    ("markers", r#"{"forceMarkers":true}"#),
    ("no-bare", r#"{"bareStrings":"quoted","bareKeys":"none"}"#),
    ("marked-bare", r#"{"bareStrings":"marked"}"#),
    ("arrays-none", r#"{"stringArrayStyle":"none"}"#),
    ("arrays-comma", r#"{"stringArrayStyle":"comma"}"#),
    ("arrays-spaces", r#"{"stringArrayStyle":"spaces"}"#),
    ("ml-transparent", r#"{"multilineStyle":"transparent"}"#),
    ("ml-folding-quotes", r#"{"multilineStyle":"foldingQuotes"}"#),
    ("ml-floating", r#"{"multilineStyle":"floating"}"#),
    ("no-tables", r#"{"tables":false}"#),
    ("no-inline", r#"{"inlineObjects":false,"inlineArrays":false}"#),
    ("glyphs", r#"{"indentGlyphStyle":"fixed","wrapWidth":40}"#),
    ("tables-eager", r#"{"tableMinRows":2,"tableMinColumns":2}"#),
];

fn options_for(config_src: &str) -> RenderOptions {
    let config: TjsonConfig = serde_json::from_str(config_src)
        .unwrap_or_else(|e| panic!("bad option set {config_src}: {e}"));
    config.into()
}

// ---------------------------------------------------------------- the check

/// What went wrong, if anything. `None` means the value survived the trip.
fn failure(json: &serde_json::Value, options: &RenderOptions) -> Option<String> {
    let original: Value = json.clone().into();
    let rendered = original.to_tjson_with(options.clone());

    match rendered.parse::<Value>() {
        Err(e) => Some(format!("parse error: {e}\n--- rendered ---\n{rendered}")),
        Ok(reparsed) if reparsed != original => Some(format!(
            "value changed\n--- rendered ---\n{rendered}\n--- became ---\n{:?}",
            serde_json::Value::from(reparsed)
        )),
        Ok(_) => None,
    }
}

/// Smaller variants of `json`, tried in order, for shrinking.
fn candidates(json: &serde_json::Value) -> Vec<serde_json::Value> {
    use serde_json::Value as J;
    let mut out = Vec::new();
    match json {
        J::Object(map) => {
            for key in map.keys() {
                let mut smaller = map.clone();
                smaller.remove(key);
                out.push(J::Object(smaller));
            }
            for (key, value) in map {
                for smaller in candidates(value) {
                    let mut m = map.clone();
                    m.insert(key.clone(), smaller);
                    out.push(J::Object(m));
                }
            }
        }
        J::Array(items) => {
            for i in 0..items.len() {
                let mut smaller = items.clone();
                smaller.remove(i);
                out.push(J::Array(smaller));
            }
            for (i, item) in items.iter().enumerate() {
                for smaller in candidates(item) {
                    let mut a = items.clone();
                    a[i] = smaller;
                    out.push(J::Array(a));
                }
            }
        }
        J::String(s) if s.chars().count() > 1 => {
            let mid = s.char_indices().nth(s.chars().count() / 2).unwrap().0;
            out.push(J::String(s[..mid].to_owned()));
            out.push(J::String(s[mid..].to_owned()));
            if s != "x" {
                out.push(J::String("x".to_owned()));
            }
        }
        _ => {}
    }
    out
}

/// Shrink while the failure survives, so the report is a minimal reproducer.
fn shrink(mut json: serde_json::Value, options: &RenderOptions) -> serde_json::Value {
    let mut progress = true;
    while progress {
        progress = false;
        for candidate in candidates(&json) {
            if failure(&candidate, options).is_some() {
                json = candidate;
                progress = true;
                break;
            }
        }
    }
    json
}

#[test]
fn roundtrip_fuzz() {
    let mut rng = Rng::new(SEED);
    let sets: Vec<(&str, RenderOptions)> = OPTION_SETS
        .iter()
        .map(|(name, src)| (*name, options_for(src)))
        .collect();

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for _ in 0..CASES {
        let json = gen_value(&mut rng, 0);
        for (name, options) in &sets {
            checked += 1;
            if failures.len() >= 8 {
                continue; // keep reporting bounded; the seed reproduces the rest
            }
            if let Some(reason) = failure(&json, options) {
                let minimal = shrink(json.clone(), options);
                let detail = failure(&minimal, options).unwrap_or(reason);
                failures.push(format!(
                    "[{name}] input: {}\n{detail}",
                    serde_json::to_string(&minimal).unwrap()
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {checked} round trips failed (seed {SEED:#x}):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
