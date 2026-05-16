//! Drop-based alternate-screen + cursor-visibility RAII guard.
//!
//! The animation renderer needs a short-lived "presentation
//! mode": switch into the terminal's alternate screen, hide the
//! cursor, draw frames, then restore the original screen on
//! every exit path. Like [`crate::term::guard::RawGuard`], this
//! wrapper makes restoration a destructor concern so normal
//! return and panic unwinding share the same cleanup path.
//!
//! Scope for this phase:
//!
//! - [`acquire`] enters the alternate screen and hides the
//!   cursor.
//! - [`Drop`] shows the cursor and leaves the alternate screen
//!   with best-effort semantics.
//! - If acquisition fails after the alternate screen is entered
//!   but before the cursor is hidden, the restore path runs
//!   immediately so the terminal is not left half-mutated.
//!
//! The renderer loop itself lands in `#47`; this module only
//! owns the terminal-state guard.

use crate::Result;

/// RAII guard that restores the normal terminal presentation on
/// drop.
///
/// Construct via [`acquire`]. While the guard is alive the
/// renderer may assume the terminal is on the alternate screen
/// and the cursor is hidden.
pub struct TerminalGuard {
    on_drop: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl std::fmt::Debug for TerminalGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalGuard")
            .field("armed", &self.is_armed())
            .finish()
    }
}

impl TerminalGuard {
    /// Whether this guard currently owns the terminal
    /// presentation state.
    pub fn is_armed(&self) -> bool {
        self.on_drop.is_some()
    }

    /// Construct a no-op guard.
    pub fn noop() -> Self {
        Self { on_drop: None }
    }

    /// Test-only constructor for drop-path assertions without
    /// touching the real terminal.
    ///
    /// The hook must not panic. A panic during `Drop` while the
    /// thread is already unwinding aborts the process.
    #[cfg(test)]
    pub(crate) fn with_restore_hook<H>(hook: H) -> Self
    where
        H: FnOnce() + Send + 'static,
    {
        Self {
            on_drop: Some(Box::new(hook)),
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.on_drop.take() {
            hook();
        }
    }
}

/// Enter the alternate screen and hide the cursor.
pub fn acquire() -> Result<TerminalGuard> {
    acquire_with(
        || {
            let mut out = std::io::stdout();
            crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen)?;
            Ok(())
        },
        || {
            let mut out = std::io::stdout();
            crossterm::execute!(out, crossterm::cursor::Hide)?;
            Ok(())
        },
        || {
            || {
                let mut out = std::io::stdout();
                let _ = crossterm::execute!(out, crossterm::cursor::Show);
                let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
            }
        },
    )
}

/// Side-effect-free wiring core for tests.
fn acquire_with<EA, HC, MR, D>(
    enter_alt: EA,
    hide_cursor: HC,
    make_restore: MR,
) -> Result<TerminalGuard>
where
    EA: FnOnce() -> Result<()>,
    HC: FnOnce() -> Result<()>,
    MR: FnOnce() -> D,
    D: FnOnce() + Send + 'static,
{
    enter_alt()?;

    let mut restore = Some(make_restore());
    if let Err(err) = hide_cursor() {
        if let Some(restore) = restore.take() {
            restore();
        }
        return Err(err);
    }

    Ok(TerminalGuard {
        on_drop: Some(Box::new(
            restore
                .take()
                .expect("restore hook must exist after successful acquire"),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn noop_guard_is_not_armed() {
        let guard = TerminalGuard::noop();
        assert!(!guard.is_armed());
        drop(guard);
    }

    #[test]
    fn armed_guard_runs_restore_on_normal_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let observed = Arc::clone(&counter);
            let _guard = TerminalGuard::with_restore_hook(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            });
        }
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn armed_guard_runs_restore_on_panic() {
        let counter = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&counter);

        let result = std::panic::catch_unwind(move || {
            let _guard = TerminalGuard::with_restore_hook(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            });
            panic!("boom");
        });

        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquire_with_success_arms_and_restores_once() {
        let enter_calls = Arc::new(AtomicUsize::new(0));
        let hide_calls = Arc::new(AtomicUsize::new(0));
        let restore_calls = Arc::new(AtomicUsize::new(0));

        {
            let enter_observed = Arc::clone(&enter_calls);
            let hide_observed = Arc::clone(&hide_calls);
            let restore_observed = Arc::clone(&restore_calls);

            let guard = acquire_with(
                move || {
                    enter_observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                move || {
                    hide_observed.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                },
                move || {
                    move || {
                        restore_observed.fetch_add(1, Ordering::SeqCst);
                    }
                },
            )
            .unwrap();

            assert!(guard.is_armed());
            assert_eq!(enter_calls.load(Ordering::SeqCst), 1);
            assert_eq!(hide_calls.load(Ordering::SeqCst), 1);
            assert_eq!(restore_calls.load(Ordering::SeqCst), 0);
        }

        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn acquire_with_enter_failure_returns_error_without_restore() {
        let hide_calls = Arc::new(AtomicUsize::new(0));
        let restore_calls = Arc::new(AtomicUsize::new(0));

        let hide_observed = Arc::clone(&hide_calls);
        let restore_observed = Arc::clone(&restore_calls);

        let err = acquire_with(
            move || Err(std::io::Error::other("enter failed").into()),
            move || {
                hide_observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move || {
                move || {
                    restore_observed.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .unwrap_err();

        assert!(matches!(err, crate::Error::Terminal(_)));
        assert_eq!(hide_calls.load(Ordering::SeqCst), 0);
        assert_eq!(restore_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn acquire_with_hide_failure_restores_immediately() {
        let enter_calls = Arc::new(AtomicUsize::new(0));
        let hide_calls = Arc::new(AtomicUsize::new(0));
        let restore_calls = Arc::new(AtomicUsize::new(0));

        let enter_observed = Arc::clone(&enter_calls);
        let hide_observed = Arc::clone(&hide_calls);
        let restore_observed = Arc::clone(&restore_calls);

        let err = acquire_with(
            move || {
                enter_observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            move || {
                hide_observed.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("hide failed").into())
            },
            move || {
                move || {
                    restore_observed.fetch_add(1, Ordering::SeqCst);
                }
            },
        )
        .unwrap_err();

        assert!(matches!(err, crate::Error::Terminal(_)));
        assert_eq!(enter_calls.load(Ordering::SeqCst), 1);
        assert_eq!(hide_calls.load(Ordering::SeqCst), 1);
        assert_eq!(restore_calls.load(Ordering::SeqCst), 1);
    }
}
