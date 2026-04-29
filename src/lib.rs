//! qorrection -- PTY wrapper that intercepts Vim-style quit
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
pub mod trigger;
pub mod usage;

pub use error::{Error, Result};

use std::process::ExitCode;

/// Entry point for the shipped binaries.
///
/// Reads `std::env::args_os()` and dispatches to [`run`]. Maps
/// any returned [`Error`] to its recommended exit code and prints
/// a one-line diagnostic to stderr, prefixed with the basename
/// of `argv[0]` so error output reflects how the user actually
/// invoked the binary (e.g. `q9: unknown option ...` vs
/// `qorrection: unknown option ...`). Falls back to the crate
/// name when `argv[0]` is missing or empty.
pub fn run_from_env() -> ExitCode {
    init_tracing();
    let mut args = std::env::args_os();
    let argv0 = args.next();
    let prog = program_name(argv0.as_deref());
    match run(args.collect()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{prog}: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}

/// Install a `tracing` subscriber that consults the
/// `QORRECTION_LOG` environment variable.
///
/// Behavior is intentionally conservative because this binary
/// runs as a transparent PTY wrapper around an interactive child
/// process; uninvited stderr output would corrupt the user's
/// terminal session.
///
/// - **`QORRECTION_LOG` unset** → return immediately. The
///   normal interactive path is completely silent.
/// - **`QORRECTION_LOG` set but invalid** → also return
///   silently. We deliberately do not surface filter parser
///   errors on stderr; an invalid value is treated as
///   "diagnostics off".
/// - **`QORRECTION_LOG` set and valid** → install a `fmt`
///   subscriber emitting on stderr with the parsed filter.
///   `set_global_default` is best-effort; a second call (which
///   should not happen in production) becomes a no-op.
///
/// Note: stdin bytes from the user and output bytes from the
/// wrapped child process are **never** logged. Diagnostics are
/// limited to wrapper-internal events (PTY spawn, signal
/// forwarding, trigger detection state) added in later phases.
fn init_tracing() {
    if std::env::var_os("QORRECTION_LOG").is_none() {
        return;
    }
    let Ok(filter) = tracing_subscriber::EnvFilter::try_from_env("QORRECTION_LOG") else {
        return;
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// Derive the diagnostic prefix from `argv[0]`.
///
/// Returns the file-name component of `argv[0]` (lossily
/// converted, since program names are virtually always ASCII /
/// UTF-8 in practice), or the literal `"qorrection"` when
/// `argv[0]` is missing, empty, or has no file-name component.
fn program_name(argv0: Option<&std::ffi::OsStr>) -> String {
    argv0
        .map(std::path::Path::new)
        .and_then(std::path::Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "qorrection".to_string())
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

#[cfg(test)]
mod tests {
    use super::program_name;
    use std::ffi::OsString;

    #[test]
    fn program_name_basenames_full_path() {
        let p = OsString::from("/usr/local/bin/q9");
        assert_eq!(program_name(Some(p.as_os_str())), "q9");
    }

    #[test]
    fn program_name_passes_bare_name_through() {
        let p = OsString::from("qorrection");
        assert_eq!(program_name(Some(p.as_os_str())), "qorrection");
    }

    #[test]
    fn program_name_falls_back_when_argv0_missing() {
        assert_eq!(program_name(None), "qorrection");
    }

    #[test]
    fn program_name_falls_back_when_argv0_empty() {
        let p = OsString::from("");
        assert_eq!(program_name(Some(p.as_os_str())), "qorrection");
    }
}
