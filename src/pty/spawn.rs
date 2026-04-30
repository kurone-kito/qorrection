//! Spawn a child on a fresh PTY pair.
//!
//! Building block for the wrap session body. PR 5 (#26) wires
//! [`spawn_child`] into `default_body`; PR 2 ships only the
//! primitive plus the preflight classifier needed to honor the
//! `Error::Spawn(NotFound) → 127` /
//! `Error::Spawn(PermissionDenied) → 126` exit-code contract from
//! `src/error.rs`.
//!
//! ## Why a project-side preflight classifier
//!
//! `portable-pty` 0.9 does its own `PATH` walk in
//! `CommandBuilder::as_command()` and reports every
//! command-resolution failure with formatted `anyhow::bail!`
//! messages -- the underlying `std::io::Error` (if there even
//! was one) is not preserved in the cause chain. Therefore
//! `anyhow::Error::downcast::<io::Error>()` on the result of
//! `spawn_command` will *not* classify a missing command as a
//! `NotFound` `io::Error`. Parsing the human-readable bail
//! messages would be brittle.
//!
//! Instead we classify the command path ourselves before
//! handing off to portable-pty. portable-pty becomes the
//! fallback for the post-`fork` failure modes only it can
//! observe (`setsid`, `TIOCSCTTY`, fd dup), which we route
//! through [`map_spawn_error`] -- still preferring
//! `Error::Spawn(io::Error)` if downstream ever does start
//! preserving an io error in the cause chain.

use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::{Error, Result};

/// Classify `command` as plausibly spawnable before calling
/// `portable-pty`. Surfaces missing / non-executable paths as
/// `Error::Spawn(io::Error)` so the existing 127 / 126 exit-code
/// mapping in `src/error.rs` fires.
///
/// Returns `Ok(())` when the command exists and looks executable
/// (under the platform's notion of executable). The post-`fork`
/// failure modes that only the OS can detect -- e.g. setuid
/// mismatches that `access(X_OK)` accepts but `execve` rejects
/// -- still surface from portable-pty via [`map_spawn_error`].
#[allow(dead_code)] // wired into spawn_child below in the next commit
pub(crate) fn preflight_command(command: &OsStr) -> Result<()> {
    let p = Path::new(command);
    // A path containing a separator (or an absolute prefix) is
    // resolved literally; a bare name is searched in PATH.
    if p.is_absolute() || p.components().count() > 1 {
        return classify_path(p, p);
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(p);
        // `classify_path` returns Ok only when the candidate is
        // both present and executable. Any error from a single
        // candidate (NotFound, perm-denied, …) is silently
        // skipped -- the next PATH entry might still resolve.
        if classify_path(&candidate, p).is_ok() {
            return Ok(());
        }
    }
    Err(Error::Spawn(io::Error::new(
        io::ErrorKind::NotFound,
        format!("command not found in PATH: {}", p.display()),
    )))
}

/// Inspect a single concrete path. `display_path` is what the
/// caller wants to surface in the error message (typically the
/// original user-supplied command for bare-name lookups, or the
/// candidate path itself for absolute / relative inputs).
fn classify_path(p: &Path, display_path: &Path) -> Result<()> {
    let meta = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Error::Spawn(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such file: {}", display_path.display()),
            )));
        }
        Err(e) => return Err(Error::Spawn(e)),
    };
    if meta.is_dir() {
        return Err(Error::Spawn(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("is a directory: {}", display_path.display()),
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err(Error::Spawn(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("not executable: {}", display_path.display()),
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn preflight_absolute_missing_path_yields_spawn_not_found() {
        let cmd = os("/definitely-not-there/qorrection-no-such-cmd-xyz");
        let err = preflight_command(&cmd).expect_err("should fail");
        match err {
            Error::Spawn(io) => assert_eq!(io.kind(), io::ErrorKind::NotFound),
            other => panic!("expected Error::Spawn(NotFound), got {other:?}"),
        }
    }

    #[test]
    fn preflight_directory_yields_spawn_permission_denied() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf().into_os_string();
        let err = preflight_command(&path).expect_err("should fail");
        match err {
            Error::Spawn(io) => assert_eq!(io.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected Error::Spawn(PermissionDenied), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn preflight_non_executable_file_yields_spawn_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.txt");
        std::fs::write(&path, b"plain").expect("write");
        let mut perms = std::fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o644); // r/w but no x
        std::fs::set_permissions(&path, perms).expect("set perms");

        let err = preflight_command(path.as_os_str()).expect_err("should fail");
        match err {
            Error::Spawn(io) => assert_eq!(io.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected Error::Spawn(PermissionDenied), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn preflight_real_binary_in_path_succeeds() {
        // /bin/sh is part of POSIX baseline; both Linux and macOS
        // CI runners have it. /bin/echo would also work; sh is
        // chosen because it's the canonical "must exist" binary.
        preflight_command(OsStr::new("/bin/sh")).expect("/bin/sh must exist on Unix");
    }

    #[cfg(unix)]
    #[test]
    fn preflight_bare_name_resolves_via_path() {
        // `sh` is on every PATH on Unix CI runners.
        preflight_command(OsStr::new("sh")).expect("sh must be in PATH on Unix");
    }

    #[test]
    fn preflight_bare_name_not_in_path_yields_not_found() {
        let cmd = os("qorrection-definitely-no-such-bin-xyz-123");
        let err = preflight_command(&cmd).expect_err("should fail");
        match err {
            Error::Spawn(io) => {
                assert_eq!(io.kind(), io::ErrorKind::NotFound);
                let msg = io.to_string();
                assert!(
                    msg.contains("PATH"),
                    "PATH-search miss should mention PATH; got {msg:?}"
                );
            }
            other => panic!("expected Error::Spawn(NotFound), got {other:?}"),
        }
    }
}
