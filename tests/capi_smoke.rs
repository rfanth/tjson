//! Runs the C ABI smoke test (tests/capi/run.sh) as part of
//! `cargo test --features capi`, so the real C boundary — clang compiling
//! against include/tjson.h, dynamic linking, AddressSanitizer — is exercised
//! by the normal test command instead of a separately remembered script.
//!
//! This target is gated by `required-features = ["capi"]` in Cargo.toml, so
//! plain `cargo test` neither builds nor runs it. It needs clang and the
//! x86_64-unknown-linux-gnu target installed (see tests/capi/run.sh).

use std::process::Command;

#[test]
fn c_abi_smoke_test_under_asan() {
    let output = Command::new("tests/capi/run.sh")
        .output()
        .expect("failed to launch tests/capi/run.sh");
    assert!(
        output.status.success(),
        "C ABI smoke test failed (exit: {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
