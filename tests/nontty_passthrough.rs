//! Non-TTY passthrough tests for the shipped `q9` binary.

mod support;

use assert_cmd::Command;

fn q9() -> Command {
    Command::cargo_bin("q9").expect("q9 bin built")
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
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(":q"),
        "expected child stdout to contain piped input, got {:?}",
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
