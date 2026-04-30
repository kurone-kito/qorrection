//! PTY end-to-end tests for the shipped `q9` binary.
//!
//! The real wrapper path only activates when both stdio streams
//! are TTYs. `rexpect` gives the subprocess that environment, so
//! these tests cover behavior that `assert_cmd` cannot observe.

#[cfg(unix)]
mod unix {
    use rexpect::process::wait::WaitStatus;
    use rexpect::session::spawn_command;
    use std::process::Command;

    const TIMEOUT_MS: u64 = 5_000;

    fn q9() -> Command {
        Command::new(env!("CARGO_BIN_EXE_q9"))
    }

    #[test]
    fn q9_cat_passthrough_echoes_input_and_exits_zero() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.arg("cat");

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line("hello from q9")?;
        session.exp_string("hello from q9")?;
        session.send_control('d')?;
        let _remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => Ok(()),
            other => panic!("expected q9 cat to exit 0, got {other:?}"),
        }
    }

    #[test]
    fn q9_sh_nonzero_exit_is_propagated() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.args(["sh", "-c", "exit 7"]);

        let session = spawn_command(command, Some(TIMEOUT_MS))?;
        match session.process.wait()? {
            WaitStatus::Exited(_, 7) => Ok(()),
            other => panic!("expected q9 sh -c 'exit 7' to exit 7, got {other:?}"),
        }
    }
}

#[cfg(windows)]
mod windows {
    /// Windows ConPTY E2E coverage is tracked separately for
    /// v0.1 because this suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY passthrough smoke is tracked by issue #65"]
    fn q9_cat_passthrough_echoes_input_and_exits_zero() {}

    /// Windows ConPTY E2E coverage is tracked separately for
    /// v0.1 because this suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY passthrough smoke is tracked by issue #65"]
    fn q9_sh_nonzero_exit_is_propagated() {}
}
