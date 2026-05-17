//! PTY end-to-end tests for the shipped `q9` binary.
//!
//! The real wrapper path only activates when both stdio streams
//! are TTYs. `rexpect` gives the subprocess that environment, so
//! these tests cover behavior that `assert_cmd` cannot observe.
//!
//! ## Windows policy (v0.1)
//!
//! All tests in this file are gated with `#[cfg(unix)]` rather
//! than `#[cfg_attr(not(unix), ignore)]`. The distinction is
//! intentional: `rexpect` is a Unix-only dev-dependency (see
//! `[target.'cfg(unix)'.dev-dependencies]` in `Cargo.toml`), so
//! the rexpect-based test bodies cannot compile on Windows at all.
//! Using `#[cfg(unix)]` correctly excludes them from the Windows
//! build rather than compiling them into ignored stubs.
//!
//! Tracking: <https://github.com/kurone-kito/qorrection/issues/64>

mod support;

#[cfg(unix)]
mod unix {
    use super::support;
    use rexpect::process::signal::Signal;
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
    const BANG_CARS: usize = 9;
    const PARADE_MIN_VISIBLE_LABELS: usize = 3;
    const LONG_ANIMATION_COLS: u16 = 120;
    const SIGWINCH_HOST_COLS: u16 = 120;
    const SIGWINCH_CHILD_COLS: u16 = 80;
    const SIGWINCH_CHILD_ROWS: u16 = 20;
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

    fn bang_cols() -> u16 {
        u16::try_from(qorrection::anim::car::max_width(qorrection::anim::car::STD) * BANG_CARS)
            .expect("nine-car convoy width must fit in u16")
    }

    fn max_frame_occurrences(animation: &str, needle: &str) -> usize {
        // `draw_frame` clears the screen before every frame, so
        // each `\u{1b}[2J` split chunk corresponds to one convoy
        // snapshot plus any surrounding cursor/home control bytes.
        animation
            .split("\u{1b}[2J")
            .map(|frame| frame.matches(needle).count())
            .max()
            .unwrap_or(0)
    }

    fn queue_run_regex(labels: usize) -> String {
        format!("QUEUE(?:[^\\n]*QUEUE){{{}}}", labels.saturating_sub(1))
    }

    fn terminal_flag_is_enabled(mode_line: &str, flag: &str) -> bool {
        let disabled = format!("-{flag}");
        let mut state = None;

        for token in mode_line
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
            .filter(|token| !token.is_empty())
        {
            if token == flag {
                state = Some(true);
            } else if token == disabled {
                state = Some(false);
            }
        }

        state.unwrap_or_else(|| panic!("expected {flag:?} in stty output: {mode_line:?}"))
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

    /// Issue #55 E2E coverage: when an allowlisted child is
    /// armed, typing `:q!` on a wide PTY must render a convoy
    /// frame with all nine `QUEUE` labels while still keeping
    /// the trigger out of child stdin.
    #[test]
    fn q9_armed_helper_q_bang_shows_nine_car_parade() -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::echo_stdin();
        // Use the full convoy width so every hosted runner gets at
        // least one frame where all nine labels are simultaneously
        // visible instead of clipped at the viewport edge.
        let mut command = q9_with_tty_size(bang_cols(), PTY_ROWS);
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":q!")?;

        let _before_animation = session.exp_string("\u{1b}[?1049h")?;
        // The full-width nine-car parade takes longer than a single
        // rexpect polling window on hosted macOS, so break the read at
        // the first fully visible convoy row and then wait only for the
        // remaining tail back to the primary screen.
        let (before_full_convoy, full_convoy_row) =
            session.exp_regex(&queue_run_regex(BANG_CARS))?;
        let after_full_convoy = session.exp_string("\u{1b}[?1049l")?;
        let animation = format!("{before_full_convoy}{full_convoy_row}{after_full_convoy}");
        let normalized_animation = animation.replace("\r\n", "\n");
        assert!(
            normalized_animation.contains("\u{1b}[2J"),
            "expected animation to draw at least one frame, got {normalized_animation:?}"
        );
        assert_eq!(
            full_convoy_row.matches("QUEUE").count(),
            BANG_CARS,
            "expected a fully visible nine-car convoy row, got {full_convoy_row:?}"
        );
        let max_queue_labels = max_frame_occurrences(&normalized_animation, "QUEUE");
        // Pure scene tests already pin the exact nine-label
        // geometry; this PTY E2E only needs enough repeated
        // labels to prove q9 fired the `:q!` convoy end-to-end
        // on a real host terminal without forwarding the trigger.
        assert!(
            max_queue_labels >= PARADE_MIN_VISIBLE_LABELS,
            "expected q9 to render a multi-car :q! convoy; best frame showed {max_queue_labels} labels in {normalized_animation:?}"
        );
        assert!(
            !normalized_animation.contains("418 I'm an AI agent"),
            "expected the :q! parade to avoid the :wq 418 banner, got {normalized_animation:?}"
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
            !normalized_remaining.contains(":q!"),
            "expected swallowed trigger to stay out of helper output, got {normalized_remaining:?}"
        );
        Ok(())
    }

    /// Issue #57 E2E coverage: an armed child may exit while the
    /// parent is still animating `:q`, and q9 must still leave
    /// the alt screen, avoid hanging, and propagate the child's
    /// eventual non-zero exit status.
    #[test]
    fn q9_armed_child_exit_during_animation_exits_nonzero_without_hanging(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::ready_then_exit_seven();
        let release_dir = tempfile::tempdir()?;
        let release_file = release_dir.path().join("release");
        let mut command = q9_with_tty_size(LONG_ANIMATION_COLS, PTY_ROWS);
        command
            .env("PATH", helper.path())
            .env(
                support::READY_THEN_EXIT_RELEASE_FILE_ENV,
                release_file.as_os_str(),
            )
            .arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        assert_eq!(session.read_line()?, "READY");
        session.send_line(":q")?;

        let _before_animation = session.exp_string("\u{1b}[?1049h")?;
        // Release the helper only after q9 has entered the alt
        // screen so the child exit deterministically lands during
        // the live animation instead of racing the test driver.
        std::fs::write(&release_file, b"release")?;
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

        let _remaining = session.exp_eof()?;
        match session.process.wait()? {
            WaitStatus::Exited(_, 7) => Ok(()),
            other => {
                panic!("expected armed helper exit 7 to propagate after animation, got {other:?}")
            }
        }
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

    #[test]
    fn q9_wrapper_sigterm_gracefully_terminates_child_and_exits_143(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9();
        command.args(["sh", "-c", "printf 'READY\\n'; exec sleep 30"]);

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        let _ready = session.exp_string("READY")?;
        session.process.signal(Signal::SIGTERM)?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 143) => {}
            other => {
                panic!("expected q9 to exit 143 after wrapper SIGTERM, got {other:?}")
            }
        }

        assert!(
            remaining.contains("child terminated by signal 15"),
            "expected wrapper SIGTERM path to surface SIGTERM diagnostic, got {remaining:?}"
        );
        Ok(())
    }

    /// Issue #62 E2E coverage: after the wrapper receives
    /// SIGTERM and shuts down, the host PTY must regain the same
    /// canonical-mode bit it had before `q9` entered raw mode.
    #[test]
    fn q9_wrapper_sigterm_restores_canonical_mode() -> Result<(), Box<dyn std::error::Error>> {
        let trigger_dir = tempfile::tempdir()?;
        let trigger_path = trigger_dir.path().join("sigterm-trigger");

        let mut command = Command::new("sh");
        let script = format!(
            "stty cols {LONG_ANIMATION_COLS} rows {PTY_ROWS}\n\
             before=$(stty -a | tr '\\n' ' ')\n\
             printf 'MODE_BEFORE:%s\\n' \"$before\"\n\
             \"$1\" sh -c 'printf READY\\n; exec sleep 30' &\n\
             child_pid=$!\n\
             while [ ! -f \"$2\" ]; do\n\
               sleep 0.05\n\
             done\n\
             kill -TERM \"$child_pid\"\n\
             wait \"$child_pid\"\n\
             rc=$?\n\
             after=$(stty -a | tr '\\n' ' ')\n\
             printf 'Q9_EXIT:%s\\n' \"$rc\"\n\
             printf 'MODE_AFTER:%s\\n' \"$after\"\n"
        );
        command
            .arg("-c")
            .arg(script)
            .arg("driver")
            .arg(env!("CARGO_BIN_EXE_q9"))
            .arg(&trigger_path);
        command.env_remove("QORRECTION_LOG");

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        let (_, before_line) = session.exp_regex("MODE_BEFORE:[^\\r\\n]+")?;
        session.exp_string("READY")?;

        std::fs::write(&trigger_path, b"go")?;

        let (_, exit_line) = session.exp_regex("Q9_EXIT:[0-9]+")?;
        let (_, after_line) = session.exp_regex("MODE_AFTER:[^\\r\\n]+")?;
        let _remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected wrapper shell to exit 0 after q9 shutdown, got {other:?}"),
        }

        assert_eq!(
            exit_line, "Q9_EXIT:143",
            "expected q9 to exit 143, got {exit_line:?}"
        );
        let before_mode = before_line.trim_start_matches("MODE_BEFORE:");
        let after_mode = after_line.trim_start_matches("MODE_AFTER:");
        assert_eq!(
            terminal_flag_is_enabled(before_mode, "icanon"),
            terminal_flag_is_enabled(after_mode, "icanon"),
            "expected host canonical mode to be restored after SIGTERM: before={before_mode:?} after={after_mode:?}"
        );
        Ok(())
    }

    /// Issue #61 E2E coverage: when the wrapper receives
    /// SIGWINCH, it must resize the child PTY back to the outer
    /// host width and forward the signal so the child trap can
    /// observe the restored terminal size.
    #[test]
    fn q9_wrapper_sigwinch_resizes_child_and_forwards_winch(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut command = q9_with_tty_size(SIGWINCH_HOST_COLS, PTY_ROWS);
        let script = format!(
            "stty cols {SIGWINCH_CHILD_COLS} rows {SIGWINCH_CHILD_ROWS}\n\
             trap 'printf \"WINCH:%s:%s\\\\n\" \"$(tput cols)\" \"$(tput lines)\"; exit 0' WINCH\n\
             printf 'READY:%s:%s\\\\n' \"$(tput cols)\" \"$(tput lines)\"\n\
             while IFS= read -r line; do\n\
               case \"$line\" in\n\
                 print) printf 'NOW:%s:%s\\\\n' \"$(tput cols)\" \"$(tput lines)\" ;;\n\
               esac\n\
             done\n"
        );
        command.env("TERM", "xterm").arg("sh").arg("-c").arg(script);

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        let ready_marker = format!("READY:{SIGWINCH_CHILD_COLS}:{SIGWINCH_CHILD_ROWS}");
        session.exp_string(&ready_marker)?;

        session.send_line("print")?;
        let before_signal = format!("NOW:{SIGWINCH_CHILD_COLS}:{SIGWINCH_CHILD_ROWS}");
        session.exp_string(&before_signal)?;

        session.process.signal(Signal::SIGWINCH)?;
        let forwarded = format!("WINCH:{SIGWINCH_HOST_COLS}:{PTY_ROWS}");
        session.exp_string(&forwarded)?;

        let _remaining = session.exp_eof()?;
        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => Ok(()),
            other => {
                panic!("expected q9 to exit 0 after forwarded SIGWINCH coverage, got {other:?}")
            }
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

    /// Windows ConPTY trigger-animation E2E for the `:q!`
    /// parade is tracked separately for v0.1 because this suite
    /// depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY trigger-animation E2E is tracked by issue #65"]
    fn q9_armed_helper_q_bang_shows_nine_car_parade() {}

    /// Windows ConPTY trigger-animation E2E for a child exiting
    /// mid-animation is tracked separately for v0.1 because
    /// this suite depends on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY trigger-animation E2E is tracked by issue #65"]
    fn q9_armed_child_exit_during_animation_exits_nonzero_without_hanging() {}

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
