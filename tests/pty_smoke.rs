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
//! All blocking calls (PTY `read`, child `wait`) are wrapped in
//! a deadline-driven loop so the test fails fast on regression
//! rather than hanging CI. If the deadline fires we kill the
//! child via the cross-thread `ChildKiller` handle and panic
//! with a diagnostic.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const READ_BUDGET: Duration = Duration::from_secs(5);
const WAIT_BUDGET: Duration = Duration::from_secs(5);
const WAIT_POLL: Duration = Duration::from_millis(20);

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
    // Best-effort: anchor the child to a known-readable CWD if
    // one is available. We only set `cwd` when `current_dir()`
    // succeeds; if it fails (e.g. the test runner's CWD has been
    // deleted) we leave `CommandBuilder` to inherit whatever the
    // parent uses, which is the same as today's default behaviour.
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair.slave.spawn_command(cmd).expect("spawn echo child");

    // Cross-thread kill handle: lets the main thread terminate
    // the child if the deadline fires while a reader thread is
    // blocked on `read`, instead of hanging CI forever.
    let mut killer = child.clone_killer();

    // The slave handle is owned by the spawned child; drop our
    // local copy so the master's read can observe EOF when the
    // child exits.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone master reader");
    // The smoke command does not need stdin; release the writer
    // immediately so we are not implicitly holding it.
    drop(pair.master.take_writer().expect("take master writer"));
    // NOTE: `pair.master` is intentionally held until after the
    // child has exited. On Windows, dropping the master closes
    // the underlying ConPTY pseudoconsole handle, which can race
    // with the child process startup and surface as
    // `STATUS_DLL_INIT_FAILED` (0xC0000142). Equally important on
    // Windows: the reader cloned above does NOT observe EOF
    // simply because the child exits — the ConPTY only signals
    // EOF once the master itself is closed. We therefore wait
    // for the child first, then drop the master to release the
    // reader (the order Unix is also happy with).
    let mut master = Some(pair.master);

    // Drain the master in a worker thread. Blocking `read` is
    // fine here because the main thread enforces the deadline
    // and will kill the child / drop the master to unblock us.
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

    // Bounded wait for the child first. The reader thread is
    // still draining whatever the child wrote into the PTY; we
    // close the master afterwards to make the reader see EOF.
    let wait_deadline = Instant::now() + WAIT_BUDGET;
    let status = loop {
        match child.try_wait() {
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
                // Best-effort kill + master release so a
                // wait-syscall failure does not leak the child
                // process or strand the reader thread.
                let _ = killer.kill();
                drop(master.take());
                panic!("child wait failed: {e}");
            }
        }
    };

    // Child has exited; closing the master signals EOF to the
    // reader on Windows (Unix is tolerant either way).
    drop(master.take());

    let captured = match rx.recv_timeout(READ_BUDGET) {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(e)) => {
            // Best-effort kill: the read failed but the child may
            // still be running (in pathological cases where
            // try_wait reported exit but a sibling process kept
            // the slave fd open).
            let _ = killer.kill();
            panic!("pty read failed: {e}");
        }
        Err(_) => {
            // Best-effort: try to unblock the reader so the OS
            // eventually reaps the thread. We deliberately do NOT
            // join() here because the whole point of this branch
            // is that the reader is stuck in a syscall that may
            // never return; an unbounded join would re-introduce
            // the hang this timeout exists to prevent. The thread
            // is detached and will be cleaned up at process exit.
            let _ = killer.kill();
            panic!("pty read did not finish within {READ_BUDGET:?}");
        }
    };
    // Successful path: the reader has already sent on the
    // channel and is about to return, so this join completes
    // immediately. Propagate any panic so reader bugs don't
    // hide behind a discarded `Result`.
    reader_thread.join().expect("reader thread panicked");

    assert!(status.success(), "child exited non-zero: {status:?}");

    let captured_str = String::from_utf8_lossy(&captured);
    assert!(
        captured_str.contains("hi"),
        "expected 'hi' in pty output, got: {captured_str:?}"
    );
}
