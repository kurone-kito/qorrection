//! Crate-level error type.
//!
//! [`Error`] is the boundary type returned by fallible
//! library operations. Binaries map it to an [`std::process::ExitCode`]
//! via [`Error::exit_code`].
//!
//! Variants are intentionally narrow at this stage; subsequent
//! phases will add transports for PTY, terminal, and trigger
//! errors as those modules land.

use std::ffi::OsString;

/// Errors returned by [`crate::run`] and downstream library
/// functions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The user passed a flag-like argument that the CLI does not
    /// recognize (anything starting with `-` that isn't one of
    /// the four metadata cases).
    ///
    /// Maps to exit code 2 and is rendered by [`crate::run_from_env`]
    /// as a one-line stderr diagnostic prefixed with the program
    /// name (e.g. `qorrection: unknown option: "--bogus"`); the
    /// actual usage screen is only printed for the dedicated
    /// `-h`/`--help` cases.
    #[error("unknown option: {0:?}")]
    UnknownOption(OsString),

    /// A terminal I/O operation failed (raw-mode toggle, size
    /// query, etc.). The wrapped [`std::io::Error`] preserves
    /// the original errno / source chain.
    ///
    /// Maps to exit code 2 -- there is nothing the user can do
    /// other than re-run, and 2 is consistent with our other
    /// pre-flight failures.
    #[error("terminal I/O failed: {0}")]
    Terminal(#[from] std::io::Error),

    /// The PTY backend failed (open, master/slave clone, resize,
    /// etc.). Wraps `anyhow::Error` because `portable-pty`'s API
    /// returns `anyhow::Result` and we want to preserve its full
    /// context chain at the crate boundary.
    ///
    /// Maps to exit code `1` -- general runtime failure. The PTY
    /// layer is mandatory; if it cannot start, no useful work can
    /// follow, but the failure is not a "user mis-invocation" and
    /// should not collide with code `2`.
    ///
    /// `Display` uses the alternate (`{:#}`) anyhow format so the
    /// full source chain appears in the rendered message; this
    /// stands in for the `Error::source()` integration that
    /// thiserror cannot synthesize for `anyhow::Error` (which is
    /// not itself a `std::error::Error`).
    #[error("PTY backend failure: {0:#}")]
    Pty(anyhow::Error),

    /// Failed to spawn the wrapped child process (`execvp` /
    /// `CreateProcess` failed). The wrapped [`std::io::Error`]
    /// carries the original errno / Win32 error.
    ///
    /// Exit code follows POSIX shell convention:
    /// - [`std::io::ErrorKind::NotFound`] → `127` (command not
    ///   found),
    /// - any other kind → `126` (found but not executable).
    #[error("failed to spawn child process: {0}")]
    Spawn(std::io::Error),

    /// The wrapped child terminated because of an OS signal.
    /// `signum` is the raw signal number reported by the
    /// platform (typically `SIGINT=2`, `SIGTERM=15`, `SIGKILL=9`,
    /// etc. on Unix; synthetic values on Windows).
    ///
    /// Exit code follows the POSIX shell convention `128 + signum`.
    /// `signum` is clamped to `[0, 127]` before adding to keep
    /// the result inside `u8` and to defend against bogus values
    /// from the platform layer.
    #[error("child terminated by signal {signum}")]
    Signal {
        /// Raw signal number as reported by the platform.
        signum: i32,
    },
}

impl Error {
    /// Recommended process exit code for this error variant.
    ///
    /// - `UnknownOption`, `Terminal` → `2` (pre-flight failures).
    /// - `Pty`                       → `1` (general runtime failure).
    /// - `Spawn(NotFound)`           → `127` (POSIX: command not found).
    /// - `Spawn(other)`              → `126` (POSIX: found, not executable).
    /// - `Signal { signum }`         → `128 + clamp(signum, 0, 127)`.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::UnknownOption(_) | Error::Terminal(_) => 2,
            Error::Pty(_) => 1,
            Error::Spawn(e) if e.kind() == std::io::ErrorKind::NotFound => 127,
            Error::Spawn(_) => 126,
            Error::Signal { signum } => {
                // Clamp first to guarantee the add stays inside
                // u8. Negative or absurdly large signums (which
                // should never reach us, but the platform layer
                // is the platform layer) collapse to a defined
                // exit code rather than wrapping or panicking.
                let clamped = (*signum).clamp(0, 127) as u8;
                128u8.saturating_add(clamped)
            }
        }
    }
}

/// Convenience alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn unknown_option_renders_offending_token() {
        let err = Error::UnknownOption("--bogus".into());
        let msg = err.to_string();
        assert!(msg.contains("--bogus"), "rendered message: {msg}");
    }

    #[test]
    fn unknown_option_exits_with_two() {
        let err = Error::UnknownOption("--bogus".into());
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn terminal_exits_with_two() {
        let err = Error::Terminal(io::Error::other("raw-mode toggle failed"));
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn pty_exits_with_one_and_renders_anyhow_chain() {
        let inner = anyhow::anyhow!("openpty failed").context("could not start PTY");
        let err = Error::Pty(inner);
        assert_eq!(err.exit_code(), 1);
        let msg = err.to_string();
        // {0:#} should fold both the outer context and the inner
        // cause into a single line so users see the whole story.
        assert!(
            msg.contains("could not start PTY"),
            "expected outer context in message, got: {msg}"
        );
        assert!(
            msg.contains("openpty failed"),
            "expected inner cause in message, got: {msg}"
        );
    }

    #[test]
    fn spawn_not_found_exits_with_127() {
        let err = Error::Spawn(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(err.exit_code(), 127);
    }

    #[test]
    fn spawn_permission_denied_exits_with_126() {
        let err = Error::Spawn(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(err.exit_code(), 126);
    }

    #[test]
    fn spawn_other_kind_exits_with_126() {
        let err = Error::Spawn(io::Error::other("totally unexpected"));
        assert_eq!(err.exit_code(), 126);
    }

    #[test]
    fn signal_sigterm_exits_with_143() {
        // SIGTERM = 15, so POSIX shells report 128 + 15 = 143.
        let err = Error::Signal { signum: 15 };
        assert_eq!(err.exit_code(), 143);
    }

    #[test]
    fn signal_sigkill_exits_with_137() {
        let err = Error::Signal { signum: 9 };
        assert_eq!(err.exit_code(), 137);
    }

    #[test]
    fn signal_zero_exits_with_128() {
        let err = Error::Signal { signum: 0 };
        assert_eq!(err.exit_code(), 128);
    }

    #[test]
    fn signal_negative_clamps_to_128() {
        // Defensive: a bogus negative signum from the platform
        // must not wrap or panic; clamp -> 0 -> 128.
        let err = Error::Signal { signum: -1 };
        assert_eq!(err.exit_code(), 128);
    }

    #[test]
    fn signal_huge_clamps_to_255() {
        // Bogus oversized signum collapses to 128 + 127 = 255,
        // the maximum representable u8 exit code.
        let err = Error::Signal { signum: 99_999 };
        assert_eq!(err.exit_code(), 255);
    }

    #[test]
    fn signal_message_includes_signum() {
        let err = Error::Signal { signum: 9 };
        let msg = err.to_string();
        assert!(msg.contains('9'), "expected signum in message: {msg}");
    }
}
