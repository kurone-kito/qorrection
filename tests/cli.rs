//! Phase A integration tests.
//!
//! These exercise the CLI dispatcher through the compiled binary
//! boundary. Phase A has no child runner yet, so we cover only:
//! `--version`, `-V`, `--help`, `-h`, no-args, and unknown-flag
//! rejection. The "wrap-and-forward verbatim" regression
//! (`q9 some-cmd --help`) lives in Phase E once a real child
//! exists to receive the bytes.

use assert_cmd::Command;
use predicates::prelude::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn qorrection() -> Command {
    Command::cargo_bin("qorrection").expect("qorrection bin built")
}

fn q9() -> Command {
    Command::cargo_bin("q9").expect("q9 bin built")
}

#[test]
fn version_long_flag_prints_canonical_line() {
    qorrection()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("qorrection {VERSION}\n"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn version_short_flag_prints_canonical_line() {
    qorrection()
        .arg("-V")
        .assert()
        .success()
        .stdout(format!("qorrection {VERSION}\n"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn q9_alias_prints_same_canonical_program_name() {
    // Spec rule: the printed program name is always `qorrection`,
    // never `q9`, so bug reports and packaging metadata stay
    // greppable on the canonical name.
    q9().arg("--version")
        .assert()
        .success()
        .stdout(format!("qorrection {VERSION}\n"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn no_args_shows_usage_placeholder_on_stdout_exit_zero() {
    // POSIX convention: bare invocation is discovery, not an
    // error. The real usage screen lands in Phase C; for now we
    // assert the discovery semantics (stdout + exit 0) so Phase
    // C cannot accidentally regress them.
    qorrection()
        .assert()
        .success()
        .stdout(predicate::str::contains("USAGE:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn help_long_flag_routes_to_usage() {
    qorrection()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("USAGE:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn help_short_flag_routes_to_usage() {
    qorrection()
        .arg("-h")
        .assert()
        .success()
        .stdout(predicate::str::contains("USAGE:"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn unknown_long_flag_exits_two_with_diagnostic() {
    qorrection()
        .arg("--bogus")
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown option"))
        .stderr(predicate::str::contains("--bogus"));
}

#[test]
fn double_dash_is_rejected_in_v0_1() {
    // `--` is not a separator in v0.1 — see cli::parse rationale.
    qorrection()
        .arg("--")
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown option"));
}
