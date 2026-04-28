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

pub mod cli;
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
/// Parses the argv tail and dispatches to a stub branch. Real
/// behavior (usage screen, version line, PTY wrap) is wired in
/// the following commits per the implementation plan.
pub fn run(args: Vec<std::ffi::OsString>) -> Result<ExitCode> {
    match cli::parse(args)? {
        cli::Invocation::Usage => {
            eprintln!("qorrection: usage screen pending");
            Ok(ExitCode::from(2))
        }
        cli::Invocation::Version => {
            println!("qorrection {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        cli::Invocation::Wrap { .. } => {
            eprintln!("qorrection: PTY wrap pending");
            Ok(ExitCode::from(2))
        }
    }
}
