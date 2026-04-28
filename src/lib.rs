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

pub mod anim;
pub mod cli;
pub mod error;
#[cfg(unix)]
pub mod signals;
pub mod term;
pub mod usage;

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
            // Pick the current terminal width (fall back to 80
            // when not on a TTY or detection fails) so the
            // fastfetch-style layout responds to the user's
            // window. Phase E will refine TTY-vs-pipe handling
            // for piped stdout; for now we always render to
            // stdout because every Usage path is reachable from
            // an interactive prompt.
            let cols = crossterm::terminal::size().map(|(c, _)| c).unwrap_or(80);
            print!("{}", usage::render(cols));
            Ok(ExitCode::SUCCESS)
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
