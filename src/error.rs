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
    /// Maps to exit code 2 and is rendered as a usage message on
    /// stderr by the binary entry point.
    #[error("unknown option: {0:?}")]
    UnknownOption(OsString),
}

impl Error {
    /// Recommended process exit code for this error variant.
    ///
    /// All current variants are CLI-usage errors and use the
    /// POSIX-conventional `2`.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::UnknownOption(_) => 2,
        }
    }
}

/// Convenience alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

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
}
