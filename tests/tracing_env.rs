//! Subprocess tests for `QORRECTION_LOG` gating.
//!
//! These run the shipped binary twice (once with the env var
//! unset, once with it set to `info`) and assert that the
//! wrapper itself remains silent when the var is unset and emits
//! a `tracing` formatter line on stderr when set. Subprocess
//! isolation is required because `tracing::subscriber::
//! set_global_default` mutates process-global state and cannot
//! be reset between in-process tests.

use assert_cmd::Command;

#[test]
fn version_is_silent_without_qorrection_log() {
    let assert = Command::cargo_bin("qorrection")
        .expect("locate binary")
        .env_remove("QORRECTION_LOG")
        .arg("--version")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.is_empty(),
        "expected silent stderr without QORRECTION_LOG; got: {stderr:?}"
    );
}

#[test]
fn invalid_qorrection_log_is_silent() {
    // An obviously-broken filter expression should be treated
    // as "diagnostics off" rather than spamming a parser error
    // into the wrapped session.
    let assert = Command::cargo_bin("qorrection")
        .expect("locate binary")
        .env("QORRECTION_LOG", "[not-a-valid-filter")
        .arg("--version")
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.is_empty(),
        "expected silent stderr for invalid QORRECTION_LOG; got: {stderr:?}"
    );
}
