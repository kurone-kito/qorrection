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

    /// Issue #53 + #186 / #187 E2E coverage: when an allowlisted
    /// child is armed, typing `:q` must animate on the parent PTY
    /// **and** forward the bytes to the child so the child can
    /// echo them back. Byte suppression was removed by #187 — see
    /// the inline comment for the contract.
    #[test]
    fn q9_armed_helper_q_fires_animation_with_byte_faithful_echo(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Roadmap #186 / fix #187 dropped the input-side holdback so
        // wrapped AI CLIs see byte-faithful echo. This test no
        // longer asserts byte suppression — instead, it asserts the
        // animation fires *and* the helper child receives + echoes
        // the trigger bytes back unchanged.
        let helper = support::ArmedHelper::echo_stdin();
        let mut command = q9();
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":q")?;

        let before_animation = session.exp_string("\u{1b}[?1049h")?;
        let animation = session.exp_string("\u{1b}[?1049l")?;
        // The looping `echo_stdin` helper stays blocked on the
        // next `read` after echoing the trigger line; releasing
        // it with Ctrl-D lets the animation tear down cleanly
        // without the wait/drain supervisor undercounting the
        // in-flight render frames (see the helper docstring for
        // the post-#187 race rationale).
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected armed helper to exit 0 after trigger, got {other:?}"),
        }

        let normalized_animation = animation.replace("\r\n", "\n");
        assert!(
            normalized_animation.contains("\u{1b}[?25l"),
            "expected animation to hide the cursor, got {normalized_animation:?}"
        );
        assert!(
            normalized_animation.contains("\u{1b}[2J"),
            "expected animation to draw at least one frame, got {normalized_animation:?}"
        );

        // Byte-faithful echo: the helper echoes each line of stdin.
        // Under the post-#187 observe-only contract, the `:q` bytes
        // reach the child and the child's echo appears somewhere
        // in the captured stream (the exact location vs. the
        // animation is timing-dependent across threads).
        let full = format!("{before_animation}{animation}{remaining}");
        let normalized_full = full.replace("\r\n", "\n");
        assert!(
            normalized_full.contains(":q"),
            "expected helper to echo the typed `:q` under the byte-faithful contract, got {normalized_full:?}"
        );
        Ok(())
    }

    /// #186 / #187 regression: a non-trigger line that starts with
    /// a former holdback prefix (`:`) must reach the armed child
    /// verbatim and be echoed back without delay or truncation.
    /// This pins the byte-faithful echo contract that fixed the
    /// "only emojis come through" symptom on `q9 copilot`.
    #[test]
    fn q9_armed_helper_passes_colon_prefixed_non_trigger_through_verbatim(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::echo_stdin();
        let mut command = q9();
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        // `:foo` shares the `:` prefix with the trigger literals
        // but immediately disqualifies on `f`. The pre-#187
        // holdback parser would still delay the `:` byte until
        // disqualification; the new contract forwards every byte
        // immediately, so the helper sees `:foo` in one go.
        session.send_line(":foo")?;
        let before_foo = session.exp_string(":foo")?;
        // Helper loops on `read`; release it with Ctrl-D so the
        // test can observe EOF.
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => {
                panic!("expected armed helper to exit 0 after non-trigger line, got {other:?}")
            }
        }

        // Check the captured prefix too — `exp_string` consumes
        // and returns everything before the match, so a stray
        // animation overlay emitted before `:foo` appears must
        // not be silently dropped from this guard.
        let full = format!("{before_foo}{remaining}");
        let normalized = full.replace("\r\n", "\n");
        assert!(
            !normalized.contains("\u{1b}[?1049h"),
            "expected no animation overlay for the non-trigger line, got {normalized:?}"
        );
        Ok(())
    }

    /// Issue #186 / #188 E2E coverage: when the wrapped child is
    /// already drawing a TUI on the primary screen (here
    /// emulated by a helper that emits `\x1b[2J\x1b[H` on
    /// startup), typing a trigger literal must **not** acquire
    /// the alt-screen overlay. The renderer falls back to a
    /// single-line gag instead so the child's screen model
    /// stays intact. Reproduces the Codex CLI corruption
    /// reported in #186 once the heuristic in
    /// `crate::trigger::tui_activity` flips
    /// `InputPump::is_child_owning_screen` to `true`.
    #[test]
    fn q9_armed_helper_tui_active_child_uses_fallback_instead_of_alt_screen(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::tui_clear_then_echo_stdin();
        let mut command = q9();
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        // Wait for the helper's TUI clear sequence to traverse the
        // PTY so `OutputArbiter` has updated the shared pump's
        // `TuiActivityTracker` before our trigger fires.
        session.exp_string("\u{1b}[2J")?;
        session.send_line(":q!")?;
        // Look for the `:q!` fallback gag's signature substring
        // — see `crate::anim::fallback::fallback`.
        let before_gag = session.exp_string("[QQ]x9")?;
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => {
                panic!("expected armed helper to exit 0 after fallback path, got {other:?}")
            }
        }

        let normalized = format!("{before_gag}{remaining}").replace("\r\n", "\n");
        assert!(
            !normalized.contains("\u{1b}[?1049h"),
            "expected no alt-screen overlay when child is drawing its own TUI, got {normalized:?}"
        );
        Ok(())
    }

    /// Issue #54 + #186 / #187 E2E coverage: when an allowlisted
    /// child is armed, typing `:wq` on a 120-column PTY must
    /// render the large scene that carries the spec-locked 418
    /// label **and** byte-faithfully echo the typed trigger back
    /// through the child (post-#187 observe-only contract).
    #[test]
    fn q9_armed_helper_wq_shows_418_label() -> Result<(), Box<dyn std::error::Error>> {
        let helper = support::ArmedHelper::echo_stdin();
        let mut command = q9_with_tty_size(LARGE_WQ_COLS, PTY_ROWS);
        command.env("PATH", helper.path()).arg(helper.command());

        let mut session = spawn_command(command, Some(TIMEOUT_MS))?;
        session.send_line(":wq")?;

        let before_animation = session.exp_string("\u{1b}[?1049h")?;
        let animation = session.exp_string("\u{1b}[?1049l")?;
        // Release the looping helper so the test can observe EOF
        // without keeping the child alive past the animation.
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected armed helper to exit 0 after trigger, got {other:?}"),
        }

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

        let full = format!("{before_animation}{animation}{remaining}");
        let normalized_full = full.replace("\r\n", "\n");
        assert!(
            normalized_full.contains(":wq"),
            "expected helper to echo the typed `:wq` under the byte-faithful contract, got {normalized_full:?}"
        );
        Ok(())
    }

    /// Issue #55 + #186 / #187 E2E coverage: when an allowlisted
    /// child is armed, typing `:q!` on a wide PTY must render a
    /// convoy frame with all nine `QUEUE` labels **and**
    /// byte-faithfully echo the typed trigger back through the
    /// child (post-#187 observe-only contract).
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

        let before_animation = session.exp_string("\u{1b}[?1049h")?;
        // The full-width nine-car parade takes longer than a single
        // rexpect polling window on hosted macOS, so break the read at
        // the first fully visible convoy row and then wait only for the
        // remaining tail back to the primary screen.
        let (before_full_convoy, full_convoy_row) =
            session.exp_regex(&queue_run_regex(BANG_CARS))?;
        let after_full_convoy = session.exp_string("\u{1b}[?1049l")?;
        // Release the looping helper so the test can observe EOF
        // without keeping the child alive past the animation.
        session.send_control('d')?;
        let remaining = session.exp_eof()?;

        match session.process.wait()? {
            WaitStatus::Exited(_, 0) => {}
            other => panic!("expected armed helper to exit 0 after trigger, got {other:?}"),
        }

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
        assert!(
            max_queue_labels >= PARADE_MIN_VISIBLE_LABELS,
            "expected q9 to render a multi-car :q! convoy; best frame showed {max_queue_labels} labels in {normalized_animation:?}"
        );
        assert!(
            !normalized_animation.contains("418 I'm an AI agent"),
            "expected the :q! parade to avoid the :wq 418 banner, got {normalized_animation:?}"
        );

        let full = format!("{before_animation}{animation}{remaining}");
        let normalized_full = full.replace("\r\n", "\n");
        assert!(
            normalized_full.contains(":q!"),
            "expected helper to echo the typed `:q!` under the byte-faithful contract, got {normalized_full:?}"
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
    fn q9_armed_helper_q_fires_animation_with_byte_faithful_echo() {}

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

    /// Windows ConPTY E2E for the TUI-active-child fallback path
    /// is tracked separately for v0.1 because this suite depends
    /// on Unix-only `rexpect`.
    /// Tracking issue: <https://github.com/kurone-kito/qorrection/issues/65>.
    #[test]
    #[ignore = "Windows ConPTY trigger-animation E2E is tracked by issue #65"]
    fn q9_armed_helper_tui_active_child_uses_fallback_instead_of_alt_screen() {}

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
