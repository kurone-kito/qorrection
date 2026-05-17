//! Windows-only smoke tests for the shipped `q9` binary.
//!
//! These tests verify that the basic wrap path works on Windows
//! in non-TTY mode (the passthrough path). The test harness
//! captures stdout so stdin/stdout are not TTYs; `q9` takes
//! the `non_tty_passthrough` route and the child runs with
//! inherited stdio from the `assert_cmd` harness.
//!
//! PTY E2E tests (which require a real pseudo-terminal) are
//! tracked by future work. See `docs/testing.md` for the
//! Windows test policy.

#[cfg(windows)]
mod windows {
    use assert_cmd::Command;

    fn q9() -> Command {
        let mut cmd = Command::cargo_bin("q9").expect("q9 bin built");
        cmd.env_remove("QORRECTION_LOG");
        cmd
    }

    /// Verify `q9 cmd /c echo hi` exits successfully on Windows.
    ///
    /// This exercises the non-TTY passthrough path: the test
    /// harness is not a TTY, so `q9` transparently delegates
    /// to the child without entering the PTY pump.
    #[test]
    fn q9_cmd_echo_hi_exits_success() {
        q9().args(["cmd", "/c", "echo", "hi"]).assert().success();
    }

    /// Verify that a child with a non-zero exit code propagates
    /// its exit status through the passthrough path.
    #[test]
    fn q9_cmd_exit_nonzero_propagates() {
        let assert = q9().args(["cmd", "/c", "exit 7"]).assert().failure();
        let code = assert.get_output().status.code().expect("has exit code");
        assert_eq!(code, 7, "non-zero exit code must be forwarded verbatim");
    }
}
