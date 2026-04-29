//! Smoke test for the `portable-pty` integration.
//!
//! This verifies the dependency is wired correctly and behaves
//! consistently across Unix and Windows. The wrapper itself
//! (PTY child + I/O pump) lands in Phase 1; this test only
//! confirms we can:
//!
//! 1. open a PTY pair,
//! 2. spawn a tiny platform-native echo command in it,
//! 3. read the child's output without blocking forever, and
//! 4. observe a clean exit.
//!
//! Reads are bounded (max iterations + a wall-clock budget) so
//! the test fails fast on regression rather than hanging CI.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::time::{Duration, Instant};

const READ_BUDGET: Duration = Duration::from_secs(5);
const MAX_READ_ITERATIONS: usize = 64;

#[test]
fn portable_pty_echoes_hi() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty pair");

    let mut cmd = if cfg!(windows) {
        let mut c = CommandBuilder::new("cmd");
        c.args(["/C", "echo hi"]);
        c
    } else {
        let mut c = CommandBuilder::new("/bin/echo");
        c.arg("hi");
        c
    };
    // Avoid inheriting the parent test's CWD when it might not
    // exist on the worker (rare, but defensive).
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair.slave.spawn_command(cmd).expect("spawn echo child");

    // The slave handle is owned by the spawned child; drop our
    // local copy so the master's read can observe EOF when the
    // child exits.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("clone master reader");
    drop(pair.master);

    let mut captured = Vec::new();
    let mut buf = [0u8; 256];
    let deadline = Instant::now() + READ_BUDGET;
    for _ in 0..MAX_READ_ITERATIONS {
        if Instant::now() >= deadline {
            break;
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => captured.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => panic!("pty read failed: {e}"),
        }
    }

    let status = child.wait().expect("child wait");
    assert!(status.success(), "child exited non-zero: {status:?}");

    let captured_str = String::from_utf8_lossy(&captured);
    assert!(
        captured_str.contains("hi"),
        "expected 'hi' in pty output, got: {captured_str:?}"
    );
}
