//! Shared integration-test fixtures.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[allow(dead_code)]
pub const READY_THEN_EXIT_RELEASE_FILE_ENV: &str = "QORRECTION_HELPER_RELEASE_FILE";

/// Tempdir-backed helper named like an armed AI CLI.
///
/// The helper lives at the front of a synthetic PATH so tests
/// can invoke an allowlisted command name without changing the
/// production arming policy.
pub struct ArmedHelper {
    _dir: tempfile::TempDir,
    command: OsString,
    path: OsString,
}

impl ArmedHelper {
    /// Create a helper that echoes one stdin line to stdout.
    ///
    /// Both platforms read exactly one line of stdin and emit it
    /// followed by a single LF, so the fixture's contract is
    /// identical regardless of whether the underlying shell is
    /// `/bin/sh` or `cmd.exe`.
    pub fn echo_stdin() -> Self {
        Self::from_scripts(
            "#!/bin/sh\nIFS= read -r line\nprintf '%s\\n' \"$line\"\n",
            "@echo off\r\nsetlocal EnableDelayedExpansion\r\nset /p line=\r\necho(!line!\r\n",
        )
    }

    /// Create a helper that writes deterministic plain stdout.
    ///
    /// Shared integration-test fixtures are compiled once per
    /// `tests/*.rs` target, so some helpers are intentionally
    /// unused in a given target even though other integration
    /// tests exercise them.
    #[allow(dead_code)]
    pub fn plain_stdout() -> Self {
        Self::from_scripts(
            "#!/bin/sh\nprintf 'plain-output\\n'\n",
            "@echo off\r\necho plain-output\r\n",
        )
    }

    /// Create a helper that announces readiness, then exits 7
    /// after the driving test releases it through a file path in
    /// [`READY_THEN_EXIT_RELEASE_FILE_ENV`].
    ///
    /// The out-of-band release file makes the mid-animation exit
    /// deterministic relative to the parent PTY entering the
    /// alternate screen, avoiding short wall-clock races on
    /// loaded CI hosts.
    #[allow(dead_code)]
    pub fn ready_then_exit_seven() -> Self {
        Self::from_scripts(
            "#!/bin/sh\nprintf 'READY\\n'\nrelease_file=${QORRECTION_HELPER_RELEASE_FILE:?}\nwhile [ ! -f \"$release_file\" ]; do\n  sleep 0.05\ndone\nexit 7\n",
            "@echo off\r\necho READY\r\npowershell -NoProfile -Command \"$path=$env:QORRECTION_HELPER_RELEASE_FILE; if (-not $path) { exit 7 }; while (-not (Test-Path -LiteralPath $path)) { Start-Sleep -Milliseconds 50 }; exit 7\"\r\nexit /b %ERRORLEVEL%\r\n",
        )
    }

    /// Command name to pass to `q9`.
    pub fn command(&self) -> &OsStr {
        &self.command
    }

    /// PATH value with the helper directory prepended.
    pub fn path(&self) -> &OsStr {
        &self.path
    }

    fn from_scripts(unix_body: &str, windows_body: &str) -> Self {
        let dir = tempfile::tempdir().expect("create armed-helper tempdir");
        let (command, body) = helper_script(unix_body, windows_body);
        let script = dir.path().join(command);
        std::fs::write(&script, body).expect("write armed-helper script");
        make_executable(&script);
        let path = path_with_front(dir.path().to_path_buf());

        Self {
            _dir: dir,
            command: command.into(),
            path,
        }
    }
}

#[cfg(unix)]
fn helper_script<'a>(unix_body: &'a str, _windows_body: &'a str) -> (&'static str, &'a str) {
    ("claude", unix_body)
}

#[cfg(windows)]
fn helper_script<'a>(_unix_body: &'a str, windows_body: &'a str) -> (&'static str, &'a str) {
    ("claude.cmd", windows_body)
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = std::fs::metadata(path)
        .expect("stat armed-helper script")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod armed-helper script");
}

#[cfg(windows)]
fn make_executable(_path: &std::path::Path) {}

fn path_with_front(front: PathBuf) -> OsString {
    let mut paths = vec![front];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("join PATH entries")
}
