//! Drop-based raw-mode RAII guard.
//!
//! Raw mode disables line discipline (no canonical-mode line
//! buffering, no echo, no signal generation) so the wrapper can
//! see every byte the user types. It must be restored on **every**
//! exit path — normal return, panic, and cooperative
//! signal-driven shutdown — or the user is left with a wedged
//! terminal until they type `stty sane` blind.
//!
//! Rules (locked v0.1, see plan §6 D-RAWMODE):
//!
//! - Acquire only when **both** stdin and stdout are TTYs. The
//!   non-TTY path bypasses PTY entirely (D-NONTTY) and so must
//!   also bypass raw mode — touching termios on a pipe is
//!   undefined.
//! - Restoration runs in [`Drop`], so any panic between
//!   acquisition and the end of the session unwinds back through
//!   the guard and disables raw mode on the way out.
//! - Uncatchable termination (SIGKILL, abort) **cannot** be
//!   handled — release notes recommend `stty sane`.

use crate::{term::TerminalCaps, Result};

/// Decision-only helper, separated from the side-effecting
/// [`acquire`] so it can be unit-tested without touching the
/// real terminal.
pub fn should_arm(caps: &TerminalCaps) -> bool {
    caps.stdin_is_tty && caps.stdout_is_tty
}

/// RAII guard that disables raw mode on [`Drop`] when armed.
///
/// Construct via [`acquire`]. Holding the guard alive (e.g. as
/// a local in `pty::run_session`) keeps raw mode engaged; let
/// it drop to restore the cooked terminal.
#[derive(Debug)]
pub struct RawGuard {
    armed: bool,
}

impl RawGuard {
    /// Whether this guard currently owns the raw-mode state.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// Construct a no-op guard for callers that have already
    /// decided not to enter raw mode (e.g. the non-TTY bypass
    /// path).
    pub fn noop() -> Self {
        Self { armed: false }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.armed {
            // Best-effort: if we cannot disable raw mode the
            // terminal is already in a bad state and there is
            // no useful recovery from a destructor.
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

/// Enter raw mode if and only if both stdin and stdout are TTYs.
///
/// Returns a [`RawGuard`] in either case; the guard is a no-op
/// when raw mode was not entered.
pub fn acquire(caps: &TerminalCaps) -> Result<RawGuard> {
    if !should_arm(caps) {
        return Ok(RawGuard::noop());
    }
    crossterm::terminal::enable_raw_mode()?;
    Ok(RawGuard { armed: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(stdin_tty: bool, stdout_tty: bool) -> TerminalCaps {
        TerminalCaps {
            stdin_is_tty: stdin_tty,
            stdout_is_tty: stdout_tty,
            utf8: true,
            color: stdout_tty,
            dumb: false,
            ci: false,
        }
    }

    #[test]
    fn arms_only_when_both_streams_are_tty() {
        assert!(should_arm(&caps(true, true)));
        assert!(!should_arm(&caps(true, false)));
        assert!(!should_arm(&caps(false, true)));
        assert!(!should_arm(&caps(false, false)));
    }

    #[test]
    fn noop_guard_is_not_armed() {
        let g = RawGuard::noop();
        assert!(!g.is_armed());
        // Drop is a no-op; the test passing without a panic is
        // the assertion.
        drop(g);
    }

    #[test]
    fn acquire_on_non_tty_caps_returns_noop() {
        // We can drive this branch deterministically because the
        // gating decision lives entirely in `should_arm` and does
        // not touch crossterm.
        let g = acquire(&caps(false, true)).unwrap();
        assert!(!g.is_armed());
        let g = acquire(&caps(true, false)).unwrap();
        assert!(!g.is_armed());
    }
}
