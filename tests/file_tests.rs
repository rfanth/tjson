use std::path::Path;
use tjson::{TjsonConfig, TjsonOptions, TjsonValue};

fn tests_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("TJSON_TESTS_DIR") {
        return std::path::PathBuf::from(p);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
}

#[test]
fn parse_valid() {
    let base = tests_dir().join("parse/valid");
    let expected_dir = base.join("expected");
    let mut failures: Vec<String> = Vec::new();

    let entries: Vec<_> = std::fs::read_dir(&base)
        .expect("cannot read parse/valid dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "tjson")
                .unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        panic!("No .tjson files found in {:?}", base);
    }

    for entry in entries {
        let tjson_path = entry.path();
        let stem = tjson_path.file_stem().unwrap().to_string_lossy().into_owned();
        let json_path = expected_dir.join(format!("{}.json", stem));

        let tjson_src = match std::fs::read_to_string(&tjson_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read: {}", stem, e));
                continue;
            }
        };

        let parsed: TjsonValue = match tjson_src.parse() {
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

        let actual_json = match parsed.to_json() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: to_json error: {}", stem, e));
                continue;
            }
        };

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
        panic!("parse_valid failures:\n{}", failures.join("\n"));
    }
}

#[test]
fn parse_invalid() {
    let base = tests_dir().join("parse/invalid");
    let mut failures: Vec<String> = Vec::new();

    let entries: Vec<_> = std::fs::read_dir(&base)
        .expect("cannot read parse/invalid dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "tjson")
                .unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        panic!("No .tjson files found in {:?}", base);
    }

    for entry in entries {
        let tjson_path = entry.path();
        let stem = tjson_path.file_stem().unwrap().to_string_lossy().into_owned();

        let tjson_src = match std::fs::read_to_string(&tjson_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: could not read: {}", stem, e));
                continue;
            }
        };

        match tjson_src.parse::<TjsonValue>() {
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
        panic!("parse_invalid failures:\n{}", failures.join("\n"));
    }
}

#[test]
fn roundtrip() {
    let base = tests_dir().join("roundtrip");
    let mut failures: Vec<String> = Vec::new();

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
        let parsed: TjsonValue = match tjson_src.parse() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: parse error: {}", stem, e));
                continue;
            }
        };

        let original_json = match parsed.to_json() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: to_json error: {}", stem, e));
                continue;
            }
        };

        // render
        let rendered = match parsed.to_tjson_with(TjsonOptions::default()) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{}: render error: {}", stem, e));
                continue;
            }
        };

        // reparse
        let reparsed: TjsonValue = match rendered.parse() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: reparse error: {}", stem, e));
                continue;
            }
        };

        let reparsed_json = match reparsed.to_json() {
            Ok(v) => v,
            Err(e) => {
                failures.push(format!("{}: reparsed to_json error: {}", stem, e));
                continue;
            }
        };

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
        panic!("roundtrip failures:\n{}", failures.join("\n"));
    }
}

#[test]
fn render() {
    let render_base = tests_dir().join("render");
    let mut failures: Vec<String> = Vec::new();

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

        let config: TjsonConfig = match serde_json::from_str(&config_src) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{}: could not parse config.json: {}", subdir_name, e));
                continue;
            }
        };
        let options: TjsonOptions = config.into();

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

            let tjson_val = TjsonValue::from(json_val);

            let rendered = match tjson_val.to_tjson_with(options.clone()) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("{}: render error: {}", test_name, e));
                    continue;
                }
            };

            let expected_raw = match std::fs::read_to_string(&tjson_path) {
                Ok(s) => s,
                Err(e) => {
                    panic!(
                        "{}: missing expected .tjson file at {:?}: {}",
                        test_name, tjson_path, e
                    );
                }
            };

            // Strip single trailing newline from expected file
            let expected = expected_raw.strip_suffix('\n').unwrap_or(&expected_raw);

            if rendered != expected {
                failures.push(format!(
                    "{}: render mismatch\n  expected: {:?}\n  actual:   {:?}",
                    test_name, expected, rendered
                ));
            }
        }
    }

    if !failures.is_empty() {
        panic!("render failures:\n{}", failures.join("\n"));
    }
}
