//! PTY session entry point.
//!
//! [`run_session`] is the structural seat for everything the
//! Wrap path needs to do: acquire raw mode, spawn the wrapped
//! child on a pseudo-terminal, forward I/O, and propagate the
//! child's exit status. PR 1 only seats the [`RawGuard`]
//! acquisition; PR 2 lands the spawn building blocks under
//! the [`size`] and [`spawn`] modules ([`size::initial_size`],
//! [`spawn::spawn_child`], [`spawn::SpawnedSession`]) so PR 5
//! (#26) can wire them into [`default_body`]. Subsequent
//! phase-2 PRs fill in forwarders (#23 / #35 / #36), wait +
//! exit code (#24 / #33), and replace [`default_body`] with
//! the real pump (#26).
//!
//! ## Design — the `run_session_with` injection seam
//!
//! The wrap-path invariant `RawGuard` must enforce — "raw mode
//! is acquired before any session work and restored on every
//! returning or unwinding exit path (normal return, `?`,
//! panic-with-unwind)" — cannot be unit-tested through
//! the production [`run_session`] entry because it touches the
//! real terminal and the real `crossterm` enable / disable
//! syscalls. [`run_session_with`] separates the policy
//! (acquire-then-run-body, drop-on-return) from the side
//! effects (real `acquire_raw`, real PTY pump body), so tests
//! can substitute counter-incrementing shims and verify the
//! ordering / lifetime invariants without touching termios.
//!
//! The `command` and `args` parameters are accepted but unused
//! by [`default_body`]; PR 2 (#22) wires them into the spawn
//! call.

mod size;
mod spawn;

use std::ffi::OsString;
use std::process::ExitCode;

use crate::{
    term::{detect, RawGuard, TerminalCaps},
    Result,
};

/// Run a single PTY-wrapped session for `command` + `args`.
///
/// Snapshots the terminal capabilities, acquires raw mode for
/// the duration of the session, and invokes the session body.
/// Returns the exit code the wrapped child should bubble up to
/// the binary entry point.
pub fn run_session(command: OsString, args: Vec<OsString>) -> Result<ExitCode> {
    let caps = detect::detect();
    run_session_with(caps, command, args, crate::term::acquire_raw, default_body)
}

/// Lifecycle core, parameterised over the raw-mode `acquire`
/// strategy and the session `body`. Exists so unit tests can
/// inject shims without touching real termios. See the
/// module-level comment for the invariants this seam protects.
pub(crate) fn run_session_with<A, B>(
    caps: TerminalCaps,
    command: OsString,
    args: Vec<OsString>,
    acquire: A,
    body: B,
) -> Result<ExitCode>
where
    A: FnOnce(&TerminalCaps) -> Result<RawGuard>,
    B: FnOnce(&OsString, &[OsString]) -> Result<ExitCode>,
{
    // Bind the guard to a named local so it lives until the end
    // of the function. `let _ = ...` would drop it immediately
    // and defeat the purpose.
    let _raw = acquire(&caps)?;
    body(&command, &args)
}

/// Placeholder body for PR 1. PR 5 (#26) replaces this with the
/// real PTY pump and removes the diagnostic.
///
/// Uses `\r\n` rather than `\n` because, on a real interactive
/// TTY, the surrounding [`run_session_with`] is now holding the
/// terminal in raw mode where output post-processing (newline
/// translation) is disabled. A bare `\n` would leave the cursor
/// dangling at the end of the line.
///
/// Intentionally emits no `argv[0]`-derived program-name prefix:
/// other diagnostics route through [`crate::program_name`], but
/// this body has no access to `argv[0]` and is throwaway code.
/// PR 5 removes the line entirely, so plumbing the program name
/// through the session seam just to delete it would be churn.
fn default_body(_command: &OsString, _args: &[OsString]) -> Result<ExitCode> {
    eprint!("PTY wrap pending\r\n");
    Ok(ExitCode::from(2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    fn caps() -> TerminalCaps {
        TerminalCaps {
            stdin_is_tty: false,
            stdout_is_tty: false,
            utf8: true,
            color: false,
            dumb: false,
            ci: false,
        }
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
        );

        assert!(matches!(result, Err(crate::Error::Terminal(_))));
        assert!(
            !body_ran.load(Ordering::SeqCst),
            "body ran after acquire failure"
        );
    }
}
