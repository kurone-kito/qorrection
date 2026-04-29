//! Subprocess tests for `QORRECTION_LOG` gating.
//!
//! These run the shipped binary as a subprocess and assert that
//! the wrapper stays silent on stderr in two "diagnostics off"
//! configurations:
//!
//! 1. `QORRECTION_LOG` is unset (the default), and
//! 2. `QORRECTION_LOG` is set to a syntactically invalid filter,
//!    which must degrade to silence rather than spamming a
//!    parser error into the wrapped session.
//!
//! A positive case (`QORRECTION_LOG=info` actually emitting a
//! `tracing` line) lands together with the first real diagnostic
//! call site in Phase 1; without it there is nothing for the
//! subscriber to log, so asserting on incidental output here
//! would be flaky.
//!
//! Subprocess isolation is required because
//! `tracing::subscriber::set_global_default` mutates
//! process-global state and cannot be reset between in-process
//! tests.

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
