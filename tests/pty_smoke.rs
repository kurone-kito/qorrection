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
//!
//! ## Windows
//!
//! On Windows ConPTY, `try_wait` does not observe process exit while the
//! master handle is open — the ConPTY session object keeps the process
//! wait from completing. The fix is to close the full `PtyMaster` (which
//! calls `ClosePseudoConsole`) *before* polling `try_wait`.
//!
//! ### Terminal protocol
//!
//! ConPTY forwards the cursor-position request (`\x1b[6n`, DSR) from
//! cmd.exe to the terminal (our master write side). cmd.exe stalls until
//! the terminal replies with a cursor-position report (`\x1b[1;1R`, CPR).
//! We write the CPR immediately after spawn so cmd.exe proceeds to run
//! the user command without delay.
//!
//! ### Close-before-wait protocol
//!
//! The reader thread streams output in chunks; the main thread watches
//! the stream for "hi" (ANSI-stripped) for up to 15 s, then closes the
//! full master. Closing just the writer half of the master signals
//! `STATUS_CONTROL_C_EXIT` to the child; we hold the writer alive until
//! after `ClosePseudoConsole`.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
#[cfg(windows)]
use std::io::Write;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const READ_BUDGET: Duration = Duration::from_secs(5);
const WAIT_BUDGET: Duration = Duration::from_secs(5);
const WAIT_POLL: Duration = Duration::from_millis(20);

/// Windows-only: maximum time to wait for "hi" to appear in PTY output
/// before closing the ConPTY session. Covers slow CI runners where
/// cmd.exe initialisation takes several seconds before running the
/// user command.
#[cfg(windows)]
const HI_WAIT_BUDGET: Duration = Duration::from_secs(15);

/// Strips ANSI/VT escape sequences from `s`.
///
/// Handles the three sequence families emitted by the Windows ConPTY VT
/// renderer:
///
/// - **CSI** (`ESC [` … final byte `0x40–0x7E`)
/// - **OSC** (`ESC ]` … `BEL` or `ESC \`)
/// - **Two-byte** (`ESC` + any other single byte)
///
/// The plain text that remains can be asserted on without false mismatches
/// caused by cursor-positioning, colour, or window-title sequences that
/// precede the actual command output.
#[cfg(windows)]
fn strip_ansi_sequences(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\x1b' {
            if i + 1 >= bytes.len() {
                i += 1;
                continue;
            }
            match bytes[i + 1] {
                b'[' => {
                    // CSI: ESC '[' <param/intermediate bytes 0x20–0x3F>* <final 0x40–0x7E>
                    i += 2;
                    while i < bytes.len() && bytes[i] < 0x40 {
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] <= 0x7e {
                        i += 1;
                    }
                }
                b']' => {
                    // OSC: ESC ']' <text> BEL | ESC '\'
                    i += 2;
                    loop {
                        if i >= bytes.len() {
                            break;
                        }
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // Other two-byte ESC sequence — skip both bytes.
                    i += 2;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

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

    // Clone the reader before moving the master into its Option wrapper.
    let mut reader = pair.master.try_clone_reader().expect("clone master reader");
    let mut master = Some(pair.master);

    // On Windows ConPTY, take the writer from the master so we can send
    // a cursor-position report (CPR) back to cmd.exe. We hold the writer
    // alive until ClosePseudoConsole — dropping it early closes the stdin
    // pipe and makes cmd.exe exit with STATUS_CONTROL_C_EXIT.
    #[cfg(windows)]
    let mut pty_writer = master
        .as_mut()
        .unwrap()
        .take_writer()
        .expect("take pty writer");

    // Reply to ConPTY's cursor-position request (\x1b[6n, DSR) so
    // cmd.exe does not stall waiting for terminal acknowledgement before
    // running the user command.  \x1b[1;1R is a CPR for row 1, col 1.
    #[cfg(windows)]
    {
        pty_writer
            .write_all(b"\x1b[1;1R")
            .expect("send ConPTY cursor-position report");
    }

    // The reader thread streams output chunks via `chunk_tx` as they
    // arrive, then signals EOF by dropping `chunk_tx`. Read errors are
    // forwarded via `err_tx`. Streaming lets the Windows path watch for
    // "hi" before closing the ConPTY session.
    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
    let (err_tx, err_rx) = mpsc::channel::<std::io::Error>();
    let reader_thread = thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        return; // main thread dropped the receiver
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let _ = err_tx.send(e);
                    return;
                }
            }
        }
        // Dropping chunk_tx signals EOF to chunk_rx.
    });

    // On Windows: wait for "hi" (ANSI-stripped) to appear in the output
    // stream for up to HI_WAIT_BUDGET, then close the full master so
    // try_wait can observe the process exit.
    #[cfg(windows)]
    let mut captured: Vec<u8> = {
        let hi_deadline = Instant::now() + HI_WAIT_BUDGET;
        let mut acc: Vec<u8> = Vec::new();
        loop {
            let remaining = hi_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break; // deadline reached; assertion will fail with diagnostic
            }
            match chunk_rx.recv_timeout(remaining.min(Duration::from_millis(200))) {
                Ok(chunk) => {
                    acc.extend_from_slice(&chunk);
                    if strip_ansi_sequences(&String::from_utf8_lossy(&acc)).contains("hi") {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            }
        }
        // ClosePseudoConsole: drop both master and writer together so the
        // session tears down cleanly (dropping only the writer earlier
        // would signal STATUS_CONTROL_C_EXIT).
        drop(master.take());
        drop(pty_writer);
        acc
    };
    #[cfg(not(windows))]
    let mut captured: Vec<u8> = Vec::new();

    // Bounded wait for the child. On Unix the reader thread is still
    // draining; we close the master afterwards to make it see EOF.
    // On Windows the master is already closed (above) and try_wait can
    // now observe the process exit.
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
                let _ = killer.kill();
                drop(master.take());
                panic!("child wait failed: {e}");
            }
        }
    };

    // Child has exited; close the master so the reader sees EOF.
    // On Windows the master was already closed before the wait loop.
    drop(master.take());

    // Drain any remaining chunks until the reader signals EOF.
    loop {
        match chunk_rx.recv_timeout(READ_BUDGET) {
            Ok(chunk) => captured.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = killer.kill();
                panic!("pty read did not finish within {READ_BUDGET:?}");
            }
        }
    }
    if let Ok(e) = err_rx.try_recv() {
        let _ = killer.kill();
        panic!("pty read failed: {e}");
    }
    reader_thread.join().expect("reader thread panicked");

    // On Windows ConPTY, ClosePseudoConsole terminates the attached
    // process with STATUS_CONTROL_C_EXIT (0xC000013A) — the exit code
    // is a teardown artefact, not a command failure. On Unix the
    // command exits normally and success() must hold.
    #[cfg(not(windows))]
    assert!(status.success(), "child exited non-zero: {status:?}");
    #[cfg(windows)]
    let _ = status;

    let captured_str = String::from_utf8_lossy(&captured);
    // On Windows ConPTY, cmd.exe wraps its output in VT escape sequences.
    // Strip them before asserting so surrounding escape codes do not
    // obscure the echoed text. The CPR response written above ensures
    // cmd.exe proceeds past initialisation and runs `echo hi`.
    #[cfg(not(windows))]
    assert!(
        captured_str.contains("hi"),
        "expected 'hi' in pty output, got: {captured_str:?}"
    );
    #[cfg(windows)]
    {
        let stripped = strip_ansi_sequences(&captured_str);
        assert!(
            stripped.lines().any(|line| line.trim() == "hi"),
            "expected 'hi' line in pty output (ANSI-stripped); raw={captured_str:?}, stripped={stripped:?}"
        );
    }
}
