use std::path::Path;
use tjson::{TjsonConfig, RenderOptions, Value};

/// Where the fixtures actually live, which is not always where they appear to.
///
/// `.cargo/config.toml` sets `TJSON_TESTS_DIR`, so a checkout beside this repo wins
/// over the `tests/fixtures` submodule -- and the submodule is the one `git status`
/// and `ls` point at. Editing the wrong copy produces a file that plainly says one
/// thing and a test that plainly reports another, so every failure below names the
/// directory it read.
fn tests_dir() -> std::path::PathBuf {
    let pathbuf = if let Ok(p) = std::env::var("TJSON_TESTS_DIR") {
        std::path::PathBuf::from(p)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
    };
    let tests_missing_message = "You are trying to test without the tjson-tests subrepository.  Did you follow the instructions in CONTRIBUTING.md first?";
    _ = pathbuf.read_dir().unwrap_or_else(|e| panic!("Cannot read tests directory {pathbuf:?}: {e}\n{tests_missing_message}"))
        .next().unwrap_or_else(|| panic!("Cannot find anything in tests directory {pathbuf:?}.\n{tests_missing_message}"));
    pathbuf
}

/// Collect every `.tjson` fixture under `base`, descending into subdirectories
/// so a category can have its own folder instead of every case living in one
/// flat listing. Returns paths paired with a name relative to `base`, so a
/// failure reports `bare_keys/leading_pipelike` rather than a bare stem that
/// gives no clue where to look.
///
/// Disabling a fixture or a whole category, for cases that record behaviour
/// known to be wrong and that should not fail the suite yet:
///
/// * rename it to `<name>.tjson.disabled`
/// * or prefix the file or directory name with `_`
/// * or put it in a directory named `known-bugs`
///
/// Names beginning with `.` are skipped as well, since editors and archivers
/// leave such files around and they are never fixtures.
fn collect_fixtures(base: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(std::path::PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            if name.starts_with('_') || name.starts_with('.') || name.ends_with(".disabled") {
                continue;
            }
            if path.is_dir() {
                // `expected/` holds the answers, not fixtures.
                if name == "expected" || name == "known-bugs" {
                    continue;
                }
                walk(&path, base, out);
                continue;
            }
            if path.extension().map(|x| x == "tjson").unwrap_or(false) {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .with_extension("")
                    .to_string_lossy()
                    .into_owned();
                out.push((path, rel));
            }
        }
    }
    let mut out = Vec::new();
    walk(base, base, &mut out);
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

#[test]
fn parse_valid() {
    let base = tests_dir().join("parse/valid");
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;

    let entries = collect_fixtures(&base);

    if entries.is_empty() {
        panic!("No .tjson files found in {:?}", base);
    }

    for (tjson_path, stem) in entries {
        total += 1;
        // Expected JSON sits in an `expected/` directory beside the fixture, so
        // a categorised subdirectory carries its own answers.
        let json_path = tjson_path
            .parent()
            .unwrap_or(&base)
            .join("expected")
            .join(format!("{}.json", tjson_path.file_stem().unwrap().to_string_lossy()));

        let tjson_src = match std::fs::read_to_string(&tjson_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read: {}", stem, e));
                continue;
            }
        };

        let parsed: Value = match tjson_src.parse() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: parse error: {}", stem, e));
                continue;
            }
        };

        let expected_json_src = match std::fs::read_to_string(&json_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: missing expected JSON at {:?}: {}", stem, json_path, e));
                continue;
            }
        };

        let expected_json: serde_json::Value = match serde_json::from_str(&expected_json_src) {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: could not parse expected JSON: {}", stem, e));
                continue;
            }
        };

        let actual_json: serde_json::Value = serde_json::from_str(&parsed.to_json()).unwrap();

        if actual_json != expected_json {
            failures.push(format!(
                "{}: mismatch\n  expected: {}\n  actual:   {}",
                stem,
                serde_json::to_string(&expected_json).unwrap(),
                serde_json::to_string(&actual_json).unwrap()
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "\n\nFAILED: {}/{} parse_valid fixture(s) from {}:\n\n{}\n",
            failures.len(),
            total,
            tests_dir().canonicalize().unwrap_or_else(|_| tests_dir()).display(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn parse_invalid() {
    let base = tests_dir().join("parse/invalid");
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;

    let entries = collect_fixtures(&base);

    if entries.is_empty() {
        panic!("No .tjson files found in {:?}", base);
    }

    for (tjson_path, stem) in entries {
        total += 1;

        let tjson_src = match std::fs::read_to_string(&tjson_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read: {}", stem, e));
                continue;
            }
        };

        match tjson_src.parse::<Value>() {
            Ok(v) => {
                failures.push(format!(
                    "{}: expected parse error but got: {:?}",
                    stem, v
                ));
            }
            Err(_) => {
                // expected
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "\n\nFAILED: {}/{} parse_invalid fixture(s) from {}:\n\n{}\n",
            failures.len(),
            total,
            tests_dir().canonicalize().unwrap_or_else(|_| tests_dir()).display(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn roundtrip() {
    let base = tests_dir().join("roundtrip");
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;

    let entries: Vec<_> = std::fs::read_dir(&base)
        .expect("cannot read roundtrip dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            // skip known-bugs subdirectory
            if p.is_dir() {
                return false;
            }
            p.extension().map(|x| x == "tjson").unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        panic!("No .tjson files found in {:?}", base);
    }

    for entry in entries {
        total += 1;
        let tjson_path = entry.path();
        let stem = tjson_path.file_stem().unwrap().to_string_lossy().into_owned();

        let tjson_src = match std::fs::read_to_string(&tjson_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read: {}", stem, e));
                continue;
            }
        };

        // parse
        let parsed: Value = match tjson_src.parse() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: parse error: {}", stem, e));
                continue;
            }
        };

        let original_json: serde_json::Value = serde_json::from_str(&parsed.to_json()).unwrap();

        // render
        let rendered = parsed.to_tjson_with(RenderOptions::default());

        // reparse
        let reparsed: Value = match rendered.parse() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: reparse error: {}", stem, e));
                continue;
            }
        };

        let reparsed_json: serde_json::Value = serde_json::from_str(&reparsed.to_json()).unwrap();

        if original_json != reparsed_json {
            failures.push(format!(
                "{}: roundtrip mismatch\n  original: {}\n  after roundtrip: {}",
                stem,
                serde_json::to_string(&original_json).unwrap(),
                serde_json::to_string(&reparsed_json).unwrap()
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "\n\nFAILED: {}/{} roundtrip fixture(s) from {}:\n\n{}\n",
            failures.len(),
            total,
            tests_dir().canonicalize().unwrap_or_else(|_| tests_dir()).display(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn render() {
    let render_base = tests_dir().join("render");
    let mut failures: Vec<String> = Vec::new();
    let mut total = 0usize;

    let subdirs: Vec<_> = std::fs::read_dir(&render_base)
        .expect("cannot read render dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();

    if subdirs.is_empty() {
        panic!("No subdirs found in {:?}", render_base);
    }

    for subdir_entry in subdirs {
        let subdir = subdir_entry.path();
        let subdir_name = subdir.file_name().unwrap().to_string_lossy().into_owned();

        let config_path = subdir.join("config.json");
        let config_src = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read config.json: {}", subdir_name, e));
                continue;
            }
        };

        // TjsonConfig ignores unknown fields by design (the JS options-bag contract),
        // which for fixtures is a trap: a typo'd key would be silently dropped and the
        // fixture would quietly test default options. serde_ignored turns that into a
        // loud failure naming the bad key.
        let mut json_de = serde_json::Deserializer::from_str(&config_src);
        let mut unknown_keys: Vec<String> = Vec::new();
        let config: TjsonConfig =
            match serde_ignored::deserialize(&mut json_de, |path| unknown_keys.push(path.to_string())) {
                Ok(o) => o,
                Err(e) => {
                    failures.push(format!("{}: could not parse config.json: {}", subdir_name, e));
                    continue;
                }
            };
        if !unknown_keys.is_empty() {
            failures.push(format!(
                "{}: config.json has unknown option keys {:?} — typo'd keys are silently \
                 ignored by TjsonConfig, so this fixture would test default options",
                subdir_name, unknown_keys
            ));
            continue;
        }
        let options: RenderOptions = config.into();

        let json_entries: Vec<_> = std::fs::read_dir(&subdir)
            .expect("cannot read subdir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path();
                p.extension().map(|x| x == "json").unwrap_or(false)
                    && p.file_name().map(|n| n != "config.json").unwrap_or(false)
            })
            .collect();

        for json_entry in json_entries {
            total += 1;
            let json_path = json_entry.path();
            let stem = json_path.file_stem().unwrap().to_string_lossy().into_owned();
            let tjson_path = subdir.join(format!("{}.tjson", stem));
            let test_name = format!("{}/{}", subdir_name, stem);

            let json_src = match std::fs::read_to_string(&json_path) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{}: could not read JSON input: {}", test_name, e));
                    continue;
                }
            };

            let json_val: serde_json::Value = match serde_json::from_str(&json_src) {
                Ok(v) => v,
                Err(e) => {
                    failures.push(format!("{}: could not parse JSON input: {}", test_name, e));
                    continue;
                }
            };

            let tjson_val = Value::from(json_val);

            let rendered = tjson_val.to_tjson_with(options.clone());

            let expected_raw = match std::fs::read_to_string(&tjson_path) {
                Ok(s) => s,
                Err(e) => {
                    panic!(
                        "{}: missing expected .tjson file at {:?}: {}",
                        test_name, tjson_path, e
                    );
                }
            };

            // Strip a single trailing line ending (CRLF or LF) from the expected file,
            // so fixtures with `eol: "crlf"` can be authored with a natural trailing newline.
            let expected = expected_raw
                .strip_suffix("\r\n")
                .or_else(|| expected_raw.strip_suffix('\n'))
                .unwrap_or(&expected_raw);

            if rendered != expected {
                failures.push(format!(
                    "{}: render mismatch\n  expected: {:?}\n  actual:   {:?}",
                    test_name, expected, rendered
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "\n\nFAILED: {}/{} render fixture(s) from {}:\n\n{}\n",
            failures.len(),
            total,
            tests_dir().canonicalize().unwrap_or_else(|_| tests_dir()).display(),
            failures.join("\n\n")
        );
    }
}
