//! PTY end-to-end tests for the shipped `q9` binary.
//!
//! The real wrapper path only activates when both stdio streams
//! are TTYs. `rexpect` gives the subprocess that environment, so
//! these tests cover behavior that `assert_cmd` cannot observe.

mod support;

#[cfg(unix)]
mod unix {
    use super::support;
    use rexpect::process::wait::WaitStatus;
    use rexpect::session::spawn_command;
    use std::process::Command;

    // The standard `:q` sweep scales linearly with the host PTY
    // width that rexpect exposes. Some hosted runners present a
    // much wider terminal than the local 80-col default, which
    // stretches the full animation past ten seconds. Keep enough
    // headroom that CI width differences do not turn the E2E
    // assertion into a timeout race.
    const TIMEOUT_MS: u64 = 30_000;
    const LARGE_WQ_COLS: u16 = 120;
    const PTY_ROWS: u16 = 24;

    fn q9() -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_q9"));
        // Keep PTY output hermetic so byte-level animation
        // assertions do not inherit tracing noise from the outer
        // environment on developer machines or CI runners.
        command.env_remove("QORRECTION_LOG");
        command
    }

    fn q9_with_tty_size(cols: u16, rows: u16) -> Command {
        let mut command = Command::new("sh");
        let script = format!("stty cols {cols} rows {rows} && exec \"$0\" \"$@\"");
        // Resize the rexpect-controlled PTY from inside the shell
        // so the same setup works on Linux and macOS; direct
        // `TIOCSWINSZ` on rexpect's master FD returns ENOTTY on
        // the hosted macOS runners.
        command.arg("-c").arg(script).arg(env!("CARGO_BIN_EXE_q9"));
        command.env_remove("QORRECTION_LOG");
        command
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
    fn q9_cat_passthrough_preserves_q_literal_when_command_is_not_armed(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.arg("cat");

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":q")?;
        session.exp_string(":q")?;
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected q9 cat to exit 0, got {other:?}"),
        }

        let normalized = remaining.replace("\r\n", "\n");
        assert!(
            normalized.contains(":q"),
            "expected cat itself to echo a second :q literal, got {normalized:?}"
        );
        assert!(
            !normalized.contains("[QQ]")
                && !normalized.contains("Fi-Fo")
                && !normalized.contains("QUEUE"),
            "expected passthrough output without animation text, got {normalized:?}"
        );
        Ok(())
    }

    /// Issue #53 E2E coverage: when an allowlisted child is
    /// armed, typing `:q` must animate on the parent PTY without
    /// forwarding that trigger line into the child. The helper's
    /// first observed stdin line should therefore be the later
    /// ordinary payload, proving the child survived the
    /// animation.
    #[test]
    fn q9_armed_helper_intercepts_q_and_keeps_child_alive() -> Result<(), Box<dyn std::error::Error>>
    {
        let helper = support::ArmedHelper::echo_stdin();
        let mut command = q9();
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":q")?;

        // The outer PTY may still locally echo the typed line
        // before q9 switches presentation modes, so child
        // suppression is proven by the later helper echo rather
        // than by asserting byte-for-byte absence here.
        let _before_animation = session.exp_string("\u{1b}[?1049h")?;

        let animation = session.exp_string("\u{1b}[?1049l")?;
        let normalized_animation = animation.replace("\r\n", "\n");
        assert!(
            normalized_animation.contains("\u{1b}[?25l"),
            "expected animation to hide the cursor, got {normalized_animation:?}"
        );
        assert!(
            normalized_animation.contains("\u{1b}[2J"),
            "expected animation to draw at least one frame, got {normalized_animation:?}"
        );
        session.send_line("still-here")?;
        session.exp_string("still-here")?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected armed helper to exit 0 after follow-up input, got {other:?}"),
        }

        let normalized_remaining = remaining.replace("\r\n", "\n");
        assert!(
            normalized_remaining.contains("still-here"),
            "expected helper stdout to echo the follow-up line after animation, got {normalized_remaining:?}"
        );
        assert!(
            !normalized_remaining.contains(":q"),
            "expected swallowed trigger to stay out of helper output, got {normalized_remaining:?}"
        );
        Ok(())
    }

    /// Issue #54 E2E coverage: when an allowlisted child is
    /// armed, typing `:wq` on a 120-column PTY must render the
    /// large scene that carries the spec-locked 418 label while
    /// still suppressing the trigger from child stdin.
    #[test]
    fn q9_armed_helper_wq_shows_418_label() -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::echo_stdin();
        let mut command = q9_with_tty_size(LARGE_WQ_COLS, PTY_ROWS);
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":wq")?;

        let _before_animation = session.exp_string("\u{1b}[?1049h")?;
        let animation = session.exp_string("\u{1b}[?1049l")?;
        let normalized_animation = animation.replace("\r\n", "\n");
        assert!(
            normalized_animation.contains("\u{1b}[2J"),
            "expected animation to draw at least one frame, got {normalized_animation:?}"
        );
        assert!(
            normalized_animation.contains("WRITE QUEUE"),
            "expected the large :wq scene banner, got {normalized_animation:?}"
        );
        assert!(
            normalized_animation.contains("418 I'm an AI agent"),
            "expected the large :wq scene to carry the 418 label, got {normalized_animation:?}"
        );

        session.send_line("still-here")?;
        session.exp_string("still-here")?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected armed helper to exit 0 after follow-up input, got {other:?}"),
        }

        let normalized_remaining = remaining.replace("\r\n", "\n");
        assert!(
            normalized_remaining.contains("still-here"),
            "expected helper stdout to echo the follow-up line after animation, got {normalized_remaining:?}"
        );
        assert!(
            !normalized_remaining.contains(":wq"),
            "expected swallowed trigger to stay out of helper output, got {normalized_remaining:?}"
        );
        Ok(())
    }

    #[test]
    fn q9_cat_typed_ctrl_c_reaches_child_and_exits_130() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.args(["sh", "-c", "printf 'READY\\n'; exec cat"]);

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        assert_eq!(session.read_line()?, "READY");
        session.send_control('c')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 130) => {}
            other => panic!("expected q9 cat to surface SIGINT as exit 130, got {other:?}"),
        }

        let normalized = remaining.replace("\r\n", "\n");
        assert!(
            normalized.contains("child terminated by signal 2"),
            "expected SIGINT diagnostic on PTY output, got {normalized:?}"
        );
        Ok(())
    }

    #[test]
    fn q9_sh_nonzero_exit_is_propagated() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.args(["sh", "-c", "exit 7"]);

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        let _remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 7) => Ok(()),
            other => panic!("expected q9 sh -c 'exit 7' to exit 7, got {other:?}"),
        }
    }

    /// Issue #24 E2E coverage: a PTY child killed by SIGTERM
    /// must propagate as host exit `128 + 15 = 143`. The unit
    /// tests in `src/pty/exit.rs` and `src/error.rs` cover the
    /// type-level mapping with mocks; this exercises the full
    /// chain (real `portable-pty` reporting → `map_exit_status`
    /// → `Error::Signal` → `ExitCode`) for the wrap path.
    #[test]
    fn q9_pty_sigterm_propagates_as_143() -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        // `kill -TERM $$` raises SIGTERM in the shell itself, so
        // `Child::wait` reports termination by signal 15. Using
        // an explicit numeric signal avoids depending on `kill`'s
        // signal-name parsing across distributions.
        command.args(["sh", "-c", "kill -15 $$"]);

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 143) => {}
            other => {
                panic!("expected q9 to surface SIGTERM as exit 143, got {other:?}")
            }
        }

        // Also assert the diagnostic so the test cannot pass on a
        // child that merely exited cleanly with status 143; the
        // PTY merges stdout+stderr so the eprintln from `lib.rs`
        // appears on the master side captured by `exp_eof`.
        assert!(
            remaining.contains("child terminated by signal 15"),
            "expected SIGTERM diagnostic on PTY output, got {remaining:?}"
        );
        Ok(())
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
    fn q9_cat_passthrough_preserves_q_literal_when_command_is_not_armed() {}

    /// Windows ConPTY trigger-animation E2E coverage is tracked
    /// separately for v0.1 because this suite depends on Unix-only
    /// `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY trigger-animation E2E is tracked by issue #65"]
    fn q9_armed_helper_intercepts_q_and_keeps_child_alive() {}

    /// Windows ConPTY trigger-animation E2E for the large `:wq`
    /// 418 scene is tracked separately for v0.1 because this
    /// suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY trigger-animation E2E is tracked by issue #65"]
    fn q9_armed_helper_wq_shows_418_label() {}

    /// Windows ConPTY E2E coverage is tracked separately for
    /// v0.1 because this suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY passthrough smoke is tracked by issue #65"]
    fn q9_cat_typed_ctrl_c_reaches_child_and_exits_130() {}

    /// Windows ConPTY E2E coverage is tracked separately for
    /// v0.1 because this suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY passthrough smoke is tracked by issue #65"]
    fn q9_sh_nonzero_exit_is_propagated() {}
}
