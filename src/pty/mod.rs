//! PTY session entry point.
//!
//! [`run_session`] is the structural seat for everything the
//! Wrap path needs to do: detect the controlling terminal,
//! acquire raw mode (TTY case only), spawn the wrapped child
//! on a pseudo-terminal, forward I/O, and propagate the
//! child's exit status.
//!
//! ## Design — the `run_session_with` injection seam
//!
//! The wrap-path invariant `RawGuard` must enforce — "raw mode
//! is acquired before any session work and restored on every
//! returning or unwinding exit path (normal return, `?`,
//! panic-with-unwind)" — cannot be unit-tested through the
//! production [`run_session`] entry because it touches the
//! real terminal, the real `crossterm` enable/disable syscalls,
//! and a real PTY. [`run_session_with`] separates the policy
//! (route by TTY-ness, then either passthrough OR
//! acquire-then-run-body, drop-on-return) from the side
//! effects (real `acquire_raw`, real PTY pump body, real child
//! spawn), so tests can substitute counter-incrementing shims
//! and verify the ordering, lifetime, and routing invariants
//! without touching termios or spawning processes.
//!
//! ## Non-TTY bypass (issue #29)
//!
//! When either stdin or stdout is not a TTY, [`run_session`]
//! must NOT acquire raw mode and must NOT route the child
//! through the PTY pump:
//!
//! - Acquiring raw mode would mutate parent termios state for
//!   no benefit (no interactive user is reading).
//! - The PTY pump would synthesize a fake controlling terminal
//!   that the wrapped child would mistake for an interactive
//!   session, and would also splice ANSI animation bytes into
//!   redirected stdout — breaking pipelines like
//!   `q9 some-cmd > out.log`.
//!
//! Instead the bypass branch spawns the child via
//! [`std::process::Command`] with stdio inherited from the
//! parent and propagates its exit status through the same
//! [`exit::map_exit_status`] used by the PTY path.

mod exit;
mod forward;
mod pump;
mod session;
mod size;
mod spawn;

use std::ffi::OsString;
use std::process::{Command, ExitCode};

use crate::{
    term::{detect, RawGuard, TerminalCaps},
    Error, Result,
};

/// Run a single PTY-wrapped session for `command` + `args`.
///
/// Snapshots the terminal capabilities, routes through the
/// non-TTY bypass when either stdin or stdout is not a TTY,
/// otherwise acquires raw mode for the duration of the session
/// and invokes the PTY session body. Returns the exit code the
/// wrapped child should bubble up to the binary entry point.
pub fn run_session(command: OsString, args: Vec<OsString>) -> Result<ExitCode> {
    let caps = detect::detect();
    run_session_with(
        caps,
        command,
        args,
        crate::term::acquire_raw,
        default_body,
        non_tty_passthrough,
    )
}

/// Lifecycle core, parameterised over the raw-mode `acquire`
/// strategy, the PTY session `body`, and the non-TTY
/// `passthrough` strategy. Exists so unit tests can inject
/// shims without touching real termios or spawning processes.
/// See the module-level comment for the invariants this seam
/// protects.
///
/// Routing rule: if either stdin or stdout is non-TTY, run
/// `passthrough` and return its result without acquiring raw
/// mode. Otherwise acquire raw mode, run `body`, and let the
/// returned [`RawGuard`] restore the terminal on drop (normal
/// return, `?` propagation, or panic unwind).
pub(crate) fn run_session_with<A, B, P>(
    caps: TerminalCaps,
    command: OsString,
    args: Vec<OsString>,
    acquire: A,
    body: B,
    passthrough: P,
) -> Result<ExitCode>
where
    A: FnOnce(&TerminalCaps) -> Result<RawGuard>,
    B: FnOnce(&OsString, &[OsString]) -> Result<ExitCode>,
    P: FnOnce(&OsString, &[OsString]) -> Result<ExitCode>,
{
    if !(caps.stdin_is_tty && caps.stdout_is_tty) {
        // Non-TTY bypass: no raw mode, no PTY pump. The child
        // inherits the parent's stdio so pipes/redirects work
        // verbatim. Documented at module level.
        return passthrough(&command, &args);
    }
    // Bind the guard to a named local so it lives until the end
    // of the function. `let _ = ...` would drop it immediately
    // and defeat the purpose.
    let _raw = acquire(&caps)?;
    body(&command, &args)
}

/// Non-TTY bypass body. Spawns `command` + `args` with stdio
/// inherited from the parent process and propagates the child's
/// exit status through [`exit::map_exit_status`] so the same
/// signal-death encoding (128 + sig) used by the PTY path
/// applies here too.
fn non_tty_passthrough(command: &OsString, args: &[OsString]) -> Result<ExitCode> {
    let status = Command::new(command.as_os_str())
        .args(args)
        .status()
        .map_err(Error::Spawn)?;
    exit::map_exit_status(portable_pty::ExitStatus::from(status))
}

/// Production wrap body. Spawns the child on a fresh PTY,
/// wires the host↔child I/O pump, and supervises the session
/// to a child exit + bounded forwarder drain.
///
/// `\r\n` line endings are unnecessary here because the
/// supervisor surfaces the child's exit status through the
/// returned [`ExitCode`]; nothing in this body writes to host
/// stdout directly.
fn default_body(command: &OsString, args: &[OsString]) -> Result<ExitCode> {
    let size = size::initial_size();
    tracing::info!(
        program = %command.to_string_lossy(),
        cols = size.cols,
        rows = size.rows,
        "wrap session: spawning child on PTY"
    );
    let mut session = spawn::spawn_child(command, args, size)?;
    // Cover the spawn -> pump handoff: if `start_io_pump` fails
    // (e.g. `try_clone_reader` / `take_writer` returns an error)
    // we'd otherwise drop `SpawnedSession` without killing the
    // child — `Box<dyn portable_pty::Child>`'s Drop is documented
    // not to kill or wait. The guard kills on early-return and is
    // disarmed once `run_pump_session` takes over (it installs
    // its own internal `KillOnDropGuard`).
    let mut kill_guard = session::KillOnDropGuard::armed(session.child.clone_killer());
    let pump = pump::start_io_pump(&mut session, std::io::stdin(), std::io::stdout())?;
    kill_guard.disarm();
    session::run_pump_session(session, pump)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    /// Default test caps simulating an interactive TTY on both
    /// stdio sides so the raw-mode + body branch fires. Tests
    /// that exercise the non-TTY bypass override these fields.
    fn caps() -> TerminalCaps {
        TerminalCaps {
            stdin_is_tty: true,
            stdout_is_tty: true,
            utf8: true,
            color: false,
            dumb: false,
            ci: false,
        }
    }

    /// Sentinel passthrough for tests that exercise the
    /// raw-mode + body branch — the seam must never call it,
    /// so this panics if the routing rule regresses.
    fn no_passthrough(_cmd: &OsString, _args: &[OsString]) -> Result<ExitCode> {
        panic!("passthrough must not be called when both stdio are TTY");
    }

    /// Sentinel acquire/body for tests that exercise the
    /// non-TTY bypass — neither must run, so both panic.
    fn no_acquire(_caps: &TerminalCaps) -> Result<RawGuard> {
        panic!("acquire must not run on the non-TTY bypass path");
    }
    fn no_body(_cmd: &OsString, _args: &[OsString]) -> Result<ExitCode> {
        panic!("body must not run on the non-TTY bypass path");
    }

    /// An armed `RawGuard` whose drop hook increments `counter`.
    /// Returned by injected `acquire` shims so tests can observe
    /// the guard's lifetime.
    fn observed_guard(counter: Arc<AtomicUsize>) -> RawGuard {
        RawGuard::with_disable_hook(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[test]
    fn run_session_with_acquires_guard_before_body() {
        let acquired = Arc::new(AtomicBool::new(false));
        let acquired_for_acquire = Arc::clone(&acquired);
        let acquired_for_body = Arc::clone(&acquired);

        let _exit = run_session_with(
            caps(),
            OsString::from("dummy"),
            Vec::new(),
            move |_caps| {
                acquired_for_acquire.store(true, Ordering::SeqCst);
                Ok(RawGuard::noop())
            },
            move |_cmd, _args| {
                assert!(
                    acquired_for_body.load(Ordering::SeqCst),
                    "body ran before acquire"
                );
                Ok(ExitCode::SUCCESS)
            },
            no_passthrough,
        )
        .unwrap();
    }

    #[test]
    fn run_session_with_calls_acquire_exactly_once() {
        // Pin the "acquired exactly once per session" half of the
        // invariant explicitly. Drop-counter tests cover the
        // restore half indirectly; this asserts the entry-side
        // contract directly so future bodies cannot regress it.
        let acquire_calls = Arc::new(AtomicUsize::new(0));
        let acquire_observed = Arc::clone(&acquire_calls);

        let _exit = run_session_with(
            caps(),
            OsString::from("dummy"),
            Vec::new(),
            move |_caps| {
                acquire_observed.fetch_add(1, Ordering::SeqCst);
                Ok(RawGuard::noop())
            },
            |_cmd, _args| Ok(ExitCode::SUCCESS),
            no_passthrough,
        )
        .unwrap();

        assert_eq!(acquire_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn run_session_with_drops_guard_when_body_returns_ok() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_acquire = Arc::clone(&counter);

        let _exit = run_session_with(
            caps(),
            OsString::from("dummy"),
            Vec::new(),
            move |_caps| Ok(observed_guard(counter_for_acquire)),
            |_cmd, _args| Ok(ExitCode::SUCCESS),
            no_passthrough,
        )
        .unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn run_session_with_drops_guard_on_body_early_err() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_acquire = Arc::clone(&counter);

        let result = run_session_with(
            caps(),
            OsString::from("dummy"),
            Vec::new(),
            move |_caps| Ok(observed_guard(counter_for_acquire)),
            |_cmd, _args| {
                Err(crate::Error::Terminal(std::io::Error::other(
                    "synthetic body failure",
                )))
            },
            no_passthrough,
        );

        assert!(matches!(result, Err(crate::Error::Terminal(_))));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn run_session_with_drops_guard_on_body_panic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_call = Arc::clone(&counter);
        // Build the guard inside the catch_unwind closure so
        // only `Arc<AtomicUsize>` crosses the unwind boundary.
        let panicked = std::panic::catch_unwind(move || {
            let _ = run_session_with(
                caps(),
                OsString::from("dummy"),
                Vec::new(),
                move |_caps| Ok(observed_guard(counter_for_call)),
                |_cmd, _args| -> Result<ExitCode> {
                    panic!("body boom");
                },
                no_passthrough,
            );
        });

        assert!(panicked.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn run_session_with_propagates_acquire_error() {
        let body_ran = Arc::new(AtomicBool::new(false));
        let body_observed = Arc::clone(&body_ran);

        let result = run_session_with(
            caps(),
            OsString::from("dummy"),
            Vec::new(),
            |_caps| {
                Err(crate::Error::Terminal(std::io::Error::other(
                    "synthetic acquire failure",
                )))
            },
            move |_cmd, _args| {
                body_observed.store(true, Ordering::SeqCst);
                Ok(ExitCode::SUCCESS)
            },
            no_passthrough,
        );

        assert!(matches!(result, Err(crate::Error::Terminal(_))));
        assert!(
            !body_ran.load(Ordering::SeqCst),
            "body ran after acquire failure"
        );
    }

    /// Non-TTY bypass routing — when stdin is not a TTY, the
    /// seam must invoke `passthrough` and must NOT call
    /// `acquire` or `body`.
    #[test]
    fn run_session_with_routes_to_passthrough_when_stdin_not_tty() {
        let mut c = caps();
        c.stdin_is_tty = false;

        let passthrough_calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&passthrough_calls);

        let exit = run_session_with(
            c,
            OsString::from("dummy"),
            Vec::new(),
            no_acquire,
            no_body,
            move |_cmd, _args| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(ExitCode::SUCCESS)
            },
        )
        .unwrap();

        assert_eq!(passthrough_calls.load(Ordering::SeqCst), 1);
        // ExitCode lacks Eq/Debug; round-trip through Termination
        // is overkill for a routing test, so just exercise the
        // happy-path return value by dropping it.
        let _ = exit;
    }

    /// Same as above but the non-TTY side is stdout. The
    /// routing rule is "either side non-TTY → bypass" and
    /// stdout-only redirection (`q9 cmd > out`) is the
    /// motivating real-world case.
    #[test]
    fn run_session_with_routes_to_passthrough_when_stdout_not_tty() {
        let mut c = caps();
        c.stdout_is_tty = false;

        let passthrough_calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&passthrough_calls);

        let _exit = run_session_with(
            c,
            OsString::from("dummy"),
            Vec::new(),
            no_acquire,
            no_body,
            move |_cmd, _args| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(ExitCode::SUCCESS)
            },
        )
        .unwrap();

        assert_eq!(passthrough_calls.load(Ordering::SeqCst), 1);
    }

    /// Passthrough errors propagate verbatim — the bypass body
    /// is the sole authority over the exit-code shape on the
    /// non-TTY path, so the seam must not swallow or remap.
    #[test]
    fn run_session_with_propagates_passthrough_error() {
        let mut c = caps();
        c.stdin_is_tty = false;

        let result = run_session_with(
            c,
            OsString::from("dummy"),
            Vec::new(),
            no_acquire,
            no_body,
            |_cmd, _args| {
                Err(crate::Error::Spawn(std::io::Error::other(
                    "synthetic passthrough failure",
                )))
            },
        );

        assert!(matches!(result, Err(crate::Error::Spawn(_))));
    }

    /// `non_tty_passthrough` is the production bypass body and
    /// runs a real subprocess; this verifies it propagates a
    /// success exit code from a trivially-available command.
    #[cfg(unix)]
    #[test]
    fn non_tty_passthrough_runs_true_command() {
        let result = non_tty_passthrough(&OsString::from("true"), &[]);
        assert!(result.is_ok(), "expected ok, got {result:?}");
    }

    /// And it must propagate a non-zero exit code through the
    /// shared `exit::map_exit_status` mapping rather than
    /// silently coercing every spawn into success. Pins the
    /// "transparent passthrough" contract that rubber-duck
    /// finding #5 (PR #91) called out as untested.
    #[cfg(unix)]
    #[test]
    fn non_tty_passthrough_propagates_nonzero_exit() {
        let args = [OsString::from("-c"), OsString::from("exit 7")];
        let result = non_tty_passthrough(&OsString::from("sh"), &args)
            .expect("sh -c 'exit 7' must complete");
        assert_eq!(format!("{result:?}"), format!("{:?}", ExitCode::from(7)));
    }

    /// And a missing command surfaces as `Error::Spawn` so the
    /// top-level mapping (`NotFound → 127`) applies.
    #[test]
    fn non_tty_passthrough_missing_command_is_spawn_error() {
        let result = non_tty_passthrough(&OsString::from("definitely-not-a-real-command-zz9"), &[]);
        assert!(matches!(result, Err(crate::Error::Spawn(_))));
    }
}
