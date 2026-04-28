//! qorrection — PTY wrapper that intercepts Vim-style quit
//! commands and responds with playful animations.
//!
//! This crate exposes a thin library boundary so that integration
//! tests can drive the CLI dispatcher directly without spawning a
//! subprocess. The shipping entry points live under `src/bin/`
//! (`qorrection` and `q9`); they are intentionally kept minimal
//! and forward to [`run_from_env`].
//!
//! The PTY wrapper, trigger detection, and animation renderer
//! will be added in subsequent phases per the project plan.

pub mod error;

pub use error::{Error, Result};

use std::process::ExitCode;

/// Entry point for the shipped binaries.
///
/// Reads `std::env::args_os()` and dispatches to [`run`]. Maps
/// any returned [`Error`] to its recommended exit code and prints
/// a one-line diagnostic to stderr.
pub fn run_from_env() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("qorrection: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

/// Library-level entry point.
///
/// Takes the parsed argv tail (i.e. without the program name) and
/// returns the resulting [`ExitCode`]. Until the dispatcher lands
/// in a subsequent commit this is a placeholder that mirrors the
/// previous binary behavior.
pub fn run(_args: Vec<std::ffi::OsString>) -> Result<ExitCode> {
    eprintln!("qorrection: implementation pending");
    Ok(ExitCode::from(2))
}
