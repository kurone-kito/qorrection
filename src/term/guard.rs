//! Drop-based raw-mode RAII guard.
//!
//! Raw mode disables line discipline (no canonical-mode line
//! buffering, no echo, no signal generation) so the wrapper can
//! see every byte the user types. It must be restored on **every**
//! exit path -- normal return, panic, and cooperative
//! signal-driven shutdown -- or the user is left with a wedged
//! terminal until they type `stty sane` blind.
//!
//! Rules (locked v0.1, see plan §6 D-RAWMODE):
//!
//! - Acquire only when **both** stdin and stdout are TTYs. The
//!   non-TTY path bypasses PTY entirely (D-NONTTY) and so must
//!   also bypass raw mode -- touching termios on a pipe is
//!   undefined.
//! - Restoration runs in [`Drop`], so any panic between
//!   acquisition and the end of the session unwinds back through
//!   the guard and disables raw mode on the way out.
//! - Uncatchable termination (SIGKILL, abort) **cannot** be
//!   handled -- release notes recommend `stty sane`.

use crate::{term::TerminalCaps, Result};

/// Decision-only helper, separated from the side-effecting
/// [`acquire`] so it can be unit-tested without touching the
/// real terminal.
pub fn should_arm(caps: &TerminalCaps) -> bool {
    caps.stdin_is_tty && caps.stdout_is_tty
}

/// RAII guard that runs a "disable" hook on [`Drop`] when armed.
///
/// Construct via [`acquire`]. Holding the guard alive (e.g. as
/// a local in `pty::run_session`) keeps raw mode engaged; let
/// it drop to restore the cooked terminal.
///
/// The disable side-effect is stored as an `FnOnce` closure so
/// unit tests can substitute a counter-incrementing hook and
/// assert restoration semantics without touching real termios.
/// Production callers ([`acquire`]) supply
/// `crossterm::terminal::disable_raw_mode`.
pub struct RawGuard {
    on_drop: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl std::fmt::Debug for RawGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawGuard")
            .field("armed", &self.is_armed())
            .finish()
    }
}

impl RawGuard {
    /// Whether this guard currently owns the raw-mode state.
    pub fn is_armed(&self) -> bool {
        self.on_drop.is_some()
    }

    /// Construct a no-op guard for callers that have already
    /// decided not to enter raw mode (e.g. the non-TTY bypass
    /// path).
    pub fn noop() -> Self {
        Self { on_drop: None }
    }

    /// Test-only constructor that produces an armed guard whose
    /// drop hook is the supplied closure. Lets unit tests assert
    /// Drop semantics (normal return, panic) without enabling
    /// real raw mode.
    ///
    /// # Panics in the hook
    ///
    /// The hook MUST NOT panic. If a `RawGuard` is dropped during
    /// unwinding (e.g. inside a `catch_unwind` after `panic!`)
    /// and its hook panics, the second panic aborts the process.
    /// Test hooks should only touch `Arc<AtomicUsize>` /
    /// `Arc<Mutex<_>>` style observers.
    #[cfg(test)]
    pub(crate) fn with_disable_hook<H>(hook: H) -> Self
    where
        H: FnOnce() + Send + 'static,
    {
        Self {
            on_drop: Some(Box::new(hook)),
        }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        // `Option::take` is defensive — `Drop::drop` is called at
        // most once by the language — but it also enforces that
        // the boxed `FnOnce` is consumed exactly once.
        //
        // The production hook is `crossterm::disable_raw_mode`,
        // which is best-effort: if it fails the terminal is
        // already in a bad state and there is no useful recovery
        // from a destructor. Hooks must not panic — see
        // [`RawGuard::with_disable_hook`].
        if let Some(hook) = self.on_drop.take() {
            hook();
        }
    }
}

/// Enter raw mode if and only if both stdin and stdout are TTYs.
///
/// Returns a [`RawGuard`] in either case; the guard is a no-op
/// when raw mode was not entered.
pub fn acquire(caps: &TerminalCaps) -> Result<RawGuard> {
    acquire_with(
        caps,
        || crossterm::terminal::enable_raw_mode().map_err(Into::into),
        || {
            || {
                let _ = crossterm::terminal::disable_raw_mode();
            }
        },
    )
}

/// Decision + wiring core, parameterised over the side-effecting
/// `enable` call and the `make_disable` factory that produces the
/// drop hook. Lets unit tests cover every armed-path branch
/// (non-TTY skip, enable success, enable failure) without
/// touching real termios.
fn acquire_with<E, MD, D>(caps: &TerminalCaps, enable: E, make_disable: MD) -> Result<RawGuard>
where
    E: FnOnce() -> Result<()>,
    MD: FnOnce() -> D,
    D: FnOnce() + Send + 'static,
{
    if !should_arm(caps) {
        return Ok(RawGuard::noop());
    }
    enable()?;
    Ok(RawGuard {
        on_drop: Some(Box::new(make_disable())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

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

    #[test]
    fn noop_guard_runs_no_hook() {
        // A noop guard has no hook to fire; nothing should be
        // observed after it leaves scope.
        let counter = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&counter);
        {
            let _g = RawGuard::noop();
            // Hold a clone so the closure-equivalent reference
            // is alive; we simply never wire it into the guard.
            let _ = observed;
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn armed_guard_runs_hook_on_normal_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let observed = Arc::clone(&counter);
            let _g = RawGuard::with_disable_hook(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn armed_guard_runs_hook_on_panic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&counter);
        // Build the guard *inside* the catch_unwind closure so
        // only `Arc<AtomicUsize>` crosses the unwind boundary --
        // this avoids needing `AssertUnwindSafe` on `RawGuard`.
        let result = std::panic::catch_unwind(move || {
            let _g = RawGuard::with_disable_hook(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            });
            panic!("boom");
        });
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquire_with_non_tty_skips_enable_and_disable() {
        let enable_calls = Arc::new(AtomicUsize::new(0));
        let disable_calls = Arc::new(AtomicUsize::new(0));
        let enable_observed = Arc::clone(&enable_calls);
        let disable_observed = Arc::clone(&disable_calls);

        let guard = acquire_with(
            &caps(false, false),
            move || {
                enable_observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move || {
                move || {
                    disable_observed.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .unwrap();
        assert!(!guard.is_armed());
        drop(guard);

        assert_eq!(enable_calls.load(Ordering::SeqCst), 0);
        assert_eq!(disable_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn acquire_with_tty_calls_enable_once_and_disable_on_drop() {
        let enable_calls = Arc::new(AtomicUsize::new(0));
        let disable_calls = Arc::new(AtomicUsize::new(0));
        let enable_observed = Arc::clone(&enable_calls);
        let disable_observed = Arc::clone(&disable_calls);

        {
            let guard = acquire_with(
                &caps(true, true),
                move || {
                    enable_observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                move || {
                    move || {
                        disable_observed.fetch_add(1, Ordering::SeqCst);
                    }
                },
            )
            .unwrap();
            assert!(guard.is_armed());
            assert_eq!(enable_calls.load(Ordering::SeqCst), 1);
            assert_eq!(disable_calls.load(Ordering::SeqCst), 0);
        }

        assert_eq!(enable_calls.load(Ordering::SeqCst), 1);
        assert_eq!(disable_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquire_with_tty_propagates_enable_error_and_skips_disable() {
        let disable_calls = Arc::new(AtomicUsize::new(0));
        let disable_observed = Arc::clone(&disable_calls);

        let result = acquire_with(
            &caps(true, true),
            || {
                Err(crate::Error::Terminal(std::io::Error::other(
                    "synthetic enable failure",
                )))
            },
            move || {
                move || {
                    disable_observed.fetch_add(1, Ordering::SeqCst);
                }
            },
        );

        assert!(matches!(result, Err(crate::Error::Terminal(_))));
        assert_eq!(disable_calls.load(Ordering::SeqCst), 0);
    }
}
