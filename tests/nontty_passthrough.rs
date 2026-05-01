//! Non-TTY passthrough tests for the shipped `q9` binary.

mod support;

use assert_cmd::Command;

fn q9() -> Command {
    let mut cmd = Command::cargo_bin("q9").expect("q9 bin built");
    // Hermetic stderr: any future `tracing` call site that
    // honours `QORRECTION_LOG` from the outer environment would
    // otherwise make `assert_no_escape("stderr", ...)` flaky on
    // contributor machines or CI runners that opt in to debug
    // logging globally. Mirror the convention from
    // tests/tracing_env.rs and explicitly clear the variable.
    cmd.env_remove("QORRECTION_LOG");
    cmd
}

#[test]
fn piped_stdin_reaches_armed_child_without_animation() {
    let helper = support::ArmedHelper::echo_stdin();
    let assert = q9()
        .env("PATH", helper.path())
        .arg(helper.command())
        .write_stdin(":q\n")
        .assert()
        .success();

    let output = assert.get_output();
    // Equality (not `contains`) so the "without animation"
    // contract this PR exists to test would actually fail if a
    // regression appended a plain-text fallback banner or any
    // other non-escape bytes alongside the trigger payload.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        ":q\n",
        "expected child stdout to be exactly the piped trigger, got {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_no_escape("stdout", &output.stdout);
    assert_no_escape("stderr", &output.stderr);
}

#[test]
fn redirected_stdout_from_armed_child_contains_no_ansi_escape() {
    let helper = support::ArmedHelper::plain_stdout();
    let assert = q9()
        .env("PATH", helper.path())
        .arg(helper.command())
        .assert()
        .success();

    let output = assert.get_output();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        "plain-output\n"
    );
    assert_no_escape("stdout", &output.stdout);
    assert_no_escape("stderr", &output.stderr);
}

fn assert_no_escape(label: &str, bytes: &[u8]) {
    assert!(
        !bytes.contains(&0x1b),
        "expected no ANSI escape bytes in {label}, got {:?}",
        String::from_utf8_lossy(bytes)
    );
}
