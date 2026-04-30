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

use portable_pty::{native_pty_system, CommandBuilder, PtyPair, PtySize};

use crate::{Error, Result};

/// Owning bundle of a live PTY spawn.
///
/// Field declaration order = drop order. `child` ships first
/// so its handle is dropped before the master/slave fds are
/// released, which keeps PTY teardown semantics predictable
/// (the master sees EOF after the child handle is gone).
///
/// **Drop is NOT a complete shutdown protocol** -- Rust's drop
/// for `Box<dyn Child>` neither waits for nor kills the
/// underlying process. PR 4 / #33 owns the explicit wait+kill
/// ladder; until then, callers must arrange shutdown
/// themselves (e.g. via [`portable_pty::Child::clone_killer`]).
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) struct SpawnedSession {
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub pair: PtyPair,
}

/// Spawn `command` + `args` on a fresh PTY pair sized `size`.
///
/// Honors the `Error::Spawn` -> 127 / 126 contract via
/// [`preflight_command`] before invoking portable-pty;
/// post-`fork` failures from portable-pty itself surface
/// through [`map_spawn_error`].
#[allow(dead_code)] // wired into default_body in PR 5 / #26
pub(crate) fn spawn_child(
    command: &OsStr,
    args: &[std::ffi::OsString],
    size: PtySize,
) -> Result<SpawnedSession> {
    #[cfg(unix)]
    preflight_command(command)?;

    let system = native_pty_system();
    let pair = system.openpty(size).map_err(Error::Pty)?;

    let mut builder = CommandBuilder::new(command);
    for arg in args {
        builder.arg(arg);
    }
    // Best-effort: anchor the child to the parent's CWD if the
    // call succeeds. Mirrors `tests/pty_smoke.rs`: silently
    // inherit when `current_dir()` fails (e.g. CWD deleted).
    if let Ok(cwd) = std::env::current_dir() {
        builder.cwd(cwd);
    }

    let child = pair.slave.spawn_command(builder).map_err(map_spawn_error)?;
    Ok(SpawnedSession { child, pair })
}

/// Defensive secondary classifier for failures that slip past
/// [`preflight_command`] (e.g. portable-pty's internal
/// `setsid`/`TIOCSCTTY`/fd-dup failures, or future portable-pty
/// versions that *do* preserve an `io::Error` in the cause
/// chain). The preflight is the primary contract for #34; this
/// keeps the Spawn / Pty distinction working for everything
/// else.
fn map_spawn_error(err: anyhow::Error) -> Error {
    match err.downcast::<io::Error>() {
        Ok(io) => Error::Spawn(io),
        Err(other) => Error::Pty(other),
    }
}

/// Classify `command` as plausibly spawnable before calling
/// `portable-pty`. Surfaces missing / non-executable paths as
/// `Error::Spawn(io::Error)` so the existing 127 / 126 exit-code
/// mapping in `src/error.rs` fires.
///
/// **Unix-only.** On Windows, `portable-pty`'s resolver consults
/// `PATHEXT`, so a literal exact-match preflight here would
/// reject normal invocations like `git`/`python` that resolve to
/// `git.exe`/`python.exe`. Until a cross-platform PATHEXT-aware
/// implementation lands, Windows leans on `portable-pty` +
/// [`map_spawn_error`] for command-resolution failure
/// classification (see PR 2 RD finding #3).
///
/// Returns `Ok(())` when the command exists and looks executable
/// (under the platform's notion of executable). The post-`fork`
/// failure modes that only the OS can detect -- e.g. setuid
/// mismatches that `access(X_OK)` accepts but `execve` rejects
/// -- still surface from portable-pty via [`map_spawn_error`].
#[cfg(unix)]
pub(crate) fn preflight_command(command: &OsStr) -> Result<()> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    preflight_command_with_path(command, path_var.as_os_str())
}

/// Pure variant: takes the `PATH` value as an argument so tests
/// can drive PATH-walk classification without mutating the
/// process-wide environment (which would race with parallel
/// tests).
#[cfg(unix)]
fn preflight_command_with_path(command: &OsStr, path_var: &OsStr) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    let p = Path::new(command);
    // A path containing a separator (or an absolute prefix) is
    // resolved literally; a bare name is searched in PATH. We
    // detect a separator at the byte level rather than via
    // `Path::components().count() > 1` because the latter
    // collapses `foo/`, single-component absolutes, and other
    // edge shapes (RD finding #4).
    let has_sep = command.as_bytes().contains(&b'/');
    if p.is_absolute() || has_sep {
        return classify_path(p, p);
    }

    // PATH walk. POSIX `execvp` semantics: a candidate that
    // exists but is not executable yields `EACCES` (-> 126),
    // not `ENOENT` (-> 127). Track the first non-NotFound
    // classification we encounter and surface it once the walk
    // exhausts without finding an executable candidate
    // (RD finding #1). NotFound entries are silently skipped
    // (a missing PATH entry is normal during a search).
    let mut first_non_not_found: Option<Error> = None;
    for dir in std::env::split_paths(path_var) {
        let candidate = dir.join(p);
        match classify_path(&candidate, p) {
            Ok(()) => return Ok(()),
            Err(Error::Spawn(io)) if io.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                if first_non_not_found.is_none() {
                    first_non_not_found = Some(e);
                }
            }
        }
    }
    if let Some(e) = first_non_not_found {
        return Err(e);
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
#[cfg(unix)]
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

    // ---- Preflight tests (Unix-only: see preflight_command docs) ---
    #[cfg(unix)]
    mod preflight {
        use super::*;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

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

        #[test]
        fn preflight_real_binary_in_path_succeeds() {
            // /bin/sh is part of POSIX baseline; both Linux and macOS
            // CI runners have it. /bin/echo would also work; sh is
            // chosen because it's the canonical "must exist" binary.
            preflight_command(OsStr::new("/bin/sh")).expect("/bin/sh must exist on Unix");
        }

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

        // RD finding #1: PATH walk must preserve "found but not
        // executable" classification (-> 126), not collapse it to
        // NotFound (-> 127).
        #[test]
        fn preflight_path_walk_preserves_permission_denied_when_only_candidate_is_a_dir() {
            let dir = tempfile::tempdir().expect("tempdir");
            let bare = "qorrection-rd1-dir-name";
            std::fs::create_dir(dir.path().join(bare)).expect("mkdir");
            let path_var = OsString::from_vec(dir.path().as_os_str().as_bytes().to_vec());
            let err =
                preflight_command_with_path(OsStr::new(bare), &path_var).expect_err("should fail");
            match err {
                Error::Spawn(io) => assert_eq!(
                    io.kind(),
                    io::ErrorKind::PermissionDenied,
                    "expected PermissionDenied (-> exit 126), got io={io:?}"
                ),
                other => panic!("expected Error::Spawn(PermissionDenied), got {other:?}"),
            }
        }

        #[test]
        fn preflight_path_walk_preserves_permission_denied_when_only_candidate_not_executable() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().expect("tempdir");
            let bare = "qorrection-rd1-noexec";
            let file = dir.path().join(bare);
            std::fs::write(&file, b"plain").expect("write");
            let mut perms = std::fs::metadata(&file).expect("meta").permissions();
            perms.set_mode(0o644);
            std::fs::set_permissions(&file, perms).expect("set perms");
            let path_var = OsString::from_vec(dir.path().as_os_str().as_bytes().to_vec());
            let err =
                preflight_command_with_path(OsStr::new(bare), &path_var).expect_err("should fail");
            match err {
                Error::Spawn(io) => assert_eq!(
                    io.kind(),
                    io::ErrorKind::PermissionDenied,
                    "expected PermissionDenied (-> exit 126), got io={io:?}"
                ),
                other => panic!("expected Error::Spawn(PermissionDenied), got {other:?}"),
            }
        }

        // RD finding #1 (positive): an early bad candidate must
        // NOT shadow a later good candidate. PATH walk continues
        // until a viable candidate succeeds.
        #[test]
        fn preflight_path_walk_skips_bad_candidate_for_later_good_candidate() {
            use std::os::unix::fs::PermissionsExt;
            let bad = tempfile::tempdir().expect("bad dir");
            let good = tempfile::tempdir().expect("good dir");
            let bare = "qorrection-rd1-shadowed";
            std::fs::create_dir(bad.path().join(bare)).expect("mkdir bad");
            let good_path = good.path().join(bare);
            std::fs::write(&good_path, b"#!/bin/sh\nexit 0\n").expect("write good");
            let mut perms = std::fs::metadata(&good_path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&good_path, perms).expect("set perms");
            let joined = std::env::join_paths([bad.path(), good.path()]).expect("join");
            preflight_command_with_path(OsStr::new(bare), &joined).expect("should resolve good");
        }

        // RD finding #4: trailing-separator paths like `foo/` must
        // be detected as separator-bearing and resolved literally,
        // not search-walked.
        #[test]
        fn preflight_trailing_slash_classifies_literally() {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut p = dir.path().as_os_str().to_owned();
            p.push("/");
            let err = preflight_command(&p).expect_err("should fail (is a dir)");
            match err {
                Error::Spawn(io) => {
                    let msg = io.to_string();
                    assert!(
                        !msg.contains("not found in PATH"),
                        "trailing-slash input must not be PATH-walked; got {msg:?}"
                    );
                }
                other => panic!("expected Error::Spawn(_), got {other:?}"),
            }
        }
    }

    #[test]
    fn map_spawn_error_classifies_io_as_spawn_variant() {
        let io_err = io::Error::from(io::ErrorKind::NotFound);
        let err = map_spawn_error(anyhow::Error::from(io_err));
        match err {
            Error::Spawn(io) => assert_eq!(io.kind(), io::ErrorKind::NotFound),
            other => panic!("expected Error::Spawn, got {other:?}"),
        }
    }

    #[test]
    fn map_spawn_error_classifies_non_io_as_pty_variant() {
        let err = map_spawn_error(anyhow::anyhow!("synthetic non-io failure"));
        assert!(
            matches!(err, Error::Pty(_)),
            "expected Error::Pty, got {err:?}"
        );
    }

    // ---- Unix-only end-to-end real-spawn tests --------------
    //
    // These mirror the bounded-deadline pattern in
    // `tests/pty_smoke.rs` deliberately verbatim (constants
    // duplicated locally) instead of refactoring a shared
    // helper, to avoid a diff conflict with PR 3 which owns the
    // forwarder helpers. Rubber-duck-filtered finding #6.
    #[cfg(unix)]
    mod real_spawn {
        use super::super::*;
        use portable_pty::PtySize;
        use std::io::Read;
        use std::sync::mpsc;
        use std::thread;
        use std::time::{Duration, Instant};

        const READ_BUDGET: Duration = Duration::from_secs(5);
        const WAIT_BUDGET: Duration = Duration::from_secs(5);
        const WAIT_POLL: Duration = Duration::from_millis(20);

        fn pty_size_80x24() -> PtySize {
            PtySize {
                cols: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            }
        }

        #[test]
        fn spawn_child_runs_real_echo() {
            let mut session = spawn_child(
                OsStr::new("/bin/echo"),
                &[std::ffi::OsString::from("hi")],
                pty_size_80x24(),
            )
            .expect("spawn /bin/echo");

            let mut killer = session.child.clone_killer();

            // Drop slave so the master observes EOF when child exits.
            drop(session.pair.slave);

            let mut reader = session
                .pair
                .master
                .try_clone_reader()
                .expect("clone reader");
            let mut master = Some(session.pair.master);

            let (tx, rx) = mpsc::channel::<std::io::Result<Vec<u8>>>();
            let reader_thread = thread::spawn(move || {
                let mut captured = Vec::new();
                let mut buf = [0u8; 256];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => captured.extend_from_slice(&buf[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            let _ = tx.send(Err(e));
                            return;
                        }
                    }
                }
                let _ = tx.send(Ok(captured));
            });

            let wait_deadline = Instant::now() + WAIT_BUDGET;
            let status = loop {
                match session.child.try_wait() {
                    Ok(Some(s)) => break s,
                    Ok(None) => {
                        if Instant::now() >= wait_deadline {
                            let _ = killer.kill();
                            drop(master.take());
                            panic!("child did not exit within {WAIT_BUDGET:?}");
                        }
                        thread::sleep(WAIT_POLL);
                    }
                    Err(e) => {
                        let _ = killer.kill();
                        drop(master.take());
                        panic!("child wait failed: {e}");
                    }
                }
            };
            drop(master.take());

            let captured = match rx.recv_timeout(READ_BUDGET) {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(e)) => {
                    let _ = killer.kill();
                    panic!("pty read failed: {e}");
                }
                Err(_) => {
                    let _ = killer.kill();
                    panic!("pty read did not finish within {READ_BUDGET:?}");
                }
            };
            reader_thread.join().expect("reader thread panicked");

            assert!(status.success(), "child exited non-zero: {status:?}");
            let captured_str = String::from_utf8_lossy(&captured);
            assert!(
                captured_str.contains("hi"),
                "expected 'hi' in pty output, got: {captured_str:?}"
            );
        }

        #[test]
        fn spawn_child_returns_spawn_not_found_for_missing_command() {
            let result = spawn_child(
                OsStr::new("/definitely-not-there/qorrection-no-such-cmd-xyz"),
                &[],
                pty_size_80x24(),
            );
            match result {
                Err(Error::Spawn(io)) => assert_eq!(io.kind(), io::ErrorKind::NotFound),
                Err(other) => panic!("expected Error::Spawn(NotFound), got {other:?}"),
                Ok(_) => panic!("expected spawn to fail for missing command"),
            }
        }
    }
}
