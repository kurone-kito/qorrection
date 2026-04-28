//! Command-line dispatcher.
//!
//! Hand-rolled because qorrection's entire CLI surface is four
//! cases:
//!
//! | Invocation              | Variant                |
//! | ----------------------- | ---------------------- |
//! | (no args)               | [`Invocation::Usage`]  |
//! | `-h` / `--help`         | [`Invocation::Usage`]  |
//! | `-V` / `--version`      | [`Invocation::Version`]|
//! | `<cmd> [args...]`       | [`Invocation::Wrap`]   |
//!
//! Anything else that begins with `-` is rejected as
//! [`crate::Error::UnknownOption`]. The bare `--` separator is
//! intentionally not supported in v0.1 and is treated as an
//! unknown option (see the locked v0.1 spec, §3 CLI surface).

use std::ffi::OsString;

use crate::{Error, Result};

pub mod arming;

/// Parsed CLI invocation, ready to dispatch.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// Show the fastfetch-style usage screen on stdout.
    ///
    /// Triggered by no args, `-h`, or `--help`.
    Usage,
    /// Print the POSIX one-liner version on stdout.
    ///
    /// Triggered by `-V` or `--version`.
    Version,
    /// PTY-wrap and run a child command.
    ///
    /// `command` is the first positional (program name as the
    /// user typed it); `args` is everything after, forwarded
    /// verbatim to the child.
    Wrap {
        command: OsString,
        args: Vec<OsString>,
    },
}

/// Parse a sequence of argv tokens (excluding the program name).
///
/// The first non-flag token starts the wrapped command and all
/// subsequent tokens are forwarded verbatim, even if they look
/// like flags. This makes `q9 some-cmd --help` reach the child
/// unmodified.
pub fn parse<I>(args: I) -> Result<Invocation>
where
    I: IntoIterator<Item = OsString>,
{
    let mut iter = args.into_iter();
    let Some(first) = iter.next() else {
        return Ok(Invocation::Usage);
    };

    if let Some(s) = first.to_str() {
        match s {
            "-h" | "--help" => return Ok(Invocation::Usage),
            "-V" | "--version" => return Ok(Invocation::Version),
            _ if s.starts_with('-') => return Err(Error::UnknownOption(first)),
            _ => {}
        }
    } else if first.as_encoded_bytes().first() == Some(&b'-') {
        // Non-UTF-8 token starting with `-` is also unknown.
        return Err(Error::UnknownOption(first));
    }

    Ok(Invocation::Wrap {
        command: first,
        args: iter.collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<const N: usize>(items: [&str; N]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_is_usage() {
        assert_eq!(parse(args([])).unwrap(), Invocation::Usage);
    }

    #[test]
    fn dash_h_is_usage() {
        assert_eq!(parse(args(["-h"])).unwrap(), Invocation::Usage);
    }

    #[test]
    fn long_help_is_usage() {
        assert_eq!(parse(args(["--help"])).unwrap(), Invocation::Usage);
    }

    #[test]
    fn dash_capital_v_is_version() {
        assert_eq!(parse(args(["-V"])).unwrap(), Invocation::Version);
    }

    #[test]
    fn long_version_is_version() {
        assert_eq!(parse(args(["--version"])).unwrap(), Invocation::Version);
    }

    #[test]
    fn lowercase_dash_v_is_unknown() {
        // -v is reserved (potential future verbosity flag); reject it
        // explicitly so we cannot accidentally bind it later.
        let err = parse(args(["-v"])).unwrap_err();
        assert!(matches!(err, Error::UnknownOption(_)));
    }

    #[test]
    fn unknown_long_flag_errors() {
        let err = parse(args(["--bogus"])).unwrap_err();
        assert!(matches!(err, Error::UnknownOption(_)));
    }

    #[test]
    fn double_dash_is_unknown_in_v0_1() {
        // `--` is not a separator in v0.1; treat it as unknown so we
        // do not silently freeze a behavior we have not designed.
        let err = parse(args(["--"])).unwrap_err();
        assert!(matches!(err, Error::UnknownOption(_)));
    }

    #[test]
    fn first_positional_starts_wrap() {
        let inv = parse(args(["claude"])).unwrap();
        assert_eq!(
            inv,
            Invocation::Wrap {
                command: "claude".into(),
                args: vec![],
            }
        );
    }

    #[test]
    fn child_args_are_forwarded_verbatim_including_flags() {
        // The locked spec requires `q9 <cmd> --help` to reach the
        // child unchanged so users can read the wrapped tool's help.
        let inv = parse(args(["claude", "--help", "-V", "--", "extra"])).unwrap();
        assert_eq!(
            inv,
            Invocation::Wrap {
                command: "claude".into(),
                args: vec!["--help".into(), "-V".into(), "--".into(), "extra".into()],
            }
        );
    }

    #[test]
    fn relative_path_command_is_wrap_not_unknown() {
        // `./weird-binary` does not start with `-`, so it's a command.
        let inv = parse(args(["./weird-binary", "arg"])).unwrap();
        assert_eq!(
            inv,
            Invocation::Wrap {
                command: "./weird-binary".into(),
                args: vec!["arg".into()],
            }
        );
    }
}
